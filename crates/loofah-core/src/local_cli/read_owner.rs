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
        tokio::spawn(async move {
            let context = tokio::task::spawn_blocking(move || {
                let root = lease.canonical_vault().to_owned();
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
                    Arc::new(crate::search::SearchService::default()),
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
                read_only: true,
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
