use std::{sync::Arc, time::Duration};

use hypr_loofah_protocol::*;
use reqwest::Method;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio_util::sync::CancellationToken;

use crate::{RemoteClient, Result, cancelled, failure, invalid, json_body, unavailable};

#[derive(Clone, Debug, Serialize, Deserialize, specta::Type)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RemoteOperation {
    Provision,
    Me,
    Vaults,
    Directory,
    CreateVault {
        input: CreateVaultRequest,
    },
    Vault {
        vault_id: String,
    },
    RenameVault {
        vault_id: String,
        input: UpdateVaultRequest,
    },
    DeleteVault {
        vault_id: String,
        expected_revision: String,
    },
    Memberships {
        vault_id: String,
    },
    SetMembership {
        vault_id: String,
        member_id: String,
        input: SetMembershipRequest,
    },
    RemoveMembership {
        vault_id: String,
        member_id: String,
        expected_revision: String,
    },
    Sessions {
        vault_id: String,
        limit: u32,
        after: Option<String>,
    },
    Session {
        vault_id: String,
        session_id: String,
    },
    CreateSession {
        vault_id: String,
        input: CreateSessionRequest,
    },
    Note {
        vault_id: String,
        session_id: String,
    },
    SaveNote {
        vault_id: String,
        session_id: String,
        input: SaveNoteRequest,
    },
    Search {
        vault_id: String,
        input: hypr_search_cache::SearchRequest,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize, specta::Type)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum RemoteReply {
    Me(Me),
    Vaults(Vec<VaultInfo>),
    Directory(Vec<MemberInfo>),
    Vault(VaultInfo),
    VaultMutation(VaultMutation),
    Memberships(Vec<Membership>),
    Sessions(SessionPage),
    Session(SessionDocument),
    Note(NoteSnapshot),
    NoteSave(NoteSaveOutcome),
    Search(hypr_search_cache::SearchResult),
}

impl RemoteClient {
    pub async fn execute(
        self: &Arc<Self>,
        operation: RemoteOperation,
        request_id: &str,
        cancel: CancellationToken,
    ) -> Result<RemoteReply> {
        if uuid::Uuid::parse_str(request_id).is_err() {
            return Err(invalid("A UUID request ID is required"));
        }
        if self.stop.is_cancelled() || cancel.is_cancelled() {
            return Err(cancelled());
        }
        let mut query: Vec<(&str, String)> = Vec::new();
        let mut body = None;
        let (method, segments) = match &operation {
            RemoteOperation::Provision => (Method::POST, vec!["me"]),
            RemoteOperation::Me => (Method::GET, vec!["me"]),
            RemoteOperation::Vaults => (Method::GET, vec!["vaults"]),
            RemoteOperation::Directory => (Method::GET, vec!["members"]),
            RemoteOperation::CreateVault { input } => {
                body = Some(value(input)?);
                (Method::POST, vec!["vaults"])
            }
            RemoteOperation::Vault { vault_id } => (Method::GET, vec!["vaults", vault_id]),
            RemoteOperation::RenameVault { vault_id, input } => {
                body = Some(value(input)?);
                (Method::PATCH, vec!["vaults", vault_id])
            }
            RemoteOperation::DeleteVault {
                vault_id,
                expected_revision,
            } => {
                query.push(("expected_revision", expected_revision.clone()));
                (Method::DELETE, vec!["vaults", vault_id])
            }
            RemoteOperation::Memberships { vault_id } => {
                (Method::GET, vec!["vaults", vault_id, "members"])
            }
            RemoteOperation::SetMembership {
                vault_id,
                member_id,
                input,
            } => {
                body = Some(value(input)?);
                (Method::PUT, vec!["vaults", vault_id, "members", member_id])
            }
            RemoteOperation::RemoveMembership {
                vault_id,
                member_id,
                expected_revision,
            } => {
                query.push(("expected_revision", expected_revision.clone()));
                (
                    Method::DELETE,
                    vec!["vaults", vault_id, "members", member_id],
                )
            }
            RemoteOperation::Sessions {
                vault_id,
                limit,
                after,
            } => {
                if !(1..=200).contains(limit) {
                    return Err(invalid("Page size must be between 1 and 200"));
                }
                query.push(("limit", limit.to_string()));
                if let Some(after) = after {
                    query.push(("after", after.clone()));
                }
                (Method::GET, vec!["vaults", vault_id, "sessions"])
            }
            RemoteOperation::Session {
                vault_id,
                session_id,
            } => (
                Method::GET,
                vec!["vaults", vault_id, "sessions", session_id],
            ),
            RemoteOperation::CreateSession { vault_id, input } => {
                body = Some(value(input)?);
                (Method::POST, vec!["vaults", vault_id, "sessions"])
            }
            RemoteOperation::Note {
                vault_id,
                session_id,
            } => (
                Method::GET,
                vec!["vaults", vault_id, "sessions", session_id, "note"],
            ),
            RemoteOperation::SaveNote {
                vault_id,
                session_id,
                input,
            } => {
                body = Some(value(input)?);
                (
                    Method::PUT,
                    vec!["vaults", vault_id, "sessions", session_id, "note"],
                )
            }
            RemoteOperation::Search { vault_id, input } => {
                body = Some(value(input)?);
                (Method::POST, vec!["vaults", vault_id, "search"])
            }
        };
        let mutation =
            !matches!(operation, RemoteOperation::Search { .. }) && method != Method::GET;
        let mut url = self.endpoint(&segments)?;
        url.query_pairs_mut().extend_pairs(query);
        let access = tokio::select! {
            _ = cancel.cancelled() => return Err(cancelled()),
            _ = self.stop.cancelled() => return Err(cancelled()),
            result = self.access_token() => result?,
        };
        let mut request = self
            .http
            .request(method, url)
            .bearer_auth(&access)
            .header("x-loofah-api-major", API_MAJOR.to_string())
            .header("x-request-id", request_id)
            .timeout(Duration::from_secs(35));
        if let Some(body) = body {
            request = request.json(&body);
        }
        let result = async {
            let response = request.send().await.map_err(|_| unavailable())?;
            let status = response.status();
            if status == reqwest::StatusCode::UNAUTHORIZED {
                self.expire_access(&access).await;
            }
            if status == reqwest::StatusCode::CONFLICT
                && matches!(operation, RemoteOperation::SaveNote { .. })
            {
                return Ok(RemoteReply::NoteSave(
                    json_body(response, 8 * 1024 * 1024).await?,
                ));
            }
            if !status.is_success() {
                return Err(json_body::<ApplicationError>(response, 131_072)
                    .await
                    .unwrap_or_else(|_| {
                        failure(ErrorCode::Unavailable, "The server rejected the request")
                    }));
            }
            let reply = match operation {
                RemoteOperation::Provision | RemoteOperation::Me => {
                    RemoteReply::Me(json_body(response, 131_072).await?)
                }
                RemoteOperation::Vaults => {
                    RemoteReply::Vaults(json_body(response, 1024 * 1024).await?)
                }
                RemoteOperation::Directory => {
                    RemoteReply::Directory(json_body(response, 1024 * 1024).await?)
                }
                RemoteOperation::CreateVault { .. } | RemoteOperation::Vault { .. } => {
                    RemoteReply::Vault(json_body(response, 131_072).await?)
                }
                RemoteOperation::RenameVault { .. }
                | RemoteOperation::DeleteVault { .. }
                | RemoteOperation::SetMembership { .. }
                | RemoteOperation::RemoveMembership { .. } => {
                    RemoteReply::VaultMutation(json_body(response, 131_072).await?)
                }
                RemoteOperation::Memberships { .. } => {
                    RemoteReply::Memberships(json_body(response, 1024 * 1024).await?)
                }
                RemoteOperation::Sessions { .. } => {
                    RemoteReply::Sessions(json_body(response, 8 * 1024 * 1024).await?)
                }
                RemoteOperation::CreateSession { .. } | RemoteOperation::Session { .. } => {
                    RemoteReply::Session(json_body(response, 8 * 1024 * 1024).await?)
                }
                RemoteOperation::Note { .. } => {
                    RemoteReply::Note(json_body(response, 8 * 1024 * 1024).await?)
                }
                RemoteOperation::SaveNote { .. } => {
                    RemoteReply::NoteSave(json_body(response, 8 * 1024 * 1024).await?)
                }
                RemoteOperation::Search { .. } => {
                    RemoteReply::Search(json_body(response, 8 * 1024 * 1024).await?)
                }
            };
            Ok(reply)
        };
        let result = tokio::select! {
            _ = cancel.cancelled() => Err(cancelled()),
            _ = self.stop.cancelled() => Err(cancelled()),
            result = result => result,
        };
        result.map_err(|mut error| {
            error.request_id.get_or_insert_with(|| request_id.into());
            if mutation && error.mutation_outcome.is_none() {
                error.mutation_outcome = Some(
                    if matches!(
                        error.code,
                        ErrorCode::Cancelled
                            | ErrorCode::Unavailable
                            | ErrorCode::Internal
                            | ErrorCode::IncompatibleVersion
                            | ErrorCode::ResourceLimit
                    ) {
                        MutationOutcome::Unknown
                    } else {
                        MutationOutcome::NotCommitted
                    },
                );
            }
            error
        })
    }
}

fn value(input: &impl Serialize) -> Result<Value> {
    serde_json::to_value(input).map_err(|_| invalid("Cannot encode request"))
}
