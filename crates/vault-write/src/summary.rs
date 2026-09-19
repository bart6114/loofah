use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::{IndexEntity, SessionStore, StoreError, WriteGuard, paths};

#[derive(Serialize, Deserialize)]
struct PendingMigration {
    legacy_id: String,
    original: String,
    markdown: String,
}

impl SessionStore {
    pub fn summary_get(&self, session_id: &str) -> Option<String> {
        self.index
            .read()
            .unwrap()
            .summaries
            .get(session_id)
            .cloned()
    }

    pub async fn read_summary(&self, session_id: &str) -> Result<Option<String>, StoreError> {
        let dir = self.session_dir(session_id).await?;
        let vault = self.vault_base.clone();
        let id = session_id.to_owned();
        tokio::task::spawn_blocking(move || {
            hypr_vault_read::summary::locate_in(&vault, &dir, &id)
                .map(|summary| summary.map(|summary| summary.markdown))
                .map_err(Into::into)
        })
        .await
        .map_err(join_error)?
    }

    pub async fn ensure_summary(&self, session_id: &str) -> Result<String, StoreError> {
        let guard = self.lock_writes().await;
        let dir = self.writable_summary_dir(&guard, session_id).await?;
        self.try_migrate_summary_locked(&guard, session_id, &dir)
            .await?;
        let markdown = match self.read_summary(session_id).await? {
            Some(markdown) => markdown,
            None => {
                self.publish_summary_locked(&guard, &dir, "").await?;
                String::new()
            }
        };
        self.index_set_summary(session_id, Some(markdown.clone()));
        Ok(markdown)
    }

    pub async fn update_summary(
        &self,
        session_id: &str,
        markdown: &str,
        expected: Option<&str>,
    ) -> Result<(), StoreError> {
        self.update_summary_impl(session_id, markdown, expected, false)
            .await
    }

    pub async fn update_generated_summary(
        &self,
        session_id: &str,
        markdown: &str,
        expected: Option<&str>,
    ) -> Result<(), StoreError> {
        self.update_summary_impl(session_id, markdown, expected, true)
            .await
    }

    async fn update_summary_impl(
        &self,
        session_id: &str,
        markdown: &str,
        expected: Option<&str>,
        reconcile_tasks: bool,
    ) -> Result<(), StoreError> {
        let task_content = if reconcile_tasks {
            let markdown = markdown.to_owned();
            Some(
                tokio::task::spawn_blocking(move || hypr_tiptap::md_to_tiptap_json(&markdown))
                    .await
                    .map_err(join_error)?
                    .map_err(StoreError::Serialize)?,
            )
        } else {
            None
        };
        let guard = self.lock_writes().await;
        let dir = self.writable_summary_dir(&guard, session_id).await?;
        self.try_migrate_summary_locked(&guard, session_id, &dir)
            .await?;
        let vault = self.vault_base.clone();
        let read_dir = dir.clone();
        let id = session_id.to_owned();
        let summary = tokio::task::spawn_blocking(move || {
            hypr_vault_read::summary::locate_in(&vault, &read_dir, &id)
        })
        .await
        .map_err(join_error)??
        .ok_or_else(|| StoreError::Conflict("summary was deleted".into()))?;
        if expected.is_some_and(|expected| expected != summary.markdown)
            && !(reconcile_tasks && markdown == summary.markdown)
        {
            return Err(StoreError::Conflict(
                "summary changed since it was read".into(),
            ));
        }
        let tasks = if let Some(content) = task_content {
            Some(
                self.prepare_generated_tasks(session_id, "session_summary", session_id, &content)
                    .await?,
            )
        } else {
            None
        };
        let bytes = if let Some(legacy_id) = summary.legacy_id {
            let mut doc = self
                .read_enhanced_doc(session_id, &legacy_id)
                .await?
                .ok_or_else(|| StoreError::Conflict("legacy summary was deleted".into()))?;
            doc.markdown = markdown.to_owned();
            hypr_vault_read::render_enhanced_file(&doc)?.into_bytes()
        } else {
            markdown.as_bytes().to_vec()
        };
        self.write_file_locked(&guard, summary.relative_path, bytes)
            .await?;
        let task_result = if let Some(tasks) = tasks {
            self.persist_generated_tasks(&guard, session_id, &tasks)
                .await
                .map_err(|error| {
                    StoreError::Io(format!(
                        "Summary saved but task synchronization failed; retry to finish: {error}"
                    ))
                })
        } else {
            Ok(())
        };
        self.index_set_summary(session_id, Some(markdown.to_owned()));
        task_result
    }

    pub async fn delete_summary(&self, session_id: &str) -> Result<(), StoreError> {
        let guard = self.lock_writes().await;
        let dir = self.writable_summary_dir(&guard, session_id).await?;
        self.migrate_summary_locked(&guard, session_id, &dir)
            .await?;
        let vault = self.vault_base.clone();
        let id = session_id.to_owned();
        tokio::task::spawn_blocking(move || -> Result<(), StoreError> {
            if let Some(summary) = hypr_vault_read::summary::locate_in(&vault, &dir, &id)? {
                hypr_fs_sync_core::export::move_to_trash(
                    &vault,
                    &vault.join(summary.relative_path),
                )
                .map_err(|e| StoreError::Io(e.to_string()))?;
            }
            Ok(())
        })
        .await
        .map_err(join_error)??;
        self.index_set_summary(session_id, None);
        Ok(())
    }

    async fn writable_summary_dir(
        &self,
        guard: &WriteGuard<'_>,
        id: &str,
    ) -> Result<PathBuf, StoreError> {
        let dir = self.session_dir_locked(guard, id).await?;
        if self.read_meta(id).await?.is_none() {
            return Err(StoreError::Conflict(format!("session {id} was deleted")));
        }
        Ok(dir)
    }

    fn index_set_summary(&self, id: &str, markdown: Option<String>) {
        let changed = {
            let mut index = self.index.write().unwrap();
            if let Some(docs) = index.docs.get_mut(id) {
                docs.retain(|doc| doc.kind != "summary");
            }
            match markdown {
                Some(markdown) if index.summaries.get(id) != Some(&markdown) => {
                    index.summaries.insert(id.to_owned(), markdown);
                    true
                }
                None => index.summaries.remove(id).is_some(),
                _ => false,
            }
        };
        if changed {
            self.notify_index_changed(IndexEntity::Docs, vec![id.to_owned()]);
        }
    }

    async fn publish_summary_locked(
        &self,
        guard: &WriteGuard<'_>,
        dir: &Path,
        markdown: &str,
    ) -> Result<(), StoreError> {
        let staging = dir.join(".summary-publish.tmp");
        self.write_file_locked(guard, staging.clone(), markdown.as_bytes().to_vec())
            .await?;
        let destination = paths::summary_path_in(dir);
        let vault = self.vault_base.clone();
        let target = destination.clone();
        let result = tokio::task::spawn_blocking(move || {
            let source = vault.join(staging);
            let result = hypr_storage::fs::rename_no_replace(&source, &vault.join(target));
            if result.is_err() {
                let _ = std::fs::remove_file(source);
            }
            result
        })
        .await
        .map_err(join_error)?;
        result.map_err(|e| {
            if e.kind() == std::io::ErrorKind::AlreadyExists {
                StoreError::Conflict(
                    "summary.md already exists; move the conflicting file and restart".into(),
                )
            } else {
                StoreError::Io(e.to_string())
            }
        })?;
        self.journal.record(
            &destination.to_string_lossy(),
            &super::sha256(markdown.as_bytes()),
        );
        Ok(())
    }

    async fn try_migrate_summary_locked(
        &self,
        guard: &WriteGuard<'_>,
        id: &str,
        dir: &Path,
    ) -> Result<(), StoreError> {
        match self.migrate_summary_locked(guard, id, dir).await {
            // A filename collision leaves the old summary writable, but an interrupted
            // migration must finish before any editor can change either copy.
            Err(StoreError::Conflict(reason))
                if !self
                    .vault_base
                    .join(dir)
                    .join(".summary-migration.json")
                    .exists() =>
            {
                tracing::warn!(%id, %reason, "summary migration deferred");
                let docs = hypr_vault_read::enhanced::list_legacy_enhanced_docs_in(
                    &self.vault_base,
                    dir,
                    id,
                )?;
                if docs.iter().filter(|doc| doc.kind == "summary").count() == 1 {
                    Ok(())
                } else {
                    Err(StoreError::Conflict(reason))
                }
            }
            result => result,
        }
    }

    pub async fn migrate_summary(&self, id: &str) -> Result<(), StoreError> {
        let guard = self.lock_writes().await;
        let dir = self.writable_summary_dir(&guard, id).await?;
        self.migrate_summary_locked(&guard, id, &dir).await
    }

    async fn migrate_summary_locked(
        &self,
        guard: &WriteGuard<'_>,
        id: &str,
        dir: &Path,
    ) -> Result<(), StoreError> {
        let marker = dir.join(".summary-migration.json");
        let vault = self.vault_base.clone();
        let scan_dir = dir.to_owned();
        let session_id = id.to_owned();
        let marker_path = vault.join(&marker);
        let pending =
            tokio::task::spawn_blocking(move || -> Result<Option<PendingMigration>, StoreError> {
                match std::fs::read(&marker_path) {
                    Ok(bytes) => {
                        return serde_json::from_slice(&bytes)
                            .map(Some)
                            .map_err(|e| StoreError::Serialize(e.to_string()));
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                    Err(e) => return Err(StoreError::Io(e.to_string())),
                }
                let mut summaries = Vec::new();
                for (_, parsed) in
                    hypr_vault_read::enhanced::scan_legacy_docs_in(&vault, &scan_dir, &session_id)?
                {
                    let doc = parsed?;
                    if doc.kind == "summary" {
                        summaries.push(doc);
                    }
                }
                if summaries.is_empty() {
                    return Ok(None);
                }
                if summaries.len() != 1 {
                    return Err(StoreError::Conflict(
                        "multiple legacy summaries; files left unchanged".into(),
                    ));
                }
                match std::fs::symlink_metadata(vault.join(paths::summary_path_in(&scan_dir))) {
                    Ok(_) => {
                        return Err(StoreError::Conflict(
                            "summary.md already exists; move the conflicting file and restart"
                                .into(),
                        ));
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                    Err(e) => return Err(StoreError::Io(e.to_string())),
                }
                let doc = summaries.remove(0);
                let original = std::fs::read_to_string(
                    vault.join(paths::enhanced_doc_path_in(&scan_dir, &doc.id)),
                )
                .map_err(|e| StoreError::Io(e.to_string()))?;
                let current =
                    hypr_vault_read::parse_enhanced_file(&doc.id, &session_id, &original)?;
                if current.kind != "summary" {
                    return Err(StoreError::Conflict(
                        "legacy document changed during migration".into(),
                    ));
                }
                // Validate tasks before publishing anything.
                hypr_vault_read::tasks::read_session_tasks_in(&vault, &scan_dir)?;
                Ok(Some(PendingMigration {
                    legacy_id: doc.id,
                    original,
                    markdown: current.markdown,
                }))
            })
            .await
            .map_err(join_error)??;
        let Some(pending) = pending else {
            return Ok(());
        };
        super::validate_doc_id(&pending.legacy_id)?;
        let source = paths::enhanced_doc_path_in(dir, &pending.legacy_id);
        let vault = self.vault_base.clone();
        let source_abs = vault.join(&source);
        let original = pending.original.clone();
        let can_resume_without_source =
            marker_path_for_resume(&self.vault_base, &marker, dir, &pending.markdown)?;
        tokio::task::spawn_blocking(move || -> Result<(), StoreError> {
            match std::fs::read_to_string(source_abs) {
                Ok(raw) if raw != original => Err(StoreError::Conflict(
                    "legacy summary changed during migration".into(),
                )),
                Ok(_) => Ok(()),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound && can_resume_without_source => {
                    Ok(())
                }
                Err(e) => Err(StoreError::Io(e.to_string())),
            }
        })
        .await
        .map_err(join_error)??;
        let marker_bytes =
            serde_json::to_vec(&pending).map_err(|e| StoreError::Serialize(e.to_string()))?;
        self.write_file_locked(guard, marker.clone(), marker_bytes)
            .await?;
        let vault = self.vault_base.clone();
        let read_dir = dir.to_owned();
        let current = tokio::task::spawn_blocking(move || {
            hypr_vault_read::summary::read_canonical_in(&vault, &read_dir)
        })
        .await
        .map_err(join_error)??;
        match current {
            None => {
                self.publish_summary_locked(guard, dir, &pending.markdown)
                    .await?
            }
            Some(current) if current == pending.markdown => {}
            Some(_) => {
                return Err(StoreError::Conflict(
                    "summary.md changed during migration; files retained".into(),
                ));
            }
        }
        self.remap_summary_tasks_locked(guard, id, Some(&pending.legacy_id))
            .await?;
        let vault = self.vault_base.clone();
        let original = pending.original.clone();
        tokio::task::spawn_blocking(move || -> Result<(), StoreError> {
            match std::fs::read_to_string(vault.join(&source)) {
                Ok(raw) if raw != original => {
                    return Err(StoreError::Conflict(
                        "legacy summary changed during migration; both copies retained".into(),
                    ));
                }
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
                    return Err(StoreError::Io(e.to_string()));
                }
                _ => {}
            }
            hypr_fs_sync_core::export::move_to_trash(&vault, &vault.join(source))
                .map_err(|e| StoreError::Io(e.to_string()))?;
            std::fs::remove_file(vault.join(marker)).map_err(|e| StoreError::Io(e.to_string()))?;
            Ok(())
        })
        .await
        .map_err(join_error)??;
        self.index_remove_doc(id, &pending.legacy_id);
        self.index_set_summary(id, Some(pending.markdown));
        Ok(())
    }
}

fn join_error(error: tokio::task::JoinError) -> StoreError {
    StoreError::Io(error.to_string())
}

fn marker_path_for_resume(
    vault: &Path,
    marker: &Path,
    dir: &Path,
    markdown: &str,
) -> Result<bool, StoreError> {
    Ok(vault.join(marker).is_file()
        && hypr_vault_read::summary::read_canonical_in(vault, dir)?.as_deref() == Some(markdown))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{EnhancedDoc, SessionMeta, TaskInput};

    async fn setup() -> (tempfile::TempDir, SessionStore) {
        let vault = tempfile::tempdir().unwrap();
        let store = SessionStore::new(vault.path().to_owned());
        let meta: SessionMeta = serde_json::from_value(serde_json::json!({
            "id": "s1", "title": "Meeting", "created_at": "2026-09-18T12:00:00Z", "tags": []
        }))
        .unwrap();
        store.write_meta(&meta).await.unwrap();
        (vault, store)
    }

    async fn legacy(store: &SessionStore, markdown: &str) {
        store
            .write_enhanced_doc(&EnhancedDoc {
                id: "old-summary".into(),
                session_id: "s1".into(),
                kind: "summary".into(),
                title: "Old title".into(),
                template_id: String::new(),
                sort_order: 1,
                markdown: markdown.into(),
            })
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn generated_summary_tasks_survive_migration_regeneration_and_restart() {
        for layout in ["canonical", "legacy", "collision"] {
            let (vault, store) = setup().await;
            let original = "# Actions\n- [ ] Keep action";
            if layout == "canonical" {
                store.ensure_summary("s1").await.unwrap();
                store
                    .update_summary("s1", original, Some(""))
                    .await
                    .unwrap();
            } else {
                legacy(&store, original).await;
                if layout == "collision" {
                    std::fs::write(
                        vault.path().join("sessions/s1/summary.md"),
                        "user attachment",
                    )
                    .unwrap();
                }
            }
            let (source_type, source_id) = if layout == "canonical" {
                ("session_summary", "s1")
            } else {
                ("enhanced_note", "old-summary")
            };
            store
                .replace_tasks(
                    source_type,
                    source_id,
                    vec![TaskInput {
                        id: "keep-id".into(),
                        source_order: 0,
                        status: "done".into(),
                        text: "Keep action".into(),
                        body: serde_json::json!([]),
                        due_at: "2026-10-01".into(),
                    }],
                )
                .await
                .unwrap();
            let before = store
                .list_tasks(source_type, source_id)
                .await
                .unwrap()
                .remove(0);
            store
                .replace_tasks(
                    "session_raw_note",
                    "s1",
                    vec![TaskInput {
                        id: "raw-id".into(),
                        source_order: 0,
                        status: "todo".into(),
                        text: "Raw task".into(),
                        body: serde_json::json!([]),
                        due_at: String::new(),
                    }],
                )
                .await
                .unwrap();
            let generated =
                "# Actions\n- Bob sends the invoice\n- [ ] Keep action\n- [ ] New action";
            store
                .update_generated_summary("s1", generated, Some(original))
                .await
                .unwrap();
            let tasks = store.list_tasks("session_summary", "s1").await.unwrap();
            assert_eq!(tasks.len(), 2, "{layout}");
            assert_eq!(tasks[0].id, before.id);
            assert_eq!(tasks[0].status, "done");
            assert_eq!(tasks[0].due_at, before.due_at);
            assert_eq!(tasks[0].created_at, before.created_at);
            assert_eq!(tasks[1].status, "todo");
            store
                .update_generated_summary("s1", generated, Some(original))
                .await
                .unwrap();
            assert_eq!(
                store.list_tasks("session_summary", "s1").await.unwrap(),
                tasks
            );
            assert!(matches!(
                store
                    .update_generated_summary("s1", "stale", Some(original))
                    .await,
                Err(StoreError::Conflict(_))
            ));
            let restarted = SessionStore::new(vault.path().to_owned());
            restarted.rebuild_index().await.unwrap();
            assert_eq!(
                restarted.read_summary("s1").await.unwrap().as_deref(),
                Some(generated)
            );
            assert_eq!(
                restarted.list_tasks("session_summary", "s1").await.unwrap(),
                tasks
            );
            let path = vault.path().join("sessions/s1/summary.md");
            assert_eq!(
                std::fs::read_to_string(path).unwrap(),
                if layout == "collision" {
                    "user attachment"
                } else {
                    generated
                }
            );
            restarted
                .update_generated_summary(
                    "s1",
                    "# Discussion\nNo personal actions.",
                    Some(generated),
                )
                .await
                .unwrap();
            assert!(
                restarted
                    .list_tasks("session_summary", "s1")
                    .await
                    .unwrap()
                    .is_empty()
            );
            assert_eq!(
                restarted
                    .list_tasks("session_raw_note", "s1")
                    .await
                    .unwrap()
                    .len(),
                1
            );
        }
    }

    #[tokio::test]
    async fn corrupt_tasks_do_not_replace_generated_summary() {
        let (vault, store) = setup().await;
        store.ensure_summary("s1").await.unwrap();
        store.update_summary("s1", "original", None).await.unwrap();
        std::fs::write(vault.path().join("sessions/s1/tasks.json"), "invalid").unwrap();
        assert!(
            store
                .update_generated_summary("s1", "- [ ] Send proposal", Some("original"))
                .await
                .is_err()
        );
        assert_eq!(
            store.read_summary("s1").await.unwrap().as_deref(),
            Some("original")
        );
    }

    #[tokio::test]
    async fn plain_markdown_survives_rebuild_external_edit_and_delete() {
        let (vault, store) = setup().await;
        assert_eq!(store.read_summary("s1").await.unwrap(), None);
        assert_eq!(store.ensure_summary("s1").await.unwrap(), "");
        let body = "---\nthis is not YAML: [\n\n# Summary\n- [x] Done\n";
        store.update_summary("s1", body, Some("")).await.unwrap();
        let path = vault.path().join("sessions/s1/summary.md");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), body);
        assert!(!vault.path().join("sessions/s1/enhanced").exists());
        let restarted = SessionStore::new(vault.path().to_owned());
        restarted.rebuild_index().await.unwrap();
        assert_eq!(restarted.summary_get("s1").as_deref(), Some(body));
        assert_eq!(restarted.session_enhanced_docs("s1")[0].id, "s1");
        std::fs::write(&path, "external change").unwrap();
        restarted.refresh_session("s1").await.unwrap();
        assert_eq!(
            restarted.summary_get("s1").as_deref(),
            Some("external change")
        );
        assert!(matches!(
            restarted.update_summary("s1", "stale", Some(body)).await,
            Err(StoreError::Conflict(_))
        ));
        restarted.delete_summary("s1").await.unwrap();
        restarted.rebuild_index().await.unwrap();
        assert_eq!(restarted.summary_get("s1"), None);
        assert!(!path.exists());
        assert!(
            restarted
                .update_summary("s1", "resurrect", None)
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn concurrent_ensure_never_replaces_existing_content() {
        let (_vault, store) = setup().await;
        let (one, two) = tokio::join!(store.ensure_summary("s1"), store.ensure_summary("s1"));
        assert_eq!(one.unwrap(), "");
        assert_eq!(two.unwrap(), "");
        store.update_summary("s1", "saved", None).await.unwrap();
        assert_eq!(store.ensure_summary("s1").await.unwrap(), "saved");
        assert!(store.ensure_summary("missing").await.is_err());
    }

    #[tokio::test]
    async fn migration_preserves_body_and_task_identity_and_retries() {
        let (vault, store) = setup().await;
        let body = "---\nnot: [frontmatter\n\n- [x] Ship\n";
        legacy(&store, body).await;
        store
            .replace_tasks(
                "enhanced_note",
                "old-summary",
                vec![TaskInput {
                    id: "task-1".into(),
                    source_order: 2,
                    status: "done".into(),
                    text: "Ship".into(),
                    body: serde_json::json!([]),
                    due_at: "2026-09-20".into(),
                }],
            )
            .await
            .unwrap();
        let before = store
            .list_tasks("enhanced_note", "old-summary")
            .await
            .unwrap()
            .remove(0);
        store.migrate_summary("s1").await.unwrap();
        store.migrate_summary("s1").await.unwrap();
        assert_eq!(
            std::fs::read_to_string(vault.path().join("sessions/s1/summary.md")).unwrap(),
            body
        );
        assert!(
            !vault
                .path()
                .join("sessions/s1/enhanced/old-summary.md")
                .exists()
        );
        assert!(
            !vault
                .path()
                .join("sessions/s1/.summary-migration.json")
                .exists()
        );
        let cold = SessionStore::new(vault.path().to_owned());
        let after = cold
            .list_tasks("session_summary", "s1")
            .await
            .unwrap()
            .remove(0);
        let mut expected = before;
        expected.source_type = "session_summary".into();
        expected.source_id = "s1".into();
        assert_eq!(after, expected);
        cold.delete_summary("s1").await.unwrap();
        cold.migrate_summary("s1").await.unwrap();
        assert_eq!(cold.read_summary("s1").await.unwrap(), None);
    }

    #[tokio::test]
    async fn collision_preserves_attachment_and_legacy_summary_remains_editable() {
        let (vault, store) = setup().await;
        legacy(&store, "legacy").await;
        let path = vault.path().join("sessions/s1/summary.md");
        std::fs::write(&path, "user attachment").unwrap();
        assert!(matches!(
            store.migrate_summary("s1").await,
            Err(StoreError::Conflict(_))
        ));
        assert_eq!(store.ensure_summary("s1").await.unwrap(), "legacy");
        store
            .update_summary("s1", "edited", Some("legacy"))
            .await
            .unwrap();
        assert_eq!(
            store.read_summary("s1").await.unwrap().as_deref(),
            Some("edited")
        );
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "user attachment");
        assert!(store.delete_summary("s1").await.is_err());
        std::fs::rename(&path, path.with_file_name("attachment.md")).unwrap();
        store.migrate_summary("s1").await.unwrap();
        assert_eq!(std::fs::read_to_string(path).unwrap(), "edited");
    }

    #[tokio::test]
    async fn interrupted_publication_completes_without_duplicate_content() {
        for source_already_archived in [false, true] {
            let (vault, store) = setup().await;
            legacy(&store, "body").await;
            let dir = vault.path().join("sessions/s1");
            let pending = PendingMigration {
                legacy_id: "old-summary".into(),
                markdown: "body".into(),
                original: std::fs::read_to_string(dir.join("enhanced/old-summary.md")).unwrap(),
            };
            std::fs::write(
                dir.join(".summary-migration.json"),
                serde_json::to_vec(&pending).unwrap(),
            )
            .unwrap();
            std::fs::write(dir.join("summary.md"), "body").unwrap();
            if source_already_archived {
                hypr_fs_sync_core::export::move_to_trash(
                    vault.path(),
                    &dir.join("enhanced/old-summary.md"),
                )
                .unwrap();
            }
            let restarted = SessionStore::new(vault.path().to_owned());
            restarted.migrate_summary("s1").await.unwrap();
            restarted.rebuild_index().await.unwrap();
            assert_eq!(restarted.session_enhanced_docs("s1").len(), 1);
            assert!(!dir.join("enhanced/old-summary.md").exists());
        }
    }

    #[tokio::test]
    async fn read_error_keeps_index_and_external_deletion_prunes_it() {
        let (vault, store) = setup().await;
        store.ensure_summary("s1").await.unwrap();
        store.update_summary("s1", "saved", None).await.unwrap();
        let path = vault.path().join("sessions/s1/summary.md");
        std::fs::write(&path, [0xff]).unwrap();
        assert!(store.refresh_session("s1").await.is_err());
        assert_eq!(store.summary_get("s1").as_deref(), Some("saved"));
        std::fs::remove_file(path).unwrap();
        store.refresh_session("s1").await.unwrap();
        assert_eq!(store.summary_get("s1"), None);
    }
    #[tokio::test]
    async fn ambiguous_legacy_files_and_invalid_tasks_are_preserved() {
        let (vault, store) = setup().await;
        legacy(&store, "first").await;
        let dir = vault.path().join("sessions/s1");
        let original = std::fs::read(dir.join("enhanced/old-summary.md")).unwrap();
        std::fs::write(dir.join("enhanced/other.md"), "second").unwrap();
        assert!(store.migrate_summary("s1").await.is_err());
        assert!(!dir.join("summary.md").exists());
        assert_eq!(
            std::fs::read(dir.join("enhanced/old-summary.md")).unwrap(),
            original
        );
        std::fs::remove_file(dir.join("enhanced/other.md")).unwrap();
        std::fs::write(dir.join("tasks.json"), "{invalid").unwrap();
        let layout = store.normalize_startup_layout().await.unwrap();
        assert!(
            layout
                .migration
                .failed
                .iter()
                .any(|error| error.contains("s1"))
        );
        assert!(!dir.join("summary.md").exists());
        assert!(!dir.join(".summary-migration.json").exists());
        assert_eq!(
            std::fs::read(dir.join("enhanced/old-summary.md")).unwrap(),
            original
        );
    }

    #[tokio::test]
    async fn unchanged_summary_rebuild_is_silent() {
        let (_vault, store) = setup().await;
        store.ensure_summary("s1").await.unwrap();
        store.update_summary("s1", "summary", None).await.unwrap();
        store.rebuild_index().await.unwrap();
        let mut receiver = store.take_index_change_receiver().unwrap();
        while receiver.try_recv().is_ok() {}
        store.rebuild_index().await.unwrap();
        assert!(receiver.try_recv().is_err());
    }
}
