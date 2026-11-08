use crate::{
    error::{ApiError, Result},
    service::Service,
    transcription_authorization::CatalogTranscriptionAuthorizer,
};
use hypr_loofah_core::{
    local_cli::{MediaAuthorization, MediaDispatcher, MediaLimits},
    transcription_jobs::{TranscriptionAccess, TranscriptionFuture},
};
use hypr_loofah_protocol::{ApplicationError, ErrorCode, TranscriptionFailure};
use std::sync::Arc;

struct Authorization {
    member: String,
    inner: CatalogTranscriptionAuthorizer,
}
impl MediaAuthorization for Authorization {
    fn authorize(
        &self,
    ) -> TranscriptionFuture<'_, std::result::Result<Box<dyn Send>, ApplicationError>> {
        Box::pin(async move {
            self.inner
                .authorize_member(&self.member)
                .await
                .map_err(failure)
        })
    }
    fn observe(
        &self,
    ) -> TranscriptionFuture<'_, std::result::Result<TranscriptionAccess, ApplicationError>> {
        Box::pin(async move {
            self.inner
                .observe_member(&self.member)
                .await
                .map_err(failure)
        })
    }
}
fn failure(cause: TranscriptionFailure) -> ApplicationError {
    ApiError::new(
        if cause == TranscriptionFailure::Interrupted {
            ErrorCode::Unavailable
        } else {
            ErrorCode::Forbidden
        },
        "Media authority is unavailable",
    )
    .0
}
impl Service {
    pub(crate) async fn media_dispatcher(
        &self,
        vault: &str,
        member: &str,
    ) -> Result<Arc<MediaDispatcher>> {
        let context = self.vault(vault).await?;
        let mut dispatchers = self.media_dispatchers.lock().await;
        if self.stop.is_cancelled() {
            return Err(ApiError::new(
                ErrorCode::Unavailable,
                "Media service is stopping",
            ));
        }
        let key = (vault.to_owned(), member.to_owned());
        if let Some(dispatcher) = dispatchers.get(&key) {
            return Ok(dispatcher.clone());
        }
        if dispatchers.len() >= 4096 {
            return Err(ApiError::busy("Media service admission capacity exhausted"));
        }
        let dispatcher = MediaDispatcher::new(
            context,
            member.into(),
            Arc::new(Authorization {
                member: member.into(),
                inner: CatalogTranscriptionAuthorizer::new(
                    self.catalog.clone(),
                    vault.into(),
                    self.stop.clone(),
                ),
            }),
            MediaLimits::default(),
        )?;
        dispatchers.insert(key, dispatcher.clone());
        Ok(dispatcher)
    }
}
