use super::*;

impl LocalCli {
    pub(super) async fn initialize(
        &self,
        progress: Arc<dyn Fn(LocalCliProgress) + Send + Sync>,
    ) -> Result<LocalCliOutput, ApplicationError> {
        let cache = self
            .context
            .search
            .cache()
            .await
            .map_err(|_| error(ErrorCode::Unavailable, "Search is not initialized"))?;
        let context = self.context.clone();
        tokio::task::spawn_blocking(move || {
            let _context = context;
            let report = cache
                .initialize(&mut |event| {
                    progress(LocalCliProgress {
                        schema_version: event.schema_version.into(),
                        phase: event.phase.into(),
                        processed: event.processed,
                        total: event.total,
                        added: event.added,
                        updated: event.updated,
                        deleted: event.deleted,
                        elapsed_ms: event.elapsed_ms,
                    })
                })
                .map_err(|_| error(ErrorCode::Internal, "Search cache initialization failed"))?;
            Ok(LocalCliOutput {
                reply: LocalCliReply::Initialized {
                    cache_path: cache.path().into(),
                    added: report.added,
                    updated: report.updated,
                    deleted: report.deleted,
                    sessions: report.sessions,
                },
                artifact: None,
            })
        })
        .await
        .map_err(|_| error(ErrorCode::Internal, "Search initialization worker stopped"))?
    }

    pub(super) async fn doctor(&self) -> Result<LocalCliOutput, ApplicationError> {
        let context = self.context.clone();
        let cache = context.search.cache().await.ok();
        tokio::task::spawn_blocking(move || {
            let root = context.store.vault_base();
            let (session_count, mut inspection_error) =
                match hypr_vault_read::discover_sessions(root) {
                    Ok(scan) => (
                        Some(scan.sessions.len()),
                        (!scan.errors.is_empty()).then(|| {
                            scan.errors
                                .iter()
                                .map(ToString::to_string)
                                .collect::<Vec<_>>()
                                .join("; ")
                        }),
                    ),
                    Err(cause) => (None, Some(format!("vault scan failed: {cause}"))),
                };
            let agents_updated = if root.join("sessions").is_dir()
                || root.join("config.json").is_file()
            {
                match hypr_vault_write::agents_doc::ensure_agents_doc(root) {
                    Ok(updated) => Some(updated),
                    Err(cause) => {
                        inspection_error.get_or_insert_with(|| format!("repair failed: {cause}"));
                        None
                    }
                }
            } else {
                None
            };
            let status = cache.map(|cache| cache.status());
            Ok(LocalCliOutput {
                reply: LocalCliReply::Doctor {
                    session_count,
                    agents_updated,
                    error: inspection_error,
                    cache_path: status.as_ref().map(|status| status.path.clone()),
                    cache_ready: status.as_ref().is_some_and(|status| status.ready),
                    cache_error: status.and_then(|status| status.error),
                },
                artifact: None,
            })
        })
        .await
        .map_err(|_| error(ErrorCode::Internal, "Vault diagnostic worker stopped"))?
    }
}
