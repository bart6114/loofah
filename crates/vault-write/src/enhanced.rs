use serde::{Deserialize, Serialize};

use super::{SessionStore, StoreError, WriteGuard, paths, validate_doc_id, validate_session_id};

// The `enhanced/<id>.md` schema (type, frontmatter parse/render) is shared with the
// read-only vault consumers (loof CLI/MCP) and lives in `hypr-vault-read`.
pub use hypr_vault_read::{ENHANCED_KINDS, EnhancedDoc};

/// Partial update for an existing enhanced doc: `None` means "leave as-is". The `expected_*`
/// fields are compare-and-swap guards against the *current file content* -- a mismatch
/// returns `StoreError::Conflict` and changes nothing, which is the store-level equivalent
/// of the SQL era's `expectedRowsAffected`/`WHERE title = ?` rejections.
#[derive(Serialize, Deserialize, specta::Type, Clone, Debug, Default, PartialEq)]
pub struct EnhancedDocPatch {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reconcile_tasks: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub template_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sort_order: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub markdown: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_markdown: Option<String>,
}

pub(super) fn render_enhanced_file(doc: &EnhancedDoc) -> Result<String, StoreError> {
    hypr_vault_read::render_enhanced_file(doc).map_err(Into::into)
}

/// See `hypr_vault_read::parse_enhanced_file`: a file with no frontmatter is accepted as a
/// plain summary, malformed frontmatter is an error -- rebuild treats that like any other
/// unparseable artifact (log, leave the existing index row alone).
pub(super) fn parse_enhanced_file(
    id: &str,
    session_id: &str,
    raw: &str,
) -> Result<EnhancedDoc, StoreError> {
    hypr_vault_read::parse_enhanced_file(id, session_id, raw).map_err(Into::into)
}

impl SessionStore {
    /// Create-or-replace an enhanced doc: file write first, then the index write-through.
    /// Requires the session's `_meta.json` to exist -- creating a doc must never resurrect
    /// a session folder that a racing delete just trashed.
    pub async fn write_enhanced_doc(&self, doc: &EnhancedDoc) -> Result<(), StoreError> {
        validate_kind(&doc.kind)?;
        validate_session_id(&doc.session_id)?;
        validate_doc_id(&doc.id)?;

        if self.read_meta(&doc.session_id).await?.is_none() {
            return Err(StoreError::Io(format!(
                "session {} has no _meta.json; refusing to create an enhanced doc",
                doc.session_id
            )));
        }

        self.persist_enhanced_doc(doc).await
    }

    /// Read-modify-write partial update. Errors when the doc file doesn't exist (the SQL
    /// era's `expectedRowsAffected: 1` "row vanished" rejection); `Conflict` when a CAS
    /// guard in the patch doesn't match the current file content.
    pub async fn update_enhanced_doc(
        &self,
        session_id: &str,
        doc_id: &str,
        patch: EnhancedDocPatch,
    ) -> Result<(), StoreError> {
        validate_session_id(session_id)?;
        validate_doc_id(doc_id)?;

        let task_content = if patch.reconcile_tasks == Some(true) {
            let markdown = patch.markdown.clone().ok_or_else(|| {
                StoreError::Serialize("task reconciliation requires Markdown".into())
            })?;
            Some(
                tokio::task::spawn_blocking(move || hypr_tiptap::md_to_tiptap_json(&markdown))
                    .await
                    .map_err(|error| StoreError::Io(format!("task parse failed: {error}")))?
                    .map_err(StoreError::Serialize)?,
            )
        } else {
            None
        };

        // Held across the read-modify-write so a concurrent patch of the same doc can't be
        // computed from bytes this call is about to replace.
        let guard = self.lock_writes().await;

        let mut doc = self
            .read_enhanced_doc(session_id, doc_id)
            .await?
            .ok_or_else(|| {
                StoreError::Io(format!(
                    "enhanced doc {doc_id} in session {session_id} has no file to update"
                ))
            })?;

        if let Some(expected) = &patch.expected_title {
            if &doc.title != expected {
                return Err(StoreError::Conflict(format!(
                    "enhanced doc {doc_id} title changed (expected {expected:?}, found {:?})",
                    doc.title
                )));
            }
        }
        if let Some(expected) = &patch.expected_markdown {
            if &doc.markdown != expected
                && !(patch.reconcile_tasks == Some(true)
                    && patch.markdown.as_ref() == Some(&doc.markdown))
            {
                return Err(StoreError::Conflict(format!(
                    "enhanced doc {doc_id} body changed since it was read"
                )));
            }
        }

        let EnhancedDocPatch {
            reconcile_tasks: _,
            kind,
            title,
            template_id,
            sort_order,
            markdown,
            expected_title: _,
            expected_markdown: _,
        } = patch;

        if let Some(kind) = kind {
            validate_kind(&kind)?;
            doc.kind = kind;
        }
        if let Some(title) = title {
            doc.title = title;
        }
        if let Some(template_id) = template_id {
            doc.template_id = template_id;
        }
        if let Some(sort_order) = sort_order {
            doc.sort_order = sort_order;
        }
        if let Some(markdown) = markdown {
            doc.markdown = markdown;
        }

        let tasks = if let Some(content) = task_content {
            Some(
                self.prepare_generated_tasks(session_id, doc_id, &content)
                    .await?,
            )
        } else {
            None
        };
        self.persist_enhanced_doc_locked(&guard, &doc, tasks.as_deref())
            .await
    }

    pub async fn read_enhanced_doc(
        &self,
        session_id: &str,
        doc_id: &str,
    ) -> Result<Option<EnhancedDoc>, StoreError> {
        validate_session_id(session_id)?;
        validate_doc_id(doc_id)?;
        let vault_base = self.vault_base.clone();
        let session_id = session_id.to_string();
        let doc_id = doc_id.to_string();

        let session_dir = self.session_dir(&session_id).await?;
        tokio::task::spawn_blocking(move || -> Result<Option<EnhancedDoc>, StoreError> {
            let path = vault_base.join(paths::enhanced_doc_path_in(&session_dir, &doc_id));
            // Attempt-then-match, same rationale as read_meta: never mistake a transient
            // read failure for "doc doesn't exist".
            let raw = match std::fs::read_to_string(&path) {
                Ok(raw) => raw,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
                Err(e) => {
                    return Err(StoreError::Io(format!(
                        "failed to read enhanced doc file: {e}"
                    )));
                }
            };
            parse_enhanced_file(&doc_id, &session_id, &raw).map(Some)
        })
        .await
        .map_err(|e| StoreError::Io(format!("task join error: {e}")))?
    }

    /// File to `.trash/<date>/sessions/<id>/enhanced/<doc>.md` (hand-recoverable, never
    /// synced), then the index entry is removed. No tombstone: no frontend undo path
    /// exists for enhanced notes, and rebuild prunes file-less entries anyway.
    /// Idempotent: deleting a doc that doesn't exist succeeds.
    pub async fn delete_enhanced_doc(
        &self,
        session_id: &str,
        doc_id: &str,
    ) -> Result<(), StoreError> {
        validate_session_id(session_id)?;
        validate_doc_id(doc_id)?;

        // The trash-move is a write: resolve the session's directory under the write
        // lock so a concurrent rename can't strand the delete at a stale path.
        let guard = self.lock_writes().await;
        let session_dir = self.session_dir_locked(&guard, session_id).await?;
        let vault_base = self.vault_base.clone();
        let relative = paths::enhanced_doc_path_in(&session_dir, doc_id);

        tokio::task::spawn_blocking(move || -> Result<(), StoreError> {
            let abs = vault_base.join(relative);
            hypr_fs_sync_core::export::move_to_trash(&vault_base, &abs).map_err(|e| {
                StoreError::Io(format!("failed to move enhanced doc to trash: {e}"))
            })?;
            Ok(())
        })
        .await
        .map_err(|e| StoreError::Io(format!("task join error: {e}")))??;
        drop(guard);

        self.index_remove_doc(session_id, doc_id);
        self.notify_index_changed(super::IndexEntity::Docs, vec![session_id.to_string()]);

        Ok(())
    }

    async fn persist_enhanced_doc(&self, doc: &EnhancedDoc) -> Result<(), StoreError> {
        let guard = self.lock_writes().await;
        self.persist_enhanced_doc_locked(&guard, doc, None).await
    }

    async fn persist_enhanced_doc_locked(
        &self,
        guard: &WriteGuard<'_>,
        doc: &EnhancedDoc,
        tasks: Option<&[super::TaskItem]>,
    ) -> Result<(), StoreError> {
        let rendered = render_enhanced_file(doc)?;
        let session_dir = self.session_dir_locked(guard, &doc.session_id).await?;
        self.write_file_locked(
            guard,
            paths::enhanced_doc_path_in(&session_dir, &doc.id),
            rendered.into_bytes(),
        )
        .await?;
        let task_result = if let Some(tasks) = tasks {
            self.persist_generated_tasks(guard, &doc.session_id, tasks)
                .await
                .map_err(|error| {
                    StoreError::Io(format!(
                        "Summary saved but task synchronization failed; retry to finish: {error}"
                    ))
                })
        } else {
            Ok(())
        };
        self.index_upsert_doc(doc);
        self.notify_index_changed(super::IndexEntity::Docs, vec![doc.session_id.clone()]);
        task_result
    }
}

fn validate_kind(kind: &str) -> Result<(), StoreError> {
    if ENHANCED_KINDS.contains(&kind) {
        Ok(())
    } else {
        Err(StoreError::Serialize(format!(
            "invalid enhanced doc kind {kind:?} (expected one of {ENHANCED_KINDS:?})"
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::super::content::SessionMeta;
    use super::*;

    fn meta(id: &str) -> SessionMeta {
        SessionMeta {
            id: id.to_string(),
            title: "Session".to_string(),
            started_at: None,
            ended_at: None,
            created_at: "2026-07-24T00:00:00Z".to_string(),
            tags: vec![],
            tag_suggestions: None,
            tracking_id: None,
            folder: None,
            author: None,
            skill: None,
            extra: Default::default(),
        }
    }

    fn doc(session_id: &str, doc_id: &str) -> EnhancedDoc {
        EnhancedDoc {
            id: doc_id.to_string(),
            session_id: session_id.to_string(),
            kind: "template_output".to_string(),
            title: "Customer review".to_string(),
            template_id: "template-1".to_string(),
            sort_order: 3,
            markdown: "# Review\n\n- Point".to_string(),
        }
    }

    async fn test_store() -> (SessionStore, tempfile::TempDir) {
        let temp = tempfile::tempdir().unwrap();
        let vault = temp.path().to_path_buf();
        let store = SessionStore::new(vault);
        (store, temp)
    }

    async fn session_path(
        store: &SessionStore,
        vault: &tempfile::TempDir,
        id: &str,
    ) -> std::path::PathBuf {
        vault.path().join(store.session_dir(id).await.unwrap())
    }

    #[tokio::test]
    async fn generated_summary_persists_and_reconciles_tasks_without_an_editor() {
        let (store, vault) = test_store().await;
        store.write_meta(&meta("s1")).await.unwrap();
        let original = doc("s1", "doc-1");
        store.write_enhanced_doc(&original).await.unwrap();
        store
            .replace_tasks(
                "session_raw_note",
                "s1",
                vec![super::super::TaskInput {
                    id: "raw-task".into(),
                    source_order: 0,
                    status: "todo".into(),
                    text: "Keep raw task".into(),
                    body: serde_json::json!([]),
                    due_at: String::new(),
                }],
            )
            .await
            .unwrap();
        let markdown = "# Actions\n- Bob sends invoice\n- [ ] Send proposal\n- [ ] Send proposal\n- [ ] Obsolete action";
        let patch = EnhancedDocPatch {
            markdown: Some(markdown.into()),
            expected_markdown: Some(original.markdown.clone()),
            reconcile_tasks: Some(true),
            ..Default::default()
        };
        store
            .update_enhanced_doc("s1", "doc-1", patch.clone())
            .await
            .unwrap();
        let tasks = store.list_tasks("enhanced_note", "doc-1").await.unwrap();
        assert_eq!(tasks.len(), 3);
        assert_eq!(tasks[0].text, "Send proposal");
        assert_ne!(tasks[0].id, tasks[1].id);
        assert!(
            session_path(&store, &vault, "s1")
                .await
                .join("tasks.json")
                .is_file()
        );
        // A retry after the Markdown write must reuse IDs and timestamps.
        store
            .update_enhanced_doc("s1", "doc-1", patch)
            .await
            .unwrap();
        assert_eq!(
            store.list_tasks("enhanced_note", "doc-1").await.unwrap(),
            tasks
        );
        store
            .replace_tasks(
                "enhanced_note",
                "doc-1",
                tasks
                    .iter()
                    .enumerate()
                    .map(|(index, task)| super::super::TaskInput {
                        id: task.id.clone(),
                        source_order: task.source_order,
                        status: if index == 0 { "done" } else { "in_progress" }.into(),
                        text: task.text.clone(),
                        body: task.body.clone(),
                        due_at: "2026-10-01".into(),
                    })
                    .collect(),
            )
            .await
            .unwrap();
        store
            .update_enhanced_doc(
                "s1",
                "doc-1",
                EnhancedDocPatch {
                    markdown: Some(
                        "# Actions\n- [ ] Send proposal\n- [ ] New action\n- [ ] Send proposal"
                            .into(),
                    ),
                    expected_markdown: Some(markdown.into()),
                    reconcile_tasks: Some(true),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        let updated = store.list_tasks("enhanced_note", "doc-1").await.unwrap();
        assert_eq!(updated.len(), 3);
        assert_eq!(updated[0].id, tasks[0].id);
        assert_eq!(updated[0].status, "done");
        assert_eq!(updated[0].due_at, "2026-10-01");
        assert_eq!(updated[0].created_at, tasks[0].created_at);
        assert_eq!(updated[2].id, tasks[1].id);
        assert_eq!(updated[2].status, "in_progress");
        assert!(!updated.iter().any(|task| task.id == tasks[2].id));
        assert_eq!(
            store
                .list_tasks("session_raw_note", "s1")
                .await
                .unwrap()
                .len(),
            1
        );
        let stale = store
            .update_enhanced_doc(
                "s1",
                "doc-1",
                EnhancedDocPatch {
                    markdown: Some("- [ ] Stale".into()),
                    expected_markdown: Some(original.markdown),
                    reconcile_tasks: Some(true),
                    ..Default::default()
                },
            )
            .await;
        assert!(matches!(stale, Err(StoreError::Conflict(_))));
        assert_eq!(
            store.list_tasks("enhanced_note", "doc-1").await.unwrap(),
            updated
        );
        store
            .update_enhanced_doc(
                "s1",
                "doc-1",
                EnhancedDocPatch {
                    markdown: Some("# Discussion\n- No personal actions.".into()),
                    reconcile_tasks: Some(true),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        assert!(
            store
                .list_tasks("enhanced_note", "doc-1")
                .await
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            store
                .list_tasks("session_raw_note", "s1")
                .await
                .unwrap()
                .len(),
            1
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn partial_task_write_failure_is_reported_and_retryable() {
        use std::os::unix::fs::PermissionsExt;
        let (store, vault) = test_store().await;
        store.write_meta(&meta("s1")).await.unwrap();
        let original = doc("s1", "doc-1");
        store.write_enhanced_doc(&original).await.unwrap();
        let dir = session_path(&store, &vault, "s1").await;
        let permissions = std::fs::metadata(&dir).unwrap().permissions();
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o555)).unwrap();
        let patch = EnhancedDocPatch {
            markdown: Some("# Actions\n- [ ] Send proposal".into()),
            expected_markdown: Some(original.markdown),
            reconcile_tasks: Some(true),
            ..Default::default()
        };
        let result = store
            .update_enhanced_doc("s1", "doc-1", patch.clone())
            .await;
        std::fs::set_permissions(&dir, permissions).unwrap();
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("task synchronization failed")
        );
        assert_eq!(
            store
                .read_enhanced_doc("s1", "doc-1")
                .await
                .unwrap()
                .unwrap()
                .markdown,
            patch.markdown.clone().unwrap()
        );
        store
            .update_enhanced_doc("s1", "doc-1", patch)
            .await
            .unwrap();
        assert_eq!(
            store
                .list_tasks("enhanced_note", "doc-1")
                .await
                .unwrap()
                .len(),
            1
        );
    }

    #[tokio::test]
    async fn invalid_tasks_file_does_not_partially_replace_a_summary() {
        let (store, vault) = test_store().await;
        store.write_meta(&meta("s1")).await.unwrap();
        let original = doc("s1", "doc-1");
        store.write_enhanced_doc(&original).await.unwrap();
        std::fs::write(
            session_path(&store, &vault, "s1").await.join("tasks.json"),
            "invalid",
        )
        .unwrap();
        assert!(
            store
                .update_enhanced_doc(
                    "s1",
                    "doc-1",
                    EnhancedDocPatch {
                        markdown: Some("- [ ] Send proposal".into()),
                        reconcile_tasks: Some(true),
                        ..Default::default()
                    }
                )
                .await
                .is_err()
        );
        assert_eq!(
            store.read_enhanced_doc("s1", "doc-1").await.unwrap(),
            Some(original)
        );
    }

    #[tokio::test]
    async fn write_enhanced_doc_round_trips_file_and_index() {
        let (store, vault) = test_store().await;
        store.write_meta(&meta("s1")).await.unwrap();
        let d = doc("s1", "doc-1");
        store.write_enhanced_doc(&d).await.unwrap();

        assert!(
            session_path(&store, &vault, "s1")
                .await
                .join("enhanced/doc-1.md")
                .is_file()
        );
        assert_eq!(
            store.read_enhanced_doc("s1", "doc-1").await.unwrap(),
            Some(d.clone())
        );

        assert_eq!(store.enhanced_doc_get("doc-1"), Some(d));
    }

    /// The file itself carries the metadata (frontmatter, no sidecar) and only the body
    /// below the closing delimiter -- the delimiter discipline is what lets rebuild restore
    /// title/template_id/sort_order/kind from the file alone.
    #[tokio::test]
    async fn enhanced_file_has_frontmatter_metadata_and_bare_body() {
        let (store, vault) = test_store().await;
        store.write_meta(&meta("s1")).await.unwrap();
        store.write_enhanced_doc(&doc("s1", "doc-1")).await.unwrap();

        let raw = std::fs::read_to_string(
            session_path(&store, &vault, "s1")
                .await
                .join("enhanced/doc-1.md"),
        )
        .unwrap();
        assert!(raw.starts_with("---\n"));
        assert!(raw.contains("title: Customer review"));
        assert!(raw.contains("template_id: template-1"));
        assert!(raw.contains("sort_order: 3"));
        assert!(raw.contains("kind: template_output"));
        assert!(raw.ends_with("# Review\n\n- Point"));
    }

    #[tokio::test]
    async fn write_enhanced_doc_refuses_a_session_without_meta() {
        let (store, vault) = test_store().await;
        let result = store.write_enhanced_doc(&doc("ghost", "doc-1")).await;
        assert!(result.is_err());
        assert!(!vault.path().join("sessions/ghost").exists());
    }

    #[tokio::test]
    async fn write_enhanced_doc_rejects_an_unknown_kind() {
        let (store, _vault) = test_store().await;
        store.write_meta(&meta("s1")).await.unwrap();
        let mut d = doc("s1", "doc-1");
        d.kind = "note".to_string();
        assert!(store.write_enhanced_doc(&d).await.is_err());
    }

    #[tokio::test]
    async fn update_enhanced_doc_patches_only_the_given_fields() {
        let (store, _vault) = test_store().await;
        store.write_meta(&meta("s1")).await.unwrap();
        let d = doc("s1", "doc-1");
        store.write_enhanced_doc(&d).await.unwrap();

        store
            .update_enhanced_doc(
                "s1",
                "doc-1",
                EnhancedDocPatch {
                    markdown: Some("# Replaced".to_string()),
                    ..Default::default()
                },
            )
            .await
            .unwrap();

        let after = store
            .read_enhanced_doc("s1", "doc-1")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(after.markdown, "# Replaced");
        assert_eq!(after.title, d.title, "unpatched fields must survive");
        assert_eq!(after.template_id, d.template_id);
        assert_eq!(after.sort_order, d.sort_order);

        assert_eq!(
            store.enhanced_doc_get("doc-1").unwrap().markdown,
            "# Replaced",
            "write-through must reach the index"
        );
    }

    #[tokio::test]
    async fn update_enhanced_doc_errors_when_the_file_is_missing() {
        let (store, _vault) = test_store().await;
        store.write_meta(&meta("s1")).await.unwrap();
        let result = store
            .update_enhanced_doc(
                "s1",
                "ghost-doc",
                EnhancedDocPatch {
                    markdown: Some("x".to_string()),
                    ..Default::default()
                },
            )
            .await;
        assert!(result.is_err());
        assert!(!matches!(result.unwrap_err(), StoreError::Conflict(_)));
    }

    #[tokio::test]
    async fn update_enhanced_doc_title_cas_conflict_is_typed_and_changes_nothing() {
        let (store, _vault) = test_store().await;
        store.write_meta(&meta("s1")).await.unwrap();
        store.write_enhanced_doc(&doc("s1", "doc-1")).await.unwrap();

        let result = store
            .update_enhanced_doc(
                "s1",
                "doc-1",
                EnhancedDocPatch {
                    title: Some("Hydrated".to_string()),
                    expected_title: Some("Some other title".to_string()),
                    ..Default::default()
                },
            )
            .await;

        assert!(matches!(result.unwrap_err(), StoreError::Conflict(_)));
        let after = store
            .read_enhanced_doc("s1", "doc-1")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            after.title, "Customer review",
            "a conflict must change nothing"
        );
    }

    #[tokio::test]
    async fn update_enhanced_doc_body_cas_applies_when_current_and_conflicts_when_stale() {
        let (store, _vault) = test_store().await;
        store.write_meta(&meta("s1")).await.unwrap();
        store.write_enhanced_doc(&doc("s1", "doc-1")).await.unwrap();

        store
            .update_enhanced_doc(
                "s1",
                "doc-1",
                EnhancedDocPatch {
                    markdown: Some("# Regenerated".to_string()),
                    expected_markdown: Some("# Review\n\n- Point".to_string()),
                    ..Default::default()
                },
            )
            .await
            .unwrap();

        let stale = store
            .update_enhanced_doc(
                "s1",
                "doc-1",
                EnhancedDocPatch {
                    markdown: Some("# From a stale run".to_string()),
                    expected_markdown: Some("# Review\n\n- Point".to_string()),
                    ..Default::default()
                },
            )
            .await;
        assert!(matches!(stale.unwrap_err(), StoreError::Conflict(_)));

        let after = store
            .read_enhanced_doc("s1", "doc-1")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(after.markdown, "# Regenerated");
    }

    /// The conflict error must be recognizable across the IPC string boundary -- the
    /// frontend distinguishes benign CAS misses (title hydration) from real failures by
    /// this prefix.
    #[test]
    fn conflict_error_stringifies_with_a_stable_prefix() {
        let err = StoreError::Conflict("x".to_string());
        assert!(err.to_string().starts_with("conflict:"));
    }

    #[tokio::test]
    async fn delete_enhanced_doc_moves_file_to_trash_and_hard_deletes_the_row() {
        let (store, vault) = test_store().await;
        store.write_meta(&meta("s1")).await.unwrap();
        store.write_enhanced_doc(&doc("s1", "doc-1")).await.unwrap();
        let rel = store.session_dir("s1").await.unwrap();

        store.delete_enhanced_doc("s1", "doc-1").await.unwrap();

        assert!(!vault.path().join(&rel).join("enhanced/doc-1.md").exists());
        let date = chrono::Utc::now().format("%Y-%m-%d").to_string();
        assert!(
            vault
                .path()
                .join(".trash")
                .join(date)
                .join(&rel)
                .join("enhanced/doc-1.md")
                .is_file(),
            "deleted doc must be hand-recoverable from trash"
        );
        assert!(
            store.enhanced_doc_get("doc-1").is_none(),
            "no tombstone: the index entry is gone"
        );
    }

    #[tokio::test]
    async fn delete_enhanced_doc_on_a_nonexistent_doc_succeeds() {
        let (store, _vault) = test_store().await;
        store.write_meta(&meta("s1")).await.unwrap();
        assert!(
            store
                .delete_enhanced_doc("s1", "never-existed")
                .await
                .is_ok()
        );
    }

    #[test]
    fn parse_accepts_a_file_without_frontmatter_as_a_plain_summary() {
        let parsed =
            parse_enhanced_file("d1", "s1", "# Just markdown\n\nDropped in by hand.").unwrap();
        assert_eq!(parsed.kind, "summary");
        assert_eq!(parsed.title, "");
        assert_eq!(parsed.template_id, "");
        assert_eq!(parsed.sort_order, 0);
        assert_eq!(parsed.markdown, "# Just markdown\n\nDropped in by hand.");
    }

    #[test]
    fn parse_degrades_an_unknown_kind_to_summary() {
        let raw = "---\nkind: nonsense\ntitle: T\n---\n\nbody";
        let parsed = parse_enhanced_file("d1", "s1", raw).unwrap();
        assert_eq!(parsed.kind, "summary");
        assert_eq!(parsed.title, "T");
    }

    #[test]
    fn parse_errors_on_malformed_frontmatter() {
        let raw = "---\ntitle: [unclosed\n---\n\nbody";
        assert!(parse_enhanced_file("d1", "s1", raw).is_err());
    }

    #[test]
    fn render_parse_round_trip_preserves_every_field() {
        let d = doc("s1", "doc-1");
        let rendered = render_enhanced_file(&d).unwrap();
        let parsed = parse_enhanced_file("doc-1", "s1", &rendered).unwrap();
        assert_eq!(parsed, d);
    }

    /// A title that needs YAML quoting (colon + quotes) must survive the file round-trip
    /// intact -- the frontmatter is the metadata home, so escaping bugs here would corrupt
    /// real titles.
    #[test]
    fn render_parse_round_trip_survives_a_yaml_hostile_title() {
        let mut d = doc("s1", "doc-1");
        d.title = "Q3: \"planning\" #review --- done".to_string();
        let rendered = render_enhanced_file(&d).unwrap();
        let parsed = parse_enhanced_file("doc-1", "s1", &rendered).unwrap();
        assert_eq!(parsed.title, d.title);
    }
}
