use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
pub struct SessionLifecycleSnapshot {
    pub session_id: String,
    pub revision: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(deny_unknown_fields)]
pub struct DeleteSessionRequest {
    pub session_id: String,
    pub expected_revision: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum DeleteSessionOutcome {
    Trashed {
        trash_token: String,
        revision: String,
    },
    Conflict {
        snapshot: SessionLifecycleSnapshot,
    },
    NotFound,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(deny_unknown_fields)]
pub struct RestoreSessionRequest {
    pub trash_token: String,
    pub expected_revision: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum RestoreSessionConflictReason {
    DestinationOccupied,
    TrashChanged,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RestoreSessionOutcome {
    Restored {
        session_id: String,
        revision: String,
    },
    Conflict {
        reason: RestoreSessionConflictReason,
        current_revision: Option<String>,
    },
    NotFound,
}

pub fn is_session_lifecycle_revision(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}
pub fn validate_delete_session_request(input: &DeleteSessionRequest) -> Result<(), &'static str> {
    crate::validate_session_id(&input.session_id).map_err(|_| "Invalid session identifier")?;
    if !is_session_lifecycle_revision(&input.expected_revision) {
        return Err("A session revision is required");
    }
    Ok(())
}
pub fn validate_restore_session_request(input: &RestoreSessionRequest) -> Result<(), &'static str> {
    if !is_session_lifecycle_revision(&input.trash_token) {
        return Err("Invalid trash token");
    }
    if !is_session_lifecycle_revision(&input.expected_revision) {
        return Err("A session revision is required");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn requests_accept_only_checked_ids_revisions_and_opaque_tokens() {
        let revision = "a".repeat(64);
        let delete = DeleteSessionRequest {
            session_id: "note".into(),
            expected_revision: revision.clone(),
        };
        let restore = RestoreSessionRequest {
            trash_token: "b".repeat(64),
            expected_revision: revision.clone(),
        };
        assert!(validate_delete_session_request(&delete).is_ok());
        assert!(validate_restore_session_request(&restore).is_ok());
        for invalid in ["", "../note", "CON", "note.", "a/b"] {
            assert!(
                validate_delete_session_request(&DeleteSessionRequest {
                    session_id: invalid.into(),
                    ..delete.clone()
                })
                .is_err()
            );
        }
        for invalid in ["", ".trash/2026/note", "../token", "A", "future:v2", "a\n"] {
            assert!(
                validate_restore_session_request(&RestoreSessionRequest {
                    trash_token: invalid.into(),
                    ..restore.clone()
                })
                .is_err()
            );
            assert!(
                validate_delete_session_request(&DeleteSessionRequest {
                    expected_revision: invalid.into(),
                    ..delete.clone()
                })
                .is_err()
            );
        }
        assert!(
            serde_json::from_value::<DeleteSessionRequest>(
                json!({"session_id":"note","expected_revision":revision,"force":true})
            )
            .is_err()
        );
        assert!(serde_json::from_value::<RestoreSessionRequest>(json!({"trash_token":restore.trash_token,"expected_revision":restore.expected_revision,"destination":"elsewhere"})).is_err());
    }

    #[test]
    fn historical_outcomes_round_trip_without_filesystem_paths() {
        let revision = "a".repeat(64);
        let deleted = DeleteSessionOutcome::Trashed {
            trash_token: "b".repeat(64),
            revision: revision.clone(),
        };
        let value = serde_json::to_value(&deleted).unwrap();
        assert_eq!(value["kind"], "trashed");
        assert_eq!(
            serde_json::from_value::<DeleteSessionOutcome>(value).unwrap(),
            deleted
        );
        let restored = RestoreSessionOutcome::Restored {
            session_id: "note".into(),
            revision,
        };
        assert_eq!(
            serde_json::from_value::<RestoreSessionOutcome>(
                serde_json::to_value(&restored).unwrap()
            )
            .unwrap(),
            restored
        );
    }
}
