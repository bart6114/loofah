use super::*;
use std::path::PathBuf;

pub async fn execute_local_maintenance(
    lease: hypr_storage::vault::VaultLease,
    cache: hypr_search_cache::Cache,
    operation: LocalCliOperation,
    progress: Arc<dyn Fn(LocalCliProgress) + Send + Sync>,
) -> Result<LocalCliOutput, ApplicationError> {
    let initialize = match operation {
        LocalCliOperation::Initialize {} => true,
        LocalCliOperation::Doctor {} => false,
        _ => {
            return Err(error(
                ErrorCode::Validation,
                "Invalid local maintenance operation",
            ));
        }
    };
    let root = lease.canonical_vault().to_owned();
    run(root, Some(cache), Box::new(lease), initialize, progress).await
}

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
        run(
            self.context.store.vault_base().to_owned(),
            Some(cache),
            Box::new(self.context.clone()),
            true,
            progress,
        )
        .await
    }

    pub(super) async fn doctor(&self) -> Result<LocalCliOutput, ApplicationError> {
        run(
            self.context.store.vault_base().to_owned(),
            self.context.search.cache().await.ok(),
            Box::new(self.context.clone()),
            false,
            Arc::new(|_| {}),
        )
        .await
    }
}

async fn run(
    root: PathBuf,
    cache: Option<hypr_search_cache::Cache>,
    owner: Box<dyn Send + 'static>,
    initialize: bool,
    progress: Arc<dyn Fn(LocalCliProgress) + Send + Sync>,
) -> Result<LocalCliOutput, ApplicationError> {
    tokio::task::spawn_blocking(move || {
        let _owner = owner;
        let root = root
            .canonicalize()
            .map_err(|_| error(ErrorCode::Unavailable, "Vault is unavailable"))?;
        if let Some(cache) = &cache {
            if cache.vault().canonicalize().ok().as_ref() != Some(&root) {
                return Err(error(
                    ErrorCode::Validation,
                    "Cache and vault owner do not match",
                ));
            }
        }
        let reply = if initialize {
            let cache =
                cache.ok_or_else(|| error(ErrorCode::Unavailable, "Search is not initialized"))?;
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
            LocalCliReply::Initialized {
                cache_path: cache.path().into(),
                added: report.added,
                updated: report.updated,
                deleted: report.deleted,
                sessions: report.sessions,
            }
        } else {
            let (session_count, inspection_error) = match hypr_vault_read::discover_sessions(&root)
            {
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
            let (agents_updated, agents_error) =
                if root.join("sessions").is_dir() || root.join("config.json").is_file() {
                    match hypr_vault_write::agents_doc::ensure_agents_doc(&root) {
                        Ok(updated) => (Some(updated), None),
                        Err(cause) => (None, Some(format!("repair failed: {cause}"))),
                    }
                } else {
                    (None, None)
                };
            let status = cache.map(|cache| cache.status());
            LocalCliReply::Doctor {
                session_count,
                agents_updated,
                agents_error,
                error: inspection_error,
                cache_path: status.as_ref().map(|status| status.path.clone()),
                cache_ready: status.as_ref().is_some_and(|status| status.ready),
                cache_error: status.and_then(|status| status.error),
            }
        };
        Ok(LocalCliOutput {
            reply,
            artifact: None,
        })
    })
    .await
    .map_err(|_| error(ErrorCode::Internal, "Local maintenance worker stopped"))?
}
