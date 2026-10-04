use super::index::{SessionEntry, SessionTranscriptMetadata, TranscriptSummary};
use super::{RebuildReport, SessionStore, StoreError};
use std::sync::{Arc, Mutex};

impl SessionStore {
    pub fn attach_search_cache(&self, cache: Arc<Mutex<hypr_search_cache::Cache>>) {
        let guard = cache.lock().unwrap();
        let mut index = self.index.write().unwrap();
        let mut cached = self.cached_sessions.lock().unwrap();
        for (id, header) in guard.headers() {
            index.cached_headers.insert(id.clone(), header.clone());
            if !header.tasks.is_empty() {
                index.tasks.insert(id.clone(), header.tasks.clone());
            }
            index
                .sessions
                .entry(id.clone())
                .or_insert_with(|| SessionEntry {
                    meta: header.meta.clone(),
                    note_markdown: None,
                });
            if !header.transcript_ids.is_empty() {
                index
                    .transcripts
                    .entry(id.clone())
                    .or_insert_with(|| TranscriptSummary {
                        metadata: SessionTranscriptMetadata::default(),
                        transcript_ids: header.transcript_ids.clone(),
                        has_words: header.has_words,
                        word_count: header.word_count,
                        content_hash: 0,
                    });
            }
            cached.insert(id.clone());
        }
        drop(guard);
        *self.search_cache.write().unwrap() = Some(cache);
    }

    pub async fn ensure_session_loaded(&self, id: &str) -> Result<(), StoreError> {
        if self.cached_sessions.lock().unwrap().contains(id) {
            if self.read_meta(id).await?.is_none() {
                return Err(StoreError::Io(format!(
                    "{id}: canonical session is not available yet"
                )));
            }
            self.refresh_session(id).await?;
            self.cached_sessions.lock().unwrap().remove(id);
            self.index.write().unwrap().cached_headers.remove(id);
        }
        Ok(())
    }

    pub async fn reconcile_incremental(&self) -> Result<RebuildReport, StoreError> {
        self.reconcile_search_cache(false).await
    }

    pub async fn verify_index(&self) -> Result<RebuildReport, StoreError> {
        self.reconcile_search_cache(true).await
    }

    async fn reconcile_search_cache(&self, full: bool) -> Result<RebuildReport, StoreError> {
        let cache = self.search_cache.read().unwrap().clone();
        let Some(cache) = cache else {
            return self.rebuild_index().await;
        };
        let report = tokio::task::spawn_blocking(move || {
            let mut cache = cache.lock().unwrap();
            if full {
                cache.refresh(true)
            } else {
                cache.maintain(16, false)
            }
        })
        .await
        .map_err(|e| StoreError::Io(e.to_string()))?
        .map_err(|e| StoreError::Io(e.to_string()))?;
        let mut result = RebuildReport {
            errors: report.errors,
            ..Default::default()
        };
        let mut work: std::collections::BTreeSet<_> =
            report.updated.into_iter().chain(report.removed).collect();
        work.extend(self.pending_cache_refresh.lock().unwrap().iter().cloned());
        for id in work {
            match self.refresh_session(&id).await {
                Ok(()) => {
                    result.sessions += 1;
                    self.pending_cache_refresh.lock().unwrap().remove(&id);
                    self.cached_sessions.lock().unwrap().remove(&id);
                    self.index.write().unwrap().cached_headers.remove(&id);
                }
                Err(e) => {
                    result.errors.push(e.to_string());
                    self.pending_cache_refresh.lock().unwrap().insert(id);
                }
            }
        }
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn cached_headers_are_ready_before_canonical_details_and_writes_repair_search() {
        let vault = tempfile::tempdir().unwrap();
        let local = tempfile::tempdir().unwrap();
        let meta: super::super::SessionMeta = serde_json::from_value(serde_json::json!({"id":"s1","title":"Before","created_at":"2026-01-01T00:00:00Z","tags":[]})).unwrap();
        let writer = SessionStore::new(vault.path().into());
        writer.write_meta(&meta).await.unwrap();
        writer.write_note("s1", "first note").await.unwrap();
        let mut cache = hypr_search_cache::Cache::open_at(vault.path(), local.path()).unwrap();
        cache.refresh(false).unwrap();
        let cache = Arc::new(Mutex::new(cache));
        let reader = SessionStore::new(vault.path().into());
        reader.attach_search_cache(cache.clone());
        assert_eq!(reader.session_list_headers()[0].title, "Before");
        assert!(reader.session_get("s1").unwrap().note_markdown.is_none());
        reader.ensure_session_loaded("s1").await.unwrap();
        assert_eq!(
            reader.session_get("s1").unwrap().note_markdown.as_deref(),
            Some("first note")
        );
        reader.write_note("s1", "changed note").await.unwrap();
        cache.lock().unwrap().mark("s1").unwrap();
        reader.reconcile_incremental().await.unwrap();
        assert_eq!(
            cache
                .lock()
                .unwrap()
                .session("s1")
                .unwrap()
                .unwrap()
                .note
                .as_deref(),
            Some("changed note")
        );
        std::fs::remove_file(local.path().join("maintenance.json")).unwrap();
        std::fs::create_dir(local.path().join("maintenance.json")).unwrap();
        reader
            .write_note("s1", "canonical write survives")
            .await
            .unwrap();
        assert!(reader.reconcile_incremental().await.is_err());
        assert_eq!(
            reader.read_note("s1").await.unwrap().as_deref(),
            Some("canonical write survives")
        );
    }
}
