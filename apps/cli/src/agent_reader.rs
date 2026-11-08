use std::{path::Path, path::PathBuf};

use hypr_loofah_protocol::{LocalCliOperation, LocalCliReply};

use hypr_agent_access as access;
use hypr_loofah_client::{RemoteOperation, RemoteReply};

use crate::{Result, profiles::RemoteTarget};

#[derive(Clone)]
pub(crate) enum AgentReader {
    Local(crate::local_owner::LocalGateway),
    Remote(RemoteTarget),
}

impl AgentReader {
    pub fn local(vault: PathBuf, cache: hypr_search_cache::Cache) -> Self {
        Self::Local(crate::local_owner::LocalGateway::new(vault, cache))
    }

    pub fn local_gateway(&self) -> Result<&crate::local_owner::LocalGateway> {
        match self {
            Self::Local(gateway) => Ok(gateway),
            Self::Remote(_) => Err(crate::Error::operation(
                "run remote command",
                "This command is not available for server profiles yet. No local vault was accessed.",
            )),
        }
    }

    pub fn vault(&self) -> Result<&Path> {
        match self {
            Self::Local(gateway) => Ok(gateway.vault()),
            Self::Remote(_) => Err(crate::Error::operation(
                "run remote command",
                "This command is not available for server profiles yet. No local vault was accessed.",
            )),
        }
    }

    pub async fn list(&self, input: access::ListMeetingsInput) -> Result<access::MeetingPage> {
        match self {
            Self::Local(gateway) => match gateway
                .execute(LocalCliOperation::AgentList { input })
                .await?
                .response
                .result?
            {
                LocalCliReply::AgentList { page } => Ok(page),
                _ => Err(crate::profiles::unexpected()),
            },
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
            Self::Local(gateway) => match gateway
                .execute(LocalCliOperation::AgentSearch { input })
                .await?
                .response
                .result?
            {
                LocalCliReply::AgentSearch { page } => Ok(page),
                _ => Err(crate::profiles::unexpected()),
            },
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
            Self::Local(gateway) => match gateway
                .execute(LocalCliOperation::AgentMeeting {
                    session_id: input.meeting_id,
                })
                .await?
                .response
                .result?
            {
                LocalCliReply::AgentMeeting { meeting } => Ok(meeting),
                _ => Err(crate::profiles::unexpected()),
            },
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

    pub async fn export(&self, session_id: String) -> Result<hypr_loofah_protocol::AgentExport> {
        match self
            .local_gateway()?
            .execute(LocalCliOperation::AgentExport { session_id })
            .await?
            .response
            .result?
        {
            LocalCliReply::AgentExport { export } => Ok(export),
            _ => Err(crate::profiles::unexpected()),
        }
    }

    pub async fn segments(
        &self,
        session_id: String,
    ) -> Result<Vec<hypr_loofah_protocol::RenderedTranscriptSegment>> {
        match self
            .local_gateway()?
            .execute(LocalCliOperation::AgentSegments { session_id })
            .await?
            .response
            .result?
        {
            LocalCliReply::AgentSegments { segments } => Ok(segments),
            _ => Err(crate::profiles::unexpected()),
        }
    }

    pub async fn transcript(
        &self,
        input: access::GetMeetingTranscriptInput,
    ) -> Result<access::MeetingTranscript> {
        match self {
            Self::Local(gateway) => match gateway
                .execute(LocalCliOperation::AgentTranscript {
                    session_id: input.meeting_id,
                })
                .await?
                .response
                .result?
            {
                LocalCliReply::AgentTranscript { transcript } => Ok(transcript),
                _ => Err(crate::profiles::unexpected()),
            },
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
