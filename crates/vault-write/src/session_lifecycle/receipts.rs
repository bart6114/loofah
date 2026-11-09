use std::path::{Component, Path, PathBuf};

use hypr_loofah_protocol::{
    DeleteSessionOutcome, DeleteSessionRequest, RestoreSessionOutcome, RestoreSessionRequest,
    is_session_lifecycle_revision,
};
use serde::{Deserialize, Serialize, de::DeserializeOwned};

use super::{LifecycleError, SessionLifecycleKey, rejected, uncertain};
use crate::{StoreError, operation_io};

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Binding {
    pub vault_id: String,
    pub namespace: String,
    pub request_id: String,
}
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Target {
    pub session_id: String,
    pub source: PathBuf,
    pub trash: PathBuf,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Receipt<I, O> {
    pub schema_version: u32,
    pub binding: Binding,
    pub input: I,
    pub target: Option<Target>,
    pub prepared_revision: Option<String>,
    pub deleted_at: Option<String>,
    pub state: State<O>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum State<O> {
    Prepared,
    Completed { outcome: O },
    Uncertain,
}
pub(super) type DeleteReceipt = Receipt<DeleteSessionRequest, DeleteSessionOutcome>;
pub(super) type RestoreReceipt = Receipt<RestoreSessionRequest, RestoreSessionOutcome>;

impl SessionLifecycleKey {
    pub(super) fn binding(&self) -> Result<Binding, LifecycleError> {
        for value in [&self.vault_id, &self.namespace] {
            if value.is_empty() || value.len() > 4096 || value.chars().any(char::is_control) {
                return Err(rejected(StoreError::Serialize(
                    "Invalid lifecycle identity".into(),
                )));
            }
        }
        Ok(Binding {
            vault_id: self.vault_id.clone(),
            namespace: self.namespace.clone(),
            request_id: self.request_id.to_string(),
        })
    }
    pub(super) fn token(&self) -> String {
        let namespace =
            serde_json::to_string(&("session-lifecycle-delete", &self.vault_id, &self.namespace))
                .unwrap();
        operation_io::operation_key(&namespace, self.request_id)
    }
    pub(super) fn restore_key(&self) -> String {
        let namespace =
            serde_json::to_string(&("session-lifecycle-restore", &self.vault_id, &self.namespace))
                .unwrap();
        operation_io::operation_key(&namespace, self.request_id)
    }
}
pub(super) fn deletion_path(root: &Path, token: &str) -> Result<PathBuf, LifecycleError> {
    if !is_session_lifecycle_revision(token) {
        return Err(rejected(StoreError::Serialize(
            "Invalid trash token".into(),
        )));
    }
    Ok(root.join("delete").join(format!("{token}.json")))
}
pub(super) fn restoration_path(root: &Path, key: &SessionLifecycleKey) -> PathBuf {
    root.join("restore")
        .join(format!("{}.json", key.restore_key()))
}

pub(super) async fn read<T: DeserializeOwned + Send + 'static>(
    path: PathBuf,
) -> Result<Option<T>, LifecycleError> {
    tokio::task::spawn_blocking(move || {
        operation_io::read_receipt::<crate::metadata_revision::UniqueValue>(&path)?
            .map(|raw| {
                serde_json::from_value(raw.0)
                    .map_err(|_| StoreError::Io("Invalid lifecycle receipt".into()))
            })
            .transpose()
    })
    .await
    .map_err(|_| uncertain("Lifecycle receipt reader stopped"))?
    .map_err(LifecycleError::Uncertain)
}
pub(super) async fn persist<T: Serialize + Clone + Send + 'static>(
    owner: &crate::SessionStore,
    path: &Path,
    value: &T,
) -> Result<(), StoreError> {
    let path = path.to_path_buf();
    let value = value.clone();
    owner
        .spawn_blocking(move || {
            if let Some(parent) = path.parent() {
                for directory in [parent.parent(), Some(parent)].into_iter().flatten() {
                    match directory.symlink_metadata() {
                        Ok(meta) if !meta.is_dir() || meta.file_type().is_symlink() => {
                            return Err(StoreError::Io(
                                "Invalid lifecycle receipt directory".into(),
                            ));
                        }
                        Err(cause) if cause.kind() != std::io::ErrorKind::NotFound => {
                            return Err(super::io_error(cause));
                        }
                        _ => {}
                    }
                }
            }
            operation_io::persist(&path, &value)
        })
        .await
        .map_err(|_| StoreError::Io("Lifecycle receipt writer stopped".into()))?
}
pub(super) fn validate<I: PartialEq, O>(
    receipt: &Receipt<I, O>,
    binding: &Binding,
    input: &I,
) -> Result<(), LifecycleError> {
    if receipt.schema_version != 1 {
        return Err(uncertain("Unsupported lifecycle receipt version"));
    }
    if &receipt.binding != binding || &receipt.input != input {
        return Err(rejected(StoreError::Conflict(
            "Request UUID was already used with different lifecycle input".into(),
        )));
    }
    if receipt
        .prepared_revision
        .as_deref()
        .is_some_and(|value| !is_session_lifecycle_revision(value))
    {
        return Err(uncertain("Invalid lifecycle revision"));
    }
    Ok(())
}
pub(super) fn validate_delete(
    receipt: &DeleteReceipt,
    vault: &str,
    token: &str,
) -> Result<(), LifecycleError> {
    let uuid = uuid::Uuid::parse_str(&receipt.binding.request_id)
        .map_err(|_| uncertain("Invalid lifecycle receipt identity"))?;
    let key = SessionLifecycleKey {
        vault_id: receipt.binding.vault_id.clone(),
        namespace: receipt.binding.namespace.clone(),
        request_id: uuid,
    };
    let binding = key.binding()?;
    validate(receipt, &binding, &receipt.input)?;
    if vault != receipt.binding.vault_id || key.token() != token {
        return Err(uncertain(
            "Trash token belongs to a different vault or operation",
        ));
    }
    hypr_loofah_protocol::validate_delete_session_request(&receipt.input)
        .map_err(|_| uncertain("Invalid lifecycle receipt request"))?;
    if receipt
        .prepared_revision
        .as_ref()
        .is_some_and(|revision| revision != &receipt.input.expected_revision)
    {
        return Err(uncertain("Deletion intent revision changed"));
    }
    if receipt.target.is_some() != receipt.deleted_at.is_some()
        || receipt
            .deleted_at
            .as_deref()
            .is_some_and(|value| chrono::DateTime::parse_from_rfc3339(value).is_err())
    {
        return Err(uncertain("Invalid original deletion timestamp"));
    }
    if let Some(target) = &receipt.target {
        validate_target(target, token)?;
        if target.session_id != receipt.input.session_id {
            return Err(uncertain("Trash session identity changed"));
        }
    }
    match &receipt.state {
        State::Prepared if receipt.target.is_none() || receipt.prepared_revision.is_none() => {
            return Err(uncertain("Incomplete lifecycle intent"));
        }
        State::Completed {
            outcome:
                DeleteSessionOutcome::Trashed {
                    trash_token,
                    revision,
                },
        } if receipt.target.is_none()
            || trash_token != token
            || Some(revision) != receipt.prepared_revision.as_ref() =>
        {
            return Err(uncertain("Invalid historical trash outcome"));
        }
        State::Completed {
            outcome: DeleteSessionOutcome::Conflict { snapshot },
        } if receipt.target.is_some()
            || receipt.prepared_revision.is_some()
            || snapshot.session_id != receipt.input.session_id
            || !is_session_lifecycle_revision(&snapshot.revision) =>
        {
            return Err(uncertain("Invalid deletion conflict outcome"));
        }
        State::Completed {
            outcome: DeleteSessionOutcome::NotFound,
        } if receipt.target.is_some() || receipt.prepared_revision.is_some() => {
            return Err(uncertain("Invalid missing deletion outcome"));
        }
        _ => {}
    }
    Ok(())
}
pub(super) fn validate_target(target: &Target, token: &str) -> Result<(), LifecycleError> {
    let expected = crate::paths::validated_session_dir(&target.session_id)
        .map_err(|_| uncertain("Invalid lifecycle session"))?;
    if expected != target.source {
        return Err(uncertain("Invalid original restore destination"));
    }
    let parts: Vec<_> = target.trash.components().collect();
    if parts.len() != 4
        || parts
            .iter()
            .any(|part| !matches!(part, Component::Normal(_)))
        || parts[0].as_os_str() != ".trash"
        || parts[2].as_os_str() != "sessions"
        || parts[1]
            .as_os_str()
            .to_str()
            .is_none_or(|date| chrono::NaiveDate::parse_from_str(date, "%Y-%m-%d").is_err())
    {
        return Err(uncertain("Invalid lifecycle trash destination"));
    }
    let name = parts[3]
        .as_os_str()
        .to_str()
        .ok_or_else(|| uncertain("Invalid lifecycle trash destination"))?;
    if name != token
        && !name
            .strip_prefix(&format!("{token}-"))
            .is_some_and(|suffix| {
                !suffix.is_empty() && suffix.bytes().all(|byte| byte.is_ascii_digit())
            })
    {
        return Err(uncertain(
            "Trash destination is not bound to this operation",
        ));
    }
    Ok(())
}

pub(super) fn validate_restore(
    receipt: &RestoreReceipt,
    binding: &Binding,
    input: &RestoreSessionRequest,
) -> Result<(), LifecycleError> {
    validate(receipt, binding, input)?;
    if receipt.deleted_at.is_some() {
        return Err(uncertain("Invalid restore receipt timestamp"));
    }
    if receipt
        .prepared_revision
        .as_ref()
        .is_some_and(|revision| revision != &input.expected_revision)
    {
        return Err(uncertain("Restore intent revision changed"));
    }
    if let Some(target) = &receipt.target {
        validate_target(target, &input.trash_token)?;
    }
    match &receipt.state {
        State::Prepared if receipt.target.is_none() || receipt.prepared_revision.is_none() => {
            return Err(uncertain("Incomplete restore intent"));
        }
        State::Completed {
            outcome:
                RestoreSessionOutcome::Restored {
                    session_id,
                    revision,
                },
        } if receipt
            .target
            .as_ref()
            .is_none_or(|target| &target.session_id != session_id)
            || Some(revision) != receipt.prepared_revision.as_ref() =>
        {
            return Err(uncertain("Invalid historical restore outcome"));
        }
        State::Completed {
            outcome:
                RestoreSessionOutcome::Conflict {
                    current_revision: Some(revision),
                    ..
                },
        } if !is_session_lifecycle_revision(revision) => {
            return Err(uncertain("Invalid restore conflict revision"));
        }
        _ => {}
    }
    Ok(())
}
