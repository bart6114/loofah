use super::*;
use sha2::{Digest, Sha256};
use std::{future::Future, path::PathBuf, pin::Pin};

pub trait MediaAuthorizer: Send + Sync {
    fn observe(
        &self,
    ) -> Pin<
        Box<
            dyn Future<
                    Output = Result<
                        crate::transcription_jobs::TranscriptionAccess,
                        ApplicationError,
                    >,
                > + Send
                + '_,
        >,
    >;
    fn authorize(
        &self,
    ) -> Pin<Box<dyn Future<Output = Result<Box<dyn Send>, ApplicationError>> + Send + '_>>;
}

pub(crate) struct MediaExecutionScope {
    pub principal: String,
    pub config: crate::settings::AppConfig,
    pub authorizer: Arc<dyn MediaAuthorizer>,
    namespace: String,
    pub baseline: Option<MediaSessionSnapshot>,
}

impl MediaExecutionScope {
    pub(crate) fn new(
        vault: &str,
        principal: String,
        config: crate::settings::AppConfig,
        authorizer: Arc<dyn MediaAuthorizer>,
    ) -> Result<Self, ApplicationError> {
        if principal.is_empty() || principal.len() > 512 || principal.chars().any(char::is_control)
        {
            return Err(error(ErrorCode::Validation, "Invalid media principal"));
        }
        let namespace = Sha256::digest(serde_json::to_vec(&(vault, &principal)).unwrap())
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        Ok(Self {
            principal,
            config,
            authorizer,
            namespace,
            baseline: None,
        })
    }
}

impl LocalCli {
    pub(crate) fn with_media_scope(mut self, scope: Arc<MediaExecutionScope>) -> Self {
        self.media_scope = Some(scope);
        self
    }
    pub(super) fn media_namespace(&self, local: &str) -> String {
        match &self.media_scope {
            Some(scope) => format!("remote-media:{}:{local}", scope.namespace),
            None => local.into(),
        }
    }
    pub(super) fn media_directory(&self, local: &str) -> PathBuf {
        match &self.media_scope {
            Some(scope) => PathBuf::from("member-media")
                .join(&scope.namespace)
                .join(local),
            None => PathBuf::from(local),
        }
    }
    pub(super) fn media_principal(&self) -> &str {
        self.media_scope
            .as_ref()
            .map_or("local-cli", |scope| &scope.principal)
    }
    pub(super) async fn authorize_media(&self) -> Result<Box<dyn Send>, ApplicationError> {
        match &self.media_scope {
            Some(scope) => scope.authorizer.authorize().await,
            None => Ok(Box::new(())),
        }
    }
    pub(super) async fn media_config(
        &self,
    ) -> Result<crate::settings::AppConfig, ApplicationError> {
        match &self.media_scope {
            Some(scope) => Ok(scope.config.clone()),
            None => self.context.settings.read_current().await.map_err(|_| {
                error(
                    ErrorCode::Unavailable,
                    "Cannot freeze Local transcription settings",
                )
            }),
        }
    }
}
