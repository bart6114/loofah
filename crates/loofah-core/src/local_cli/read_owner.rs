use super::*;

pub(super) fn operations() -> Vec<LocalCliOperationKind> {
    use LocalCliOperationKind::*;
    vec![
        AgentMeeting,
        AgentTranscript,
        AgentExport,
        AgentSegments,
        Metadata,
        Note,
        SessionPath,
        Attachment,
        AttachmentPage,
    ]
}

impl LocalCli {
    /// Per-ID reads recover their owned canonical files without scanning unrelated sessions.
    pub async fn open_read_owner(
        lease: hypr_storage::vault::VaultLease,
        operation_root: std::path::PathBuf,
    ) -> Result<Arc<Self>, ApplicationError> {
        Self::open_restricted_owner(lease, operation_root, None, false).await
    }

    pub async fn open_session_owner(
        lease: hypr_storage::vault::VaultLease,
        operation_root: std::path::PathBuf,
        cache: Option<hypr_search_cache::Cache>,
    ) -> Result<Arc<Self>, ApplicationError> {
        Self::open_restricted_owner(lease, operation_root, cache, true).await
    }

    async fn open_restricted_owner(
        lease: hypr_storage::vault::VaultLease,
        operation_root: std::path::PathBuf,
        cache: Option<hypr_search_cache::Cache>,
        session_only: bool,
    ) -> Result<Arc<Self>, ApplicationError> {
        tokio::spawn(async move {
            let context = tokio::task::spawn_blocking(move || {
                let root = lease.canonical_vault().to_owned();
                if let Some(cache) = &cache {
                    if cache.vault().canonicalize().ok().as_ref() != Some(&root) {
                        return Err(error(
                            ErrorCode::Validation,
                            "Cache and owner vault do not match",
                        ));
                    }
                    cache
                        .check()
                        .map_err(|cause| error(ErrorCode::Unavailable, &cause.to_string()))?;
                }
                std::fs::create_dir_all(&operation_root).map_err(|_| {
                    error(
                        ErrorCode::Unavailable,
                        "Local operation storage is unavailable",
                    )
                })?;
                let operation_root = operation_root.canonicalize().map_err(|_| {
                    error(
                        ErrorCode::Unavailable,
                        "Local operation storage is unavailable",
                    )
                })?;
                let store = Arc::new(hypr_vault_write::SessionStore::with_content_limits(
                    root.clone(),
                    hypr_vault_write::ContentLimits::unlimited(),
                ));
                store.set_startup_pending(true);
                let settings = crate::settings::ConfigState::from_config(
                    hypr_storage::vault::compute_config_path(&root),
                    crate::settings::AppConfig::default(),
                    Some(lease.pin()),
                );
                let context = VaultContext::new(
                    format!("local:{}", hypr_storage::vault::canonical_vault_key(&root)),
                    store,
                    crate::startup::StartupState::new(Some(&root)),
                    Arc::new(settings),
                    Arc::new(
                        cache
                            .map(crate::search::SearchService::with_ready_cache)
                            .unwrap_or_default(),
                    ),
                )
                .with_vault_lease(lease)?
                .with_operation_storage(operation_root);
                Ok::<_, ApplicationError>(Arc::new(context))
            })
            .await
            .map_err(|_| {
                error(
                    ErrorCode::Internal,
                    "Local read owner initialization stopped",
                )
            })??;
            context.recover_recording_publications().await?;
            context.store.set_startup_pending(false);
            context.startup.set_phase(StartupPhase::Ready);
            Ok(Arc::new(Self {
                context,
                read_only: !session_only,
                session_only,
                media: None,
            }))
        })
        .await
        .map_err(|_| {
            error(
                ErrorCode::Internal,
                "Local read owner initialization stopped",
            )
        })?
    }
}
