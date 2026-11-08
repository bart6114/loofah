use super::*;
use hypr_vault_write::transcript_publication::{
    TranscriptPublicationBaseline, TranscriptSourcePreparation, TranscriptSourceSnapshot,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs::File,
    io::{Read, Seek, Write},
    path::{Path, PathBuf},
    sync::OnceLock,
};
use tokio::sync::watch;

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct AudioBinding {
    source: hypr_vault_read::AudioSource,
    layout: hypr_vault_read::AudioLayout,
}
impl From<SessionAudio> for AudioBinding {
    fn from(audio: SessionAudio) -> Self {
        Self {
            source: audio.source,
            layout: audio.layout,
        }
    }
}
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Binding {
    pub recording_sha256: String,
    pub selection: String,
    pub transcript_revision: String,
    audio: AudioBinding,
    pub session_created_at: String,
    pub memo_sha256: String,
}
impl Binding {
    pub fn validate(&self, request: &TranscriptionJobRequest) -> Result<(), ApplicationError> {
        if self.recording_sha256 != request.expected_recording_sha256
            || !is_transcription_sha256(&self.recording_sha256)
            || !is_transcription_sha256(&self.transcript_revision)
            || !is_transcription_sha256(&self.memo_sha256)
            || !matches!(
                self.selection.as_str(),
                "audio.wav"
                    | "audio.mp3"
                    | "audio.ogg"
                    | "audio/audio.wav"
                    | "audio/audio.mp3"
                    | "audio/audio.ogg"
            )
            || chrono::DateTime::parse_from_rfc3339(&self.session_created_at).is_err()
        {
            return Err(storage("invalid transcription source binding"));
        }
        Ok(())
    }
    pub fn matches(&self, source: &TranscriptSourceSnapshot, created: &str) -> bool {
        self.recording_sha256 == source.recording_sha256()
            && self.selection == source.recording_selection()
            && self.transcript_revision == source.transcript_revision()
            && self.audio == source.audio().into()
            && self.session_created_at == created
    }
}

pub struct TrustedTranscriptionSource {
    path: PathBuf,
    owner: Box<dyn Send>,
}
impl TrustedTranscriptionSource {
    pub(crate) fn capture_suffix(path: PathBuf, owner: Box<dyn Send>) -> Self {
        Self { path, owner }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
    pub fn into_parts(self) -> (PathBuf, Box<dyn Send>) {
        (self.path, self.owner)
    }
}
struct Released(watch::Sender<bool>);
impl Drop for Released {
    fn drop(&mut self) {
        self.0.send_replace(true);
    }
}
struct SourceOwner {
    _file: File,
    _source: Arc<TranscriptSourceSnapshot>,
    _context: Arc<VaultContext>,
    // Final field: completion is evidence that descriptors and Context ownership
    // have actually been released, not just that an async waiter was dropped.
    _released: Released,
}
pub(super) struct Prepared {
    pub input: TranscriptionInput,
    pub metadata: TranscriptionMetadata,
    pub baseline: TranscriptPublicationBaseline,
    pub binding: Binding,
    pub released: watch::Receiver<bool>,
}

pub(super) async fn prepare(
    context: Arc<VaultContext>,
    journal: &ledger::Journal,
    record: &ledger::Record,
    cancel: CancellationToken,
) -> Result<Prepared, ApplicationError> {
    context.ensure_recording_publication_ready(&record.request.session_id)?;
    let source = snapshot(&context, &record.request.session_id).await?;
    context.ensure_recording_publication_ready(&record.request.session_id)?;
    if source.recording_sha256() != record.request.expected_recording_sha256 {
        return Err(error(
            ErrorCode::Conflict,
            "Recording changed before transcription",
        ));
    }
    let meta = context.read_metadata(&record.request.session_id).await?;
    let note = context
        .read_note(&record.request.session_id)
        .await?
        .markdown
        .unwrap_or_default();
    let memo = tokio::task::spawn_blocking(move || {
        if note.len() > 8 * 1024 * 1024 {
            return Err(capacity());
        }
        if note.is_empty() {
            return Ok(String::new());
        }
        let json = hypr_tiptap::md_to_tiptap_json(&note).map_err(storage)?;
        let memo = serde_json::to_string(&json).map_err(storage)?;
        if memo.len() > 16 * 1024 * 1024 {
            return Err(capacity());
        }
        Ok(memo)
    })
    .await
    .map_err(storage)??;
    if cancel.is_cancelled() {
        return Err(error(ErrorCode::Unavailable, "Transcription stopped"));
    }
    let binding = Binding {
        recording_sha256: source.recording_sha256().into(),
        selection: source.recording_selection().into(),
        transcript_revision: source.transcript_revision().into(),
        audio: source.audio().into(),
        session_created_at: meta.meta.created_at,
        memo_sha256: ledger::hash(memo.as_bytes()),
    };
    let bytes = source.recording_file().metadata().map_err(storage)?.len();
    if bytes > u32::MAX as u64 {
        return Err(capacity());
    }
    journal.reserve_audio(bytes).await?;
    let root = journal.root().await?;
    let extension = Path::new(source.recording_selection())
        .extension()
        .and_then(|s| s.to_str())
        .ok_or_else(unavailable)?;
    let path = root.join(format!("{}.source.{extension}", record.snapshot.id));
    let copied = source.clone();
    let owner = context.clone();
    let copied_path = path.clone();
    let expected = binding.recording_sha256.clone();
    let file = tokio::task::spawn_blocking(move || {
        let _context = owner;
        ledger::ensure_directory(&root)?;
        let temporary = root.join(format!(".{}.source.tmp", uuid::Uuid::new_v4()));
        let mut output = File::options()
            .create_new(true)
            .write(true)
            .open(&temporary)
            .map_err(storage)?;
        let mut input = copied.recording_file().try_clone().map_err(storage)?;
        input.rewind().map_err(storage)?;
        let mut total = 0_u64;
        let mut buffer = [0_u8; 64 * 1024];
        let mut digest = Sha256::new();
        loop {
            if cancel.is_cancelled() {
                return Err(error(ErrorCode::Unavailable, "Transcription stopped"));
            }
            let read = input.read(&mut buffer).map_err(storage)?;
            if read == 0 {
                break;
            }
            total = total.checked_add(read as u64).ok_or_else(capacity)?;
            if total > bytes || total > u32::MAX as u64 {
                return Err(capacity());
            }
            digest.update(&buffer[..read]);
            output.write_all(&buffer[..read]).map_err(storage)?;
        }
        if total != bytes || ledger::hex(&digest.finalize()) != expected {
            return Err(error(
                ErrorCode::Conflict,
                "Recording changed while copying transcription source",
            ));
        }
        output.sync_all().map_err(storage)?;
        let mut permissions = output.metadata().map_err(storage)?.permissions();
        permissions.set_readonly(true);
        output.set_permissions(permissions).map_err(storage)?;
        drop(output);
        hypr_storage::fs::rename_no_replace(&temporary, &copied_path).map_err(storage)?;
        ledger::sync_parent(&copied_path)?;
        File::open(copied_path).map_err(storage)
    })
    .await
    .map_err(storage)??;
    let metadata = TranscriptionMetadata {
        session_id: record.request.session_id.clone(),
        user_id: record.member.clone(),
        created_at: record.snapshot.created_at.clone(),
        memo_md: memo,
        audio: source.audio(),
        materialized: Arc::new(OnceLock::new()),
    };
    let (completed, released) = watch::channel(false);
    let source_owner = SourceOwner {
        _file: file,
        _source: source.clone(),
        _context: context,
        _released: Released(completed),
    };
    let input = TranscriptionInput {
        source: TrustedTranscriptionSource {
            path,
            owner: Box::new(source_owner),
        },
        metadata: metadata.clone(),
        request: record.request.clone(),
    };
    Ok(Prepared {
        input,
        metadata,
        baseline: source.baseline(),
        binding,
        released,
    })
}

pub(super) async fn snapshot(
    context: &VaultContext,
    session: &str,
) -> Result<Arc<TranscriptSourceSnapshot>, ApplicationError> {
    match context
        .store
        .prepare_transcript_source(session)
        .await
        .map_err(|cause| crate::vault::storage_error(cause, false))?
    {
        Some(TranscriptSourcePreparation::Ready(source)) => Ok(Arc::from(source)),
        Some(TranscriptSourcePreparation::Conflict) => {
            Err(error(ErrorCode::Conflict, "A transcript already exists"))
        }
        Some(TranscriptSourcePreparation::Busy) => Err(error(
            ErrorCode::Unavailable,
            "Recording or transcript is busy",
        )),
        Some(TranscriptSourcePreparation::NoRecording) => {
            Err(error(ErrorCode::NotFound, "Recording not found"))
        }
        None => Err(error(ErrorCode::NotFound, "Session not found")),
    }
}
