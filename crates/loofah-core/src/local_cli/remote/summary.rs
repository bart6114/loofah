use super::*;
use crate::manual_media_intents::{
    ManualMediaKind, ManualMediaSummaryDeclaration, ManualMediaSummaryStatus,
};

impl MediaDispatcher {
    pub(super) async fn declare_summary(
        &self,
        input: &MediaDeclaration,
    ) -> Result<(), ApplicationError> {
        let cli = self.cli(input, self.stop.child_token())?;
        let fresh = input.operation.session_id().is_none();
        let session = match input.operation.session_id() {
            Some(session) => session.into(),
            None => self
                .context
                .created_session_id(&cli.media_namespace("local-cli-import"), &input.request_id)?,
        };
        let (kind, expected_created_at) = match &input.operation {
            MediaOperation::ImportAudio {
                created_at,
                session_id,
                ..
            } => (
                ManualMediaKind::AudioImport,
                session_id.as_ref().and(normalized_date(created_at)?),
            ),
            MediaOperation::ImportSubtitles { .. } => (ManualMediaKind::Subtitles, None),
            MediaOperation::Transcribe { .. } => (ManualMediaKind::Transcription, None),
        };
        let only_if_empty = kind != ManualMediaKind::Subtitles;
        let _authorization = self.authorization.authorize().await?;
        self.context
            .declare_remote_media_summary(
                self.principal.clone(),
                ManualMediaSummaryDeclaration {
                    request_id: input.request_id.clone(),
                    session_id: session,
                    fresh_session: fresh,
                    expected_created_at,
                    kind,
                    summary: input.summary.clone(),
                    only_if_empty,
                },
            )
            .await?;
        Ok(())
    }
    pub(super) async fn complete_summary(
        &self,
        request: &str,
    ) -> Result<ManualMediaSummaryStatus, ApplicationError> {
        let _authorization = self.authorization.authorize().await?;
        self.context
            .complete_remote_media_summary(self.principal.clone(), request.into())
            .await
    }
    pub async fn summary_status(
        &self,
        request: &str,
    ) -> Result<ManualMediaSummaryStatus, ApplicationError> {
        let _authorization = self.authorization.authorize().await?;
        self.load(request)
            .await?
            .ok_or_else(|| error(ErrorCode::NotFound, "Media request not found"))?;
        self.context
            .remote_media_summary_status(&self.principal, request)
            .await
    }
}
