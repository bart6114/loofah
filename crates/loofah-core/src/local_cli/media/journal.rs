use crate::{VaultContext, vault::error};
use hypr_loofah_protocol::{
    ApplicationError, ErrorCode, LocalCliOperation, LocalCliReply, LocalTranscriptionOutcome,
};
use hypr_vault_read::SessionMeta;
use hypr_vault_write::recording_publish::RecordingPublicationRequest;
use serde::{Deserialize, Serialize};
use std::{
    fs::{File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::Arc,
};

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Record {
    version: u32,
    fingerprint: String,
    #[serde(default)]
    operation: Option<LocalCliOperation>,
    #[serde(default)]
    config: Option<FrozenTranscriptionConfig>,
    #[serde(default)]
    import: Option<ImportRecovery>,
    #[serde(default)]
    transcription: Option<LocalTranscriptionOutcome>,
    result: Option<Result<LocalCliReply, ApplicationError>>,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct FrozenTranscriptionConfig {
    provider: Option<String>,
    model: Option<String>,
    languages: Option<Vec<String>>,
}
impl From<crate::settings::AppConfig> for FrozenTranscriptionConfig {
    fn from(config: crate::settings::AppConfig) -> Self {
        Self {
            provider: config.current_stt_provider,
            model: config.current_stt_model,
            languages: config.meeting_languages,
        }
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ImportRecovery {
    pub meta: SessionMeta,
    pub publication: RecordingPublicationRequest,
}

pub(super) struct Claim {
    context: Arc<VaultContext>,
    path: PathBuf,
    record: Record,
}
impl Claim {
    pub async fn open(
        context: Arc<VaultContext>,
        root: PathBuf,
        request: &str,
        fingerprint: String,
        operation: LocalCliOperation,
        config: crate::settings::AppConfig,
    ) -> Result<Self, ApplicationError> {
        let request = request.to_owned();
        tokio::task::spawn_blocking(move || {
            let path = root.join(format!("{request}.json"));
            check_parents(&path)?;
            std::fs::create_dir_all(&root).map_err(io_error)?;
            let wanted = Record {
                version: 1,
                fingerprint,
                operation: Some(operation),
                config: Some(config.into()),
                import: None,
                transcription: None,
                result: None,
            };
            let mut options = OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            let record = match options.open(&path) {
                Ok(mut file) => {
                    file.write_all(&serde_json::to_vec(&wanted).map_err(io_error)?)
                        .map_err(io_error)?;
                    file.sync_all().map_err(io_error)?;
                    sync_parent(&path)?;
                    wanted
                }
                Err(cause) if cause.kind() == std::io::ErrorKind::AlreadyExists => {
                    let current = read(&path)?;
                    if current.fingerprint != wanted.fingerprint {
                        return Err(error(ErrorCode::Conflict, "Local media request changed"));
                    }
                    if current.result.is_none() {
                        return Err(super::unknown("Local media work was admitted previously"));
                    }
                    current
                }
                Err(cause) => return Err(io_error(cause)),
            };
            Ok(Self {
                context,
                path,
                record,
            })
        })
        .await
        .map_err(io_error)?
    }

    pub async fn recover(
        context: Arc<VaultContext>,
        root: PathBuf,
        request: &str,
    ) -> Result<Self, ApplicationError> {
        let path = root.join(format!("{request}.json"));
        tokio::task::spawn_blocking(move || {
            let record = read(&path)?;
            Ok(Self {
                context,
                path,
                record,
            })
        })
        .await
        .map_err(io_error)?
    }

    pub fn result(&self) -> Option<&Result<LocalCliReply, ApplicationError>> {
        self.record.result.as_ref()
    }
    pub fn operation(&self) -> Option<LocalCliOperation> {
        self.record.operation.clone()
    }
    pub fn config(&self) -> Result<crate::settings::AppConfig, ApplicationError> {
        let config =
            self.record.config.as_ref().ok_or_else(|| {
                super::unknown("Original transcription settings were not retained")
            })?;
        Ok(crate::settings::AppConfig {
            current_stt_provider: config.provider.clone(),
            current_stt_model: config.model.clone(),
            meeting_languages: config.languages.clone(),
            ..Default::default()
        })
    }

    pub fn import(&self) -> Option<ImportRecovery> {
        self.record.import.clone()
    }
    pub fn transcription(&self) -> Option<LocalTranscriptionOutcome> {
        self.record.transcription.clone()
    }
    pub async fn bind_import(&mut self, import: ImportRecovery) -> Result<(), ApplicationError> {
        self.record.import = Some(import);
        self.persist().await
    }
    pub async fn bind_transcription(
        &mut self,
        transcription: LocalTranscriptionOutcome,
    ) -> Result<(), ApplicationError> {
        self.record.transcription = Some(transcription);
        self.persist().await
    }
    pub async fn finish(
        mut self,
        result: Result<LocalCliReply, ApplicationError>,
    ) -> Result<(), ApplicationError> {
        self.record.result = Some(result);
        self.persist().await
    }
    async fn persist(&self) -> Result<(), ApplicationError> {
        let context = self.context.clone();
        let path = self.path.clone();
        let record = self.record.clone();
        tokio::task::spawn_blocking(move || {
            let _context = context;
            check_parents(&path)?;
            let bytes = serde_json::to_string(&record).map_err(io_error)?;
            hypr_storage::fs::atomic_write(&path, &bytes)
                .map_err(|_| super::unknown("Local media work could not be checkpointed"))?;
            sync_parent(&path)
        })
        .await
        .map_err(io_error)?
    }
}
fn read(path: &Path) -> Result<Record, ApplicationError> {
    check_parents(path)?;
    let metadata = std::fs::symlink_metadata(path).map_err(|cause| {
        if cause.kind() == std::io::ErrorKind::NotFound {
            error(ErrorCode::NotFound, "Local media request not found")
        } else {
            io_error(cause)
        }
    })?;
    if !metadata.is_file() || metadata.file_type().is_symlink() || metadata.len() > 4 * 1024 * 1024
    {
        return Err(super::unknown("Local media receipt requires review"));
    }
    let mut bytes = Vec::new();
    File::open(path)
        .map_err(io_error)?
        .take(metadata.len().saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(io_error)?;
    if bytes.len() as u64 != metadata.len() {
        return Err(super::unknown("Local media receipt changed while reading"));
    }
    let record: Record = serde_json::from_slice(&bytes)
        .map_err(|_| super::unknown("Local media receipt requires review"))?;
    if record.version != 1
        || record.fingerprint.len() != 64
        || !record
            .fingerprint
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit())
    {
        return Err(super::unknown(
            "Local media receipt version requires review",
        ));
    }
    Ok(record)
}
fn check_parents(path: &Path) -> Result<(), ApplicationError> {
    for parent in path.ancestors() {
        match std::fs::symlink_metadata(parent) {
            Ok(meta) if meta.file_type().is_symlink() || (parent != path && !meta.is_dir()) => {
                return Err(error(
                    ErrorCode::Unavailable,
                    "Local media operation storage is unsafe",
                ));
            }
            Ok(_) => {}
            Err(cause) if cause.kind() == std::io::ErrorKind::NotFound => {}
            Err(cause) => return Err(io_error(cause)),
        }
    }
    Ok(())
}
fn sync_parent(path: &Path) -> Result<(), ApplicationError> {
    #[cfg(unix)]
    {
        File::open(path.parent().unwrap())
            .and_then(|file| file.sync_all())
            .map_err(io_error)?;
    }
    Ok(())
}
fn io_error(cause: impl std::fmt::Display) -> ApplicationError {
    error(
        ErrorCode::Internal,
        &format!("Local media state could not be retained: {cause}"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn frozen_transcription_projection_contains_only_model_preferences() {
        let config = crate::settings::AppConfig {
            current_stt_provider: Some("fmtr".into()),
            current_stt_model: Some("QuantizedBase".into()),
            meeting_languages: Some(vec!["en".into()]),
            current_llm_model: Some("must-not-enter-media-receipt".into()),
            ..Default::default()
        };
        let frozen = FrozenTranscriptionConfig::from(config);
        let encoded = serde_json::to_value(frozen).unwrap();
        assert_eq!(
            encoded,
            serde_json::json!({"provider":"fmtr","model":"QuantizedBase","languages":["en"]})
        );
    }
}
