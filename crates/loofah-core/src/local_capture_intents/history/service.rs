use super::*;
use tokio::sync::OwnedRwLockReadGuard;

pub(in crate::local_capture_intents) struct OriginalRecord {
    pub record: Record,
    pub directory: PathBuf,
    pub archived: bool,
    _read: OwnedRwLockReadGuard<()>,
    _context: Arc<VaultContext>,
}

impl LocalCaptureIntents {
    pub async fn history(
        &self,
        session: Option<&str>,
        cursor: Option<&str>,
        limit: u32,
    ) -> Result<LocalCaptureHistoryPage, ApplicationError> {
        if self.stop.is_cancelled() {
            return Err(review("Local capture owner is stopping"));
        }
        let read = self.history_gate.clone().read_owned().await;
        if self.stop.is_cancelled() {
            return Err(review("Local capture owner is stopping"));
        }
        let history = self.history.clone();
        let session = session.map(str::to_owned);
        let cursor = cursor.map(str::to_owned);
        let context = self.context.clone();
        tokio::task::spawn_blocking(move || {
            let (_context, _read) = (context, read);
            history.page(session.as_deref(), cursor.as_deref(), limit)
        })
        .await
        .map_err(review)?
    }

    pub(in crate::local_capture_intents) async fn read_original(
        &self,
        request: &str,
    ) -> Result<OriginalRecord, ApplicationError> {
        self.read_original_inner(request, true).await
    }
    pub(in crate::local_capture_intents) async fn read_public_original(
        &self,
        request: &str,
    ) -> Result<OriginalRecord, ApplicationError> {
        self.read_original_inner(request, false).await
    }
    async fn read_original_inner(
        &self,
        request: &str,
        completion: bool,
    ) -> Result<OriginalRecord, ApplicationError> {
        uuid(request)?;
        if !completion && self.stop.is_cancelled() {
            return Err(review("Local capture owner is stopping"));
        }
        let read = self.history_gate.clone().read_owned().await;
        if !completion && self.stop.is_cancelled() {
            return Err(review("Local capture owner is stopping"));
        }
        let active = self
            .state
            .lock()
            .await
            .get(request)
            .map(|entry| entry.record.clone());
        if completion && self.stop.is_cancelled() {
            let record = active.ok_or_else(|| review("Local capture owner is stopping"))?;
            return Ok(OriginalRecord {
                record,
                directory: self.root.join(request),
                archived: false,
                _read: read,
                _context: self.context.clone(),
            });
        }
        if let Some(record) = active.as_ref().filter(|record| !eligible(record)) {
            return Ok(OriginalRecord {
                record: record.clone(),
                directory: self.root.join(request),
                archived: false,
                _read: read,
                _context: self.context.clone(),
            });
        }
        let history = self.history.clone();
        let request = request.to_owned();
        let context = self.context.clone();
        tokio::task::spawn_blocking(move || {
            if let Some(record) = active {
                let directory = history.active.join(&request);
                if ledger::existing_directory(&directory)? {
                    if ledger::existing_directory(&history.receipt_directory(&request)?)? {
                        return Err(review(
                            "Local intent exists in both active and history storage",
                        ));
                    }
                    return Ok(OriginalRecord {
                        record,
                        directory,
                        archived: false,
                        _read: read,
                        _context: context,
                    });
                }
            }
            let located = history
                .read(&request)?
                .ok_or_else(|| error(ErrorCode::NotFound, "Local capture intent not found"))?;
            Ok(OriginalRecord {
                record: located.record,
                directory: located.directory,
                archived: true,
                _read: read,
                _context: context,
            })
        })
        .await
        .map_err(review)?
    }

    pub(in crate::local_capture_intents) async fn archive_completed(
        self: &Arc<Self>,
    ) -> Result<(), ApplicationError> {
        let service = self.clone();
        tokio::spawn(async move { service.archive_completed_owned().await })
            .await
            .map_err(review)?
    }

    async fn archive_completed_owned(&self) -> Result<(), ApplicationError> {
        if self.stop.is_cancelled() {
            return Err(review("Local capture owner is stopping"));
        }
        let _write = self.history_gate.write().await;
        if self.stop.is_cancelled() {
            return Err(review("Local capture owner is stopping"));
        }
        let candidates: Vec<_> = self
            .state
            .lock()
            .await
            .iter()
            .filter(|(_, entry)| {
                !entry.running && entry.admission.is_none() && eligible(&entry.record)
            })
            .take(ARCHIVE_BATCH)
            .map(|(request, entry)| (request.clone(), entry.dispatch.clone()))
            .collect();
        for (request, dispatch) in candidates {
            let _dispatch = dispatch.lock_owned().await;
            let Some(record) = self
                .state
                .lock()
                .await
                .get(&request)
                .filter(|entry| {
                    !entry.running && entry.admission.is_none() && eligible(&entry.record)
                })
                .map(|entry| entry.record.clone())
            else {
                continue;
            };
            let history = self.history.clone();
            let archived = record.clone();
            let context = self.context.clone();
            tokio::task::spawn_blocking(move || {
                let _context = context;
                history.archive(&archived)
            })
            .await
            .map_err(review)??;
            let mut state = self.state.lock().await;
            if state.get(&request).is_none_or(|entry| {
                entry.running
                    || entry.admission.is_some()
                    || !same_record(&entry.record, &record).unwrap_or(false)
            }) {
                return Err(review("Local intent changed during archival"));
            }
            state.remove(&request);
        }
        Ok(())
    }
}
