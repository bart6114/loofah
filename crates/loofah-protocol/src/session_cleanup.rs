use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(deny_unknown_fields)]
pub struct DeleteEmptySessionRequest {
    pub session_id: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum DeleteEmptySessionOutcome {
    Deleted,
    NotEmpty,
    NotFound,
}

pub fn validate_delete_empty_session_request(
    input: &DeleteEmptySessionRequest,
) -> Result<(), &'static str> {
    crate::validate_session_id(&input.session_id).map_err(|_| "Invalid session identifier")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn cleanup_binds_one_opaque_session_and_never_accepts_a_client_emptiness_claim_or_path() {
        let input: DeleteEmptySessionRequest =
            serde_json::from_value(json!({"session_id":"note"})).unwrap();
        assert!(validate_delete_empty_session_request(&input).is_ok());
        for id in ["", "../note", "CON", "note.", "a/b"] {
            assert!(
                validate_delete_empty_session_request(&DeleteEmptySessionRequest {
                    session_id: id.into()
                })
                .is_err()
            );
        }
        for field in ["empty", "force", "path", "expected_revision"] {
            let mut value = json!({"session_id":"note"});
            value[field] = json!(true);
            assert!(serde_json::from_value::<DeleteEmptySessionRequest>(value).is_err());
        }
        for (outcome, kind) in [
            (DeleteEmptySessionOutcome::Deleted, "deleted"),
            (DeleteEmptySessionOutcome::NotEmpty, "not_empty"),
            (DeleteEmptySessionOutcome::NotFound, "not_found"),
        ] {
            let value = json!({"kind":kind});
            assert_eq!(serde_json::to_value(outcome).unwrap(), value);
            assert_eq!(
                serde_json::from_value::<DeleteEmptySessionOutcome>(value).unwrap(),
                outcome
            );
        }
    }
}
