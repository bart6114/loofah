use hypr_loofah_protocol::{DocumentSaveOutcome, DocumentSnapshot, DocumentTarget};

use crate::{EnhancedDocPatch, SessionStore, StoreError, WriteGuard, paths};

#[derive(Clone, Copy)]
pub(super) enum DocumentWriteMode {
    Edited,
    Generated,
}

impl SessionStore {
    pub async fn read_recovered_document_snapshot(
        &self,
        operation_root: Option<&std::path::Path>,
        session_id: &str,
        target: &DocumentTarget,
    ) -> Result<Option<DocumentSnapshot>, StoreError> {
        let target = target.clone();
        self.recovered_read(
            operation_root,
            session_id,
            |store, snapshot, id| async move {
                store
                    .document_snapshot_locked(&snapshot._guard, &id, &target)
                    .await
            },
        )
        .await
    }

    pub async fn read_recovered_session_documents(
        &self,
        operation_root: Option<&std::path::Path>,
        session_id: &str,
    ) -> Result<Option<Vec<crate::EnhancedDoc>>, StoreError> {
        self.recovered_read(
            operation_root,
            session_id,
            |store, snapshot, id| async move {
                let _snapshot = snapshot;
                if store.read_meta(&id).await?.is_none() {
                    return Ok(None);
                }
                Ok(Some(store.session_enhanced_docs(&id)))
            },
        )
        .await
    }

    pub async fn read_document_snapshot(
        &self,
        session_id: &str,
        target: &DocumentTarget,
    ) -> Result<Option<DocumentSnapshot>, StoreError> {
        let guard = self.lock_writes().await;
        self.document_snapshot_locked(&guard, session_id, target)
            .await
    }

    pub async fn save_document_if_revision(
        &self,
        operation_root: &std::path::Path,
        session_id: &str,
        target: &DocumentTarget,
        markdown: &str,
        expected_revision: &str,
    ) -> Result<DocumentSaveOutcome, StoreError> {
        self.save_document_mode(
            operation_root,
            session_id,
            target,
            markdown,
            expected_revision,
            DocumentWriteMode::Edited,
        )
        .await
    }

    pub async fn save_generated_document_if_revision(
        &self,
        operation_root: &std::path::Path,
        session_id: &str,
        target: &DocumentTarget,
        markdown: &str,
        expected_revision: &str,
    ) -> Result<DocumentSaveOutcome, StoreError> {
        self.save_document_mode(
            operation_root,
            session_id,
            target,
            markdown,
            expected_revision,
            DocumentWriteMode::Generated,
        )
        .await
    }

    async fn save_document_mode(
        &self,
        operation_root: &std::path::Path,
        session_id: &str,
        target: &DocumentTarget,
        markdown: &str,
        expected_revision: &str,
        mode: DocumentWriteMode,
    ) -> Result<DocumentSaveOutcome, StoreError> {
        let result = match target {
            DocumentTarget::Summary => {
                self.update_summary_checked_impl(
                    session_id,
                    markdown,
                    None,
                    true,
                    None,
                    None,
                    false,
                    Some((expected_revision, operation_root)),
                    mode,
                )
                .await?
            }
            DocumentTarget::TemplateOutput { document_id } => {
                self.update_enhanced_doc_checked_impl(
                    session_id,
                    document_id,
                    EnhancedDocPatch {
                        markdown: Some(markdown.into()),
                        reconcile_tasks: Some(true),
                        ..Default::default()
                    },
                    None,
                    false,
                    Some((expected_revision, operation_root)),
                    mode,
                )
                .await?
            }
        };
        result.ok_or_else(|| StoreError::Io("document save omitted its result".into()))
    }

    pub(super) async fn document_snapshot_locked(
        &self,
        _guard: &WriteGuard<'_>,
        session_id: &str,
        target: &DocumentTarget,
    ) -> Result<Option<DocumentSnapshot>, StoreError> {
        crate::validate_session_id(session_id)?;
        if let DocumentTarget::TemplateOutput { document_id } = target {
            crate::validate_doc_id(document_id)?;
        }
        if self.read_meta(session_id).await?.is_none() {
            return Ok(None);
        }
        let dir = self.session_dir(session_id).await?;
        let vault = self.vault_base.clone();
        let session = session_id.to_owned();
        let target = target.clone();
        tokio::task::spawn_blocking(move || {
            let located = match &target {
                DocumentTarget::Summary => {
                    hypr_vault_read::summary::locate_in(&vault, &dir, &session)?
                        .map(|summary| (summary.relative_path, summary.legacy_id))
                }
                DocumentTarget::TemplateOutput { document_id } => {
                    let parent = vault.join(paths::enhanced_dir_in(&dir));
                    match std::fs::symlink_metadata(&parent) {
                        Ok(meta) if !meta.is_dir() || meta.file_type().is_symlink() => {
                            return Err(StoreError::Io(
                                "enhanced is not a regular directory".into(),
                            ));
                        }
                        Err(error) if error.kind() != std::io::ErrorKind::NotFound => {
                            return Err(StoreError::Io(error.to_string()));
                        }
                        _ => {}
                    }
                    let path = paths::enhanced_doc_path_in(&dir, document_id);
                    match std::fs::symlink_metadata(vault.join(&path)) {
                        Ok(meta) if !meta.is_file() => {
                            return Err(StoreError::Io("document is not a regular file".into()));
                        }
                        Ok(_) => Some((path, Some(document_id.clone()))),
                        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
                        Err(error) => return Err(StoreError::Io(error.to_string())),
                    }
                }
            };
            let raw = located
                .as_ref()
                .map(|(path, _)| std::fs::read(vault.join(path)))
                .transpose()
                .map_err(|error| StoreError::Io(error.to_string()))?;
            let revision = revision(&session, &target, raw.as_deref())?;
            let document = match (located, raw) {
                (Some((_, legacy_id)), Some(raw)) => {
                    let raw = String::from_utf8(raw)
                        .map_err(|error| StoreError::Serialize(error.to_string()))?;
                    let parsed = match legacy_id {
                        Some(id) => hypr_vault_read::parse_enhanced_file(&id, &session, &raw)?,
                        None => hypr_vault_read::summary::as_document(&session, raw),
                    };
                    Some(match target {
                        DocumentTarget::Summary => {
                            hypr_vault_read::summary::as_document(&session, parsed.markdown)
                        }
                        DocumentTarget::TemplateOutput { .. }
                            if parsed.kind == "template_output" =>
                        {
                            parsed
                        }
                        DocumentTarget::TemplateOutput { .. } => {
                            return Err(StoreError::Conflict(
                                "document is a legacy summary; use the summary operation".into(),
                            ));
                        }
                    })
                }
                _ => None,
            };
            Ok(Some(DocumentSnapshot { document, revision }))
        })
        .await
        .map_err(|error| StoreError::Io(error.to_string()))?
    }
}

pub(super) fn revision(
    session: &str,
    target: &DocumentTarget,
    raw: Option<&[u8]>,
) -> Result<String, StoreError> {
    let mut bytes =
        serde_json::to_vec(&(session, target)).map_err(|e| StoreError::Serialize(e.to_string()))?;
    bytes.push(u8::from(raw.is_some()));
    if let Some(raw) = raw {
        bytes.extend_from_slice(raw);
    }
    Ok(crate::sha256(&bytes))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{EnhancedDoc, TaskInput};
    use serde_json::json;

    async fn fixture() -> (tempfile::TempDir, SessionStore) {
        let vault = tempfile::tempdir().unwrap();
        let store = SessionStore::new(vault.path().into());
        let meta = serde_json::from_value(
            json!({"id":"s1","title":"Meeting","created_at":"2026-11-05T10:00:00Z","tags":[]}),
        )
        .unwrap();
        store
            .create_note_session(&meta, "Original note")
            .await
            .unwrap();
        (vault, store)
    }

    #[tokio::test]
    async fn summary_creation_and_concurrent_replacements_use_one_canonical_revision() {
        let (vault, store) = fixture().await;
        let operation_storage = tempfile::tempdir().unwrap();
        let operation_root = operation_storage.path();
        let target = DocumentTarget::Summary;
        let absent = store
            .read_document_snapshot("s1", &target)
            .await
            .unwrap()
            .unwrap();
        assert!(absent.document.is_none());
        let (first, second) = tokio::join!(
            store.save_document_if_revision(
                operation_root,
                "s1",
                &target,
                "First",
                &absent.revision
            ),
            store.save_document_if_revision(
                operation_root,
                "s1",
                &target,
                "Second",
                &absent.revision
            ),
        );
        let outcomes = [first.unwrap(), second.unwrap()];
        assert_eq!(
            outcomes
                .iter()
                .filter(|r| matches!(r, DocumentSaveOutcome::Saved(_)))
                .count(),
            1
        );
        assert_eq!(
            outcomes
                .iter()
                .filter(|r| matches!(r, DocumentSaveOutcome::Conflict(_)))
                .count(),
            1
        );
        let current = store
            .read_document_snapshot("s1", &target)
            .await
            .unwrap()
            .unwrap();
        assert_ne!(current.revision, absent.revision);
        assert_eq!(
            std::fs::read_to_string(vault.path().join("sessions/s1/summary.md")).unwrap(),
            current.document.unwrap().markdown
        );
        assert_eq!(
            store.read_note("s1").await.unwrap().unwrap(),
            "Original note"
        );
    }

    #[tokio::test]
    async fn template_revisions_cover_raw_frontmatter_and_preserve_other_documents_and_task_metadata()
     {
        let (vault, store) = fixture().await;
        let operation_storage = tempfile::tempdir().unwrap();
        let operation_root = operation_storage.path();
        store
            .write_enhanced_doc(&EnhancedDoc {
                id: "output".into(),
                session_id: "s1".into(),
                kind: "template_output".into(),
                title: "Action items".into(),
                template_id: "actions".into(),
                sort_order: 3,
                markdown: "- [ ] Call Alice".into(),
            })
            .await
            .unwrap();
        store
            .replace_tasks(
                "enhanced_note",
                "output",
                vec![TaskInput {
                    id: "stable".into(),
                    source_order: 0,
                    status: "done".into(),
                    text: "Call Alice".into(),
                    body: json!([]),
                    due_at: "2026-12-01".into(),
                }],
            )
            .await
            .unwrap();
        let target = DocumentTarget::TemplateOutput {
            document_id: "output".into(),
        };
        let initial = store
            .read_document_snapshot("s1", &target)
            .await
            .unwrap()
            .unwrap();
        let file = vault.path().join("sessions/s1/enhanced/output.md");
        let mut raw = std::fs::read_to_string(&file).unwrap();
        raw = raw.replacen("---\n", "---\ncustom: external\n", 1);
        std::fs::write(&file, &raw).unwrap();
        assert!(matches!(
            store
                .save_document_if_revision(
                    operation_root,
                    "s1",
                    &target,
                    "stale",
                    &initial.revision
                )
                .await
                .unwrap(),
            DocumentSaveOutcome::Conflict(_)
        ));
        assert_eq!(std::fs::read_to_string(&file).unwrap(), raw);
        let current = store
            .read_document_snapshot("s1", &target)
            .await
            .unwrap()
            .unwrap();
        let DocumentSaveOutcome::Saved(saved) = store
            .save_document_if_revision(
                operation_root,
                "s1",
                &target,
                "New paragraph\n\n- [ ] Call Alice\n- [ ] New task",
                &current.revision,
            )
            .await
            .unwrap()
        else {
            panic!("save");
        };
        let doc = saved.document.unwrap();
        assert_eq!(
            (doc.title.as_str(), doc.template_id.as_str(), doc.sort_order),
            ("Action items", "actions", 3)
        );
        let tasks = store.session_tasks("s1").await.unwrap();
        assert_eq!(tasks.len(), 2);
        let previous = tasks.iter().find(|t| t.id == "stable").unwrap();
        assert_eq!(previous.due_at, "2026-12-01");
        assert_eq!(previous.status, "done");
        assert_eq!(
            store.read_note("s1").await.unwrap().unwrap(),
            "Original note"
        );
        assert!(!vault.path().join("sessions/s1/summary.md").exists());
        let DocumentSaveOutcome::Saved(checked) = store
            .save_document_if_revision(
                operation_root,
                "s1",
                &target,
                "New paragraph\n\n- [x] Call Alice\n- [ ] New task",
                &saved.revision,
            )
            .await
            .unwrap()
        else {
            panic!("checked");
        };
        store
            .save_document_if_revision(
                operation_root,
                "s1",
                &target,
                "New paragraph\n\n- [ ] Call Alice\n- [ ] New task",
                &checked.revision,
            )
            .await
            .unwrap();
        let tasks = store.session_tasks("s1").await.unwrap();
        let previous = tasks.iter().find(|t| t.id == "stable").unwrap();
        assert_eq!(previous.status, "todo");
        assert_eq!(previous.due_at, "2026-12-01");
    }

    #[tokio::test]
    async fn legacy_summary_migrates_through_existing_writer_and_stale_deleted_sessions_do_not_return()
     {
        let (vault, store) = fixture().await;
        let operation_storage = tempfile::tempdir().unwrap();
        let operation_root = operation_storage.path();
        store
            .write_enhanced_doc(&EnhancedDoc {
                id: "legacy".into(),
                session_id: "s1".into(),
                kind: "summary".into(),
                title: "Old".into(),
                template_id: "".into(),
                sort_order: 0,
                markdown: "Legacy".into(),
            })
            .await
            .unwrap();
        std::fs::write(
            vault.path().join("sessions/s1/unknown.md"),
            "User attachment",
        )
        .unwrap();
        let target = DocumentTarget::Summary;
        let old = store
            .read_document_snapshot("s1", &target)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(old.document.unwrap().id, "s1");
        assert!(matches!(
            store
                .save_document_if_revision(operation_root, "s1", &target, "Updated", &old.revision)
                .await
                .unwrap(),
            DocumentSaveOutcome::Saved(_)
        ));
        assert_eq!(
            std::fs::read_to_string(vault.path().join("sessions/s1/summary.md")).unwrap(),
            "Updated"
        );
        assert!(!vault.path().join("sessions/s1/enhanced/legacy.md").exists());
        assert_eq!(
            std::fs::read_to_string(vault.path().join("sessions/s1/unknown.md")).unwrap(),
            "User attachment"
        );
        std::fs::remove_file(vault.path().join("sessions/s1/_meta.json")).unwrap();
        assert!(
            store
                .save_document_if_revision(
                    operation_root,
                    "s1",
                    &target,
                    "Resurrected",
                    &old.revision
                )
                .await
                .is_err()
        );
        assert!(!vault.path().join("sessions/s1/_meta.json").exists());
    }

    #[tokio::test]
    async fn absent_outputs_invalid_ids_and_symlinks_cannot_be_written() {
        let (vault, store) = fixture().await;
        let operation_storage = tempfile::tempdir().unwrap();
        let operation_root = operation_storage.path();
        let target = DocumentTarget::TemplateOutput {
            document_id: "missing".into(),
        };
        let absent = store
            .read_document_snapshot("s1", &target)
            .await
            .unwrap()
            .unwrap();
        assert!(
            store
                .save_document_if_revision(
                    operation_root,
                    "s1",
                    &target,
                    "Must not create",
                    &absent.revision
                )
                .await
                .is_err()
        );
        assert!(
            store
                .read_document_snapshot("../secret", &target)
                .await
                .is_err()
        );
        assert!(
            store
                .read_document_snapshot(
                    "s1",
                    &DocumentTarget::TemplateOutput {
                        document_id: "../secret".into()
                    }
                )
                .await
                .is_err()
        );
        #[cfg(unix)]
        {
            let enhanced = vault.path().join("sessions/s1/enhanced");
            std::fs::create_dir_all(&enhanced).unwrap();
            let foreign = vault.path().join("foreign.md");
            std::fs::write(&foreign, "Private").unwrap();
            std::os::unix::fs::symlink(&foreign, enhanced.join("missing.md")).unwrap();
            assert!(store.read_document_snapshot("s1", &target).await.is_err());
            assert_eq!(std::fs::read_to_string(foreign).unwrap(), "Private");
        }
    }

    #[tokio::test]
    async fn damaged_tasks_block_body_replacement_and_summary_revision_excludes_unrelated_metadata()
    {
        let (vault, store) = fixture().await;
        let operation_storage = tempfile::tempdir().unwrap();
        let operation_root = operation_storage.path();
        store.ensure_summary("s1").await.unwrap();
        let target = DocumentTarget::Summary;
        let initial = store
            .read_document_snapshot("s1", &target)
            .await
            .unwrap()
            .unwrap();
        store
            .update_meta(
                "s1",
                crate::SessionMetaPatch {
                    title: Some("Renamed meeting".into()),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        assert_eq!(
            store
                .read_document_snapshot("s1", &target)
                .await
                .unwrap()
                .unwrap()
                .revision,
            initial.revision
        );
        std::fs::write(vault.path().join("sessions/s1/tasks.json"), "damaged").unwrap();
        assert!(
            store
                .save_document_if_revision(
                    operation_root,
                    "s1",
                    &target,
                    "Must not replace",
                    &initial.revision
                )
                .await
                .is_err()
        );
        assert_eq!(
            std::fs::read_to_string(vault.path().join("sessions/s1/summary.md")).unwrap(),
            ""
        );
        assert_eq!(
            std::fs::read_to_string(vault.path().join("sessions/s1/tasks.json")).unwrap(),
            "damaged"
        );
    }
}
