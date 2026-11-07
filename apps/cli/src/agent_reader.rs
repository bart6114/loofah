use std::{path::Path, path::PathBuf, sync::Arc};

use hypr_agent_access as access;
use hypr_loofah_client::{RemoteOperation, RemoteReply};

use crate::{Result, profiles::RemoteTarget};

#[derive(Clone)]
pub(crate) enum AgentReader {
    Local {
        vault: Arc<PathBuf>,
        cache: hypr_search_cache::Cache,
    },
    Remote(RemoteTarget),
}

impl AgentReader {
    pub fn local(vault: PathBuf, cache: hypr_search_cache::Cache) -> Self {
        Self::Local {
            vault: Arc::new(vault),
            cache,
        }
    }

    pub fn vault(&self) -> Result<&Path> {
        match self {
            Self::Local { vault, .. } => Ok(vault),
            Self::Remote(_) => Err(crate::Error::operation(
                "run remote command",
                "This command is not available for server profiles yet. No local vault was accessed.",
            )),
        }
    }

    pub async fn list(&self, input: access::ListMeetingsInput) -> Result<access::MeetingPage> {
        match self {
            Self::Local { cache, .. } => Ok(access::list_meetings(cache, input).await?),
            Self::Remote(target) => match target
                .execute(|vault_id| RemoteOperation::AgentList { vault_id, input })
                .await?
            {
                RemoteReply::AgentList(page) => Ok(page),
                _ => Err(crate::profiles::unexpected()),
            },
        }
    }

    pub async fn search(&self, input: access::SearchMeetingsInput) -> Result<access::SearchPage> {
        match self {
            Self::Local { cache, .. } => Ok(access::search_meetings(cache, input).await?),
            Self::Remote(target) => match target
                .execute(|vault_id| RemoteOperation::AgentSearch { vault_id, input })
                .await?
            {
                RemoteReply::AgentSearch(page) => Ok(page),
                _ => Err(crate::profiles::unexpected()),
            },
        }
    }

    pub async fn meeting(&self, input: access::GetMeetingInput) -> Result<access::Meeting> {
        match self {
            Self::Local { vault, .. } => Ok(access::get_meeting(vault, input).await?),
            Self::Remote(target) => match target
                .execute(|vault_id| RemoteOperation::AgentMeeting {
                    vault_id,
                    session_id: input.meeting_id,
                })
                .await?
            {
                RemoteReply::AgentMeeting(meeting) => Ok(meeting),
                _ => Err(crate::profiles::unexpected()),
            },
        }
    }

    pub async fn transcript(
        &self,
        input: access::GetMeetingTranscriptInput,
    ) -> Result<access::MeetingTranscript> {
        match self {
            Self::Local { vault, .. } => Ok(access::get_meeting_transcript(vault, input).await?),
            Self::Remote(target) => match target
                .execute(|vault_id| RemoteOperation::AgentTranscript {
                    vault_id,
                    session_id: input.meeting_id,
                })
                .await?
            {
                RemoteReply::AgentTranscript(transcript) => Ok(transcript),
                _ => Err(crate::profiles::unexpected()),
            },
        }
    }
}
