use std::{
    io::Read,
    path::{Component, Path, PathBuf},
    sync::Weak,
};

use serde::{Deserialize, Serialize};

use crate::{SessionStore, StoreError, WriteGuard, paths};

const MAX_DOCUMENTS: usize = 64;
const MAX_ENTRIES: usize = 4096;
const MAX_MATERIAL: usize = 16 * 1024 * 1024;
pub(super) const MAX_INTENT: usize = 32 * 1024 * 1024;

#[derive(Debug)]
pub enum GeneratedTitlePreparation {
    AlreadyNamed,
    Ready(GeneratedTitleBaseline),
}

pub struct GeneratedTitleBaseline {
    owner: Weak<tokio::sync::Mutex<()>>,
    vault: PathBuf,
    session_id: String,
    title: String,
    documents: Vec<CapturedDocument>,
    note: CapturedNote,
}

impl std::fmt::Debug for GeneratedTitleBaseline {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("GeneratedTitleBaseline")
            .field("session_id", &self.session_id)
            .field("documents", &self.documents.len())
            .finish_non_exhaustive()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GeneratedTitleOutcome {
    Empty,
    AlreadyNamed,
    Conflict,
    Applied { raw_note: RawNoteTitleOutcome },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RawNoteTitleOutcome {
    Applied,
    Unavailable,
    Absent,
    Empty,
    Changed,
}

#[derive(Debug)]
struct CapturedDocument {
    path: PathBuf,
    raw: String,
    document: Option<crate::EnhancedDoc>,
    stamp: bool,
}

#[derive(Debug)]
enum CapturedNote {
    Skip(RawNoteTitleOutcome),
    Ready { raw: String, legacy: bool },
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct TitleIntent {
    pub schema_version: u32,
    pub session_id: String,
    before_title: String,
    title: String,
    documents: Vec<TitleDocument>,
    note: NotePlan,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct TitleDocument {
    path: PathBuf,
    before_hash: String,
    before_bytes: usize,
    after: Option<String>,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum NotePlan {
    Skip {
        outcome: RawNoteTitleOutcome,
    },
    Stamp {
        before_hash: String,
        legacy: bool,
        after: String,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Stage {
    Intent,
    Document(usize),
    Metadata,
    RawNote,
    Synced,
}

impl SessionStore {
    pub async fn prepare_generated_title(
        &self,
        operation_root: &Path,
        session_id: &str,
    ) -> Result<GeneratedTitlePreparation, StoreError> {
        self.recovered_read(
            Some(operation_root),
            session_id,
            |store, snapshot, id| async move {
                store.ensure_ready()?;
                if store.is_recording(&id) {
                    return Err(StoreError::Conflict(
                        "recording is active; title preparation is unavailable".into(),
                    ));
                }
                let mut bounded = store.clone();
                bounded.content_limits.metadata_bytes =
                    bounded.content_limits.metadata_bytes.min(8 * 1024 * 1024);
                let meta = bounded
                    .metadata_snapshot_locked(&snapshot._guard, &id)
                    .await?
                    .ok_or_else(|| StoreError::Conflict("session was deleted".into()))?;
                if !meta.meta.title.trim().is_empty() {
                    return Ok(GeneratedTitlePreparation::AlreadyNamed);
                }
                let directory = store.vault_base.join(paths::validated_session_dir(&id)?);
                let read_id = id.clone();
                let limit = store.content_limits.note_read_bytes.min(MAX_MATERIAL);
                let (documents, note) = store
                    .spawn_blocking(move || {
                        let documents = capture_documents(&directory, &read_id, limit)?;
                        let used = documents.iter().map(|doc| doc.raw.len()).sum::<usize>();
                        let note =
                            capture_note(&directory, limit.min(MAX_MATERIAL.saturating_sub(used)));
                        Ok::<_, StoreError>((documents, note))
                    })
                    .await
                    .map_err(worker_error)??;
                Ok(GeneratedTitlePreparation::Ready(GeneratedTitleBaseline {
                    owner: std::sync::Arc::downgrade(&store.write_lock),
                    vault: store.vault_base.clone(),
                    session_id: id,
                    title: meta.meta.title,
                    documents,
                    note,
                }))
            },
        )
        .await
    }

    pub async fn apply_generated_title(
        &self,
        operation_root: &Path,
        baseline: GeneratedTitleBaseline,
        candidate: String,
    ) -> Result<GeneratedTitleOutcome, StoreError> {
        self.apply_generated_title_with(operation_root, baseline, candidate, |_| async { Ok(()) })
            .await
    }

    async fn apply_generated_title_with<C, F>(
        &self,
        operation_root: &Path,
        baseline: GeneratedTitleBaseline,
        candidate: String,
        mut checkpoint: C,
    ) -> Result<GeneratedTitleOutcome, StoreError>
    where
        C: FnMut(Stage) -> F + Send + 'static,
        F: std::future::Future<Output = Result<(), StoreError>> + Send + 'static,
    {
        if baseline.vault != self.vault_base
            || !Weak::ptr_eq(
                &baseline.owner,
                &std::sync::Arc::downgrade(&self.write_lock),
            )
        {
            return Err(StoreError::Serialize(
                "Title baseline belongs to another vault owner".into(),
            ));
        }
        let title = candidate.trim().to_owned();
        if title.is_empty() || title == "<EMPTY>" {
            return Ok(GeneratedTitleOutcome::Empty);
        }
        validate_candidate(&title, self.content_limits.title_bytes)?;
        let id = baseline.session_id.clone();
        let root = operation_root.to_path_buf();
        self.recovered_read(
            Some(operation_root),
            &id,
            move |store, snapshot, id| async move {
                store.ensure_ready()?;
                if store.is_recording(&id) {
                    return Err(StoreError::Conflict(
                        "recording is active; generated title cannot be applied".into(),
                    ));
                }
                let guard = &snapshot._guard;
                let mut bounded = store.clone();
                bounded.content_limits.metadata_bytes =
                    bounded.content_limits.metadata_bytes.min(8 * 1024 * 1024);
                let (metadata, raw) = bounded
                    .metadata_with_raw_locked(guard, &id)
                    .await?
                    .ok_or_else(|| StoreError::Conflict("session was deleted".into()))?;
                let meta = metadata.meta;
                if !meta.title.trim().is_empty() {
                    return Ok(GeneratedTitleOutcome::AlreadyNamed);
                }
                if meta.title != baseline.title {
                    return Ok(GeneratedTitleOutcome::Conflict);
                }
                crate::metadata_revision::serialize_title_patch(
                    raw,
                    &title,
                    bounded.content_limits.metadata_bytes,
                )?;
                let directory = store.vault_base.join(paths::validated_session_dir(&id)?);
                let plan_directory = directory.clone();
                let read_id = id.clone();
                let limit = store.content_limits.note_read_bytes.min(MAX_MATERIAL);
                let write_limit = store.content_limits.note_write_bytes.min(MAX_MATERIAL);
                let planned = store
                    .spawn_blocking(move || {
                        let fresh = capture_documents(&plan_directory, &read_id, limit)?;
                        if !same_documents(&baseline.documents, &fresh) {
                            return Ok(None);
                        }
                        make_intent(baseline, title, &plan_directory, write_limit).map(Some)
                    })
                    .await
                    .map_err(worker_error)??;
                let Some(mut intent) = planned else {
                    return Ok(GeneratedTitleOutcome::Conflict);
                };
                let pending = store
                    .begin_title_write_locked(guard, &root, intent.clone())
                    .await?;
                checkpoint(Stage::Intent).await?;
                let result = store
                    .finish_title_locked(guard, &mut intent, pending.path(), &mut checkpoint)
                    .await;
                if result.is_err() {
                    for entity in [
                        crate::IndexEntity::Sessions,
                        crate::IndexEntity::SessionHeaders,
                        crate::IndexEntity::Docs,
                    ] {
                        store.notify_index_changed(entity, vec![id.clone()]);
                    }
                }
                let outcome = result.map_err(StoreError::after_commit)?;
                pending.finish().await.map_err(StoreError::after_commit)?;
                Ok(GeneratedTitleOutcome::Applied { raw_note: outcome })
            },
        )
        .await
    }

    pub(super) async fn recover_generated_title_locked(
        &self,
        guard: &WriteGuard<'_>,
        path: &Path,
        mut intent: TitleIntent,
    ) -> Result<(), StoreError> {
        self.finish_title_locked(guard, &mut intent, path, &mut |_| async { Ok(()) })
            .await?;
        crate::document_operation::finish_title_intent(self, path).await
    }

    async fn finish_title_locked<C, F>(
        &self,
        guard: &WriteGuard<'_>,
        intent: &mut TitleIntent,
        intent_path: &Path,
        checkpoint: &mut C,
    ) -> Result<RawNoteTitleOutcome, StoreError>
    where
        C: FnMut(Stage) -> F + Send,
        F: std::future::Future<Output = Result<(), StoreError>> + Send,
    {
        validate_intent(
            intent,
            self.content_limits.title_bytes,
            self.content_limits.note_write_bytes,
        )?;
        if self.is_recording(&intent.session_id) {
            return Err(StoreError::Conflict(
                "recording is active; title recovery requires review".into(),
            ));
        }
        self.preflight_title_locked(guard, intent).await?;
        let directory = paths::validated_session_dir(&intent.session_id)?;
        for (index, document) in intent.documents.iter().enumerate() {
            let Some(after) = &document.after else {
                continue;
            };
            let path = self.vault_base.join(&directory).join(&document.path);
            let check = document.clone();
            let current = self
                .spawn_blocking(move || {
                    let raw = read_file(&path, MAX_MATERIAL)?
                        .ok_or_else(|| StoreError::Conflict("title output was deleted".into()))?;
                    let hash = crate::sha256(raw.as_bytes());
                    if hash != check.before_hash
                        && hash != crate::sha256(check.after.as_ref().unwrap().as_bytes())
                    {
                        return Err(StoreError::Conflict(
                            "title output changed; recovery requires review".into(),
                        ));
                    }
                    Ok::<_, StoreError>(raw)
                })
                .await
                .map_err(worker_error)??;
            if current != *after {
                self.write_file_locked(
                    guard,
                    directory.join(&document.path),
                    after.as_bytes().to_vec(),
                )
                .await?;
            }
            self.index_title_document(&intent.session_id, &document.path, after)?;
            checkpoint(Stage::Document(index)).await?;
        }
        self.preflight_title_locked(guard, intent).await?;
        let mut bounded = self.clone();
        bounded.content_limits.metadata_bytes =
            bounded.content_limits.metadata_bytes.min(8 * 1024 * 1024);
        let (metadata, raw) = bounded
            .metadata_with_raw_locked(guard, &intent.session_id)
            .await?
            .ok_or_else(|| StoreError::Conflict("session was deleted".into()))?;
        let mut meta = metadata.meta;
        if meta.title != intent.before_title && meta.title != intent.title {
            return Err(StoreError::Conflict(
                "session title changed; recovery requires review".into(),
            ));
        }
        if meta.title != intent.title {
            let bytes = crate::metadata_revision::serialize_title_patch(
                raw,
                &intent.title,
                bounded.content_limits.metadata_bytes,
            )?;
            self.write_file_locked(guard, paths::meta_path_in(&directory), bytes)
                .await?;
            meta.title = intent.title.clone();
            self.index_upsert_meta(&meta);
            self.notify_index_changed(
                crate::IndexEntity::Sessions,
                vec![intent.session_id.clone()],
            );
        } else {
            self.index_upsert_meta(&meta);
        }
        checkpoint(Stage::Metadata).await?;
        let raw_note = self.finish_title_note_locked(guard, intent).await?;
        if matches!(intent.note, NotePlan::Stamp { .. }) && raw_note != RawNoteTitleOutcome::Applied
        {
            intent.note = NotePlan::Skip { outcome: raw_note };
            let record = intent.clone();
            let path = intent_path.to_owned();
            self.spawn_blocking(move || crate::operation_io::persist(&path, &record))
                .await
                .map_err(worker_error)??;
        }
        checkpoint(Stage::RawNote).await?;
        let vault = self.vault_base.clone();
        let paths = intent
            .documents
            .iter()
            .filter(|doc| doc.after.is_some())
            .map(|doc| vault.join(&directory).join(&doc.path))
            .chain([
                vault.join(paths::meta_path_in(&directory)),
                vault.join(paths::note_path_in(&directory)),
            ])
            .collect::<Vec<_>>();
        self.spawn_blocking(move || {
            for path in paths {
                crate::operation_io::sync_parent(&path)?;
            }
            Ok::<_, StoreError>(())
        })
        .await
        .map_err(worker_error)??;
        checkpoint(Stage::Synced).await?;
        Ok(raw_note)
    }

    async fn preflight_title_locked(
        &self,
        guard: &WriteGuard<'_>,
        intent: &TitleIntent,
    ) -> Result<(), StoreError> {
        let mut bounded = self.clone();
        bounded.content_limits.metadata_bytes =
            bounded.content_limits.metadata_bytes.min(8 * 1024 * 1024);
        let (metadata, raw) = bounded
            .metadata_with_raw_locked(guard, &intent.session_id)
            .await?
            .ok_or_else(|| StoreError::Conflict("session was deleted".into()))?;
        let meta = metadata.meta;
        if meta.title != intent.before_title && meta.title != intent.title {
            return Err(StoreError::Conflict(
                "session title changed; recovery requires review".into(),
            ));
        }
        if meta.title != intent.title {
            crate::metadata_revision::serialize_title_patch(
                raw,
                &intent.title,
                bounded.content_limits.metadata_bytes,
            )?;
        }
        let directory = self
            .vault_base
            .join(paths::validated_session_dir(&intent.session_id)?);
        let id = intent.session_id.clone();
        let expected = intent.documents.clone();
        self.spawn_blocking(move || {
            let actual = capture_documents(&directory, &id, MAX_MATERIAL)?;
            if actual.len() != expected.len()
                || actual
                    .iter()
                    .zip(expected.iter())
                    .any(|(actual, expected)| {
                        actual.path != expected.path
                            || (crate::sha256(actual.raw.as_bytes()) != expected.before_hash
                                && expected.after.as_deref() != Some(actual.raw.as_str()))
                    })
            {
                return Err(StoreError::Conflict(
                    "title outputs changed; recovery requires review".into(),
                ));
            }
            Ok(())
        })
        .await
        .map_err(worker_error)?
    }

    async fn finish_title_note_locked(
        &self,
        guard: &WriteGuard<'_>,
        intent: &TitleIntent,
    ) -> Result<RawNoteTitleOutcome, StoreError> {
        let NotePlan::Stamp {
            before_hash,
            legacy,
            after,
        } = &intent.note
        else {
            let NotePlan::Skip { outcome } = intent.note else {
                unreachable!()
            };
            return Ok(outcome);
        };
        let directory = paths::validated_session_dir(&intent.session_id)?;
        let path = self.vault_base.join(&directory);
        let captured = match self
            .spawn_blocking(move || capture_note(&path, MAX_MATERIAL))
            .await
        {
            Ok(value) => value,
            Err(_) => return Ok(RawNoteTitleOutcome::Unavailable),
        };
        match captured {
            CapturedNote::Ready {
                raw,
                legacy: current_legacy,
            } if !current_legacy && raw == *after => {}
            CapturedNote::Ready {
                raw,
                legacy: current_legacy,
            } if current_legacy == *legacy && crate::sha256(raw.as_bytes()) == *before_hash => {
                self.write_note_file_locked(guard, &intent.session_id, after)
                    .await?;
            }
            CapturedNote::Skip(RawNoteTitleOutcome::Unavailable) => {
                return Ok(RawNoteTitleOutcome::Unavailable);
            }
            _ => return Ok(RawNoteTitleOutcome::Changed),
        }
        self.index_set_note(
            &intent.session_id,
            Some(crate::strip_leading_frontmatter(after.clone())),
        );
        Ok(RawNoteTitleOutcome::Applied)
    }

    fn index_title_document(&self, id: &str, path: &Path, raw: &str) -> Result<(), StoreError> {
        if path == Path::new("summary.md") {
            self.index_set_summary(id, Some(raw.into()));
        } else {
            let document_id = path
                .file_stem()
                .and_then(|stem| stem.to_str())
                .ok_or_else(|| StoreError::Serialize("Invalid output ID".into()))?;
            let document = hypr_vault_read::parse_enhanced_file(document_id, id, raw)?;
            if document.kind == "summary" {
                self.index_set_summary(id, Some(document.markdown));
            } else {
                self.index_upsert_doc(&document);
                self.notify_index_changed(crate::IndexEntity::Docs, vec![id.into()]);
            }
        }
        Ok(())
    }
}

fn validate_candidate(title: &str, limit: usize) -> Result<(), StoreError> {
    if title.len() > limit.min(4096) {
        return Err(StoreError::ResourceLimit(
            "Generated title exceeds its size limit".into(),
        ));
    }
    if title.is_empty() || title == "<EMPTY>" || title.chars().any(char::is_control) {
        return Err(StoreError::Serialize("Invalid generated title".into()));
    }
    Ok(())
}

fn same_documents(left: &[CapturedDocument], right: &[CapturedDocument]) -> bool {
    left.len() == right.len()
        && left
            .iter()
            .zip(right)
            .all(|(left, right)| left.path == right.path && left.raw == right.raw)
}

fn make_intent(
    baseline: GeneratedTitleBaseline,
    title: String,
    directory: &Path,
    write_limit: usize,
) -> Result<TitleIntent, StoreError> {
    let mut used = 0usize;
    let mut documents = Vec::new();
    for captured in baseline.documents {
        let after = if captured.stamp {
            let markdown = captured
                .document
                .as_ref()
                .map(|doc| doc.markdown.as_str())
                .unwrap_or(&captured.raw);
            let stamped = stamp_title(markdown, &title);
            let raw = if let Some(document) = &captured.document {
                crate::enhanced::replace_body_preserving_frontmatter(
                    &captured.raw,
                    document,
                    &stamped,
                )?
            } else {
                stamped
            };
            if raw.len() > write_limit {
                return Err(StoreError::ResourceLimit(
                    "Titled output exceeds its size limit".into(),
                ));
            }
            used = used.saturating_add(raw.len());
            Some(raw)
        } else {
            used = used.saturating_add(captured.raw.len());
            None
        };
        documents.push(TitleDocument {
            path: captured.path,
            before_hash: crate::sha256(captured.raw.as_bytes()),
            before_bytes: captured.raw.len(),
            after,
        });
    }
    if used > MAX_MATERIAL {
        return Err(StoreError::ResourceLimit(
            "Title outputs exceed the aggregate limit".into(),
        ));
    }
    let note = match baseline.note {
        CapturedNote::Skip(outcome) => NotePlan::Skip { outcome },
        CapturedNote::Ready { raw, legacy } => {
            let current = capture_note(directory, MAX_MATERIAL.saturating_sub(used));
            if !matches!(current, CapturedNote::Ready { raw: ref value, legacy: current_legacy } if *value == raw && current_legacy == legacy)
            {
                NotePlan::Skip {
                    outcome: if matches!(
                        current,
                        CapturedNote::Skip(RawNoteTitleOutcome::Unavailable)
                    ) {
                        RawNoteTitleOutcome::Unavailable
                    } else {
                        RawNoteTitleOutcome::Changed
                    },
                }
            } else {
                let content = crate::strip_leading_frontmatter(raw.clone());
                let stamped = stamp_title(&content, &title);
                let after = raw
                    .strip_suffix(&content)
                    .map(|prefix| format!("{prefix}{stamped}"));
                match after {
                    Some(after)
                        if after.len() <= write_limit
                            && after.len() <= MAX_MATERIAL.saturating_sub(used) =>
                    {
                        NotePlan::Stamp {
                            before_hash: crate::sha256(raw.as_bytes()),
                            legacy,
                            after,
                        }
                    }
                    _ => NotePlan::Skip {
                        outcome: RawNoteTitleOutcome::Unavailable,
                    },
                }
            }
        }
    };
    Ok(TitleIntent {
        schema_version: 4,
        session_id: baseline.session_id,
        before_title: baseline.title,
        title,
        documents,
        note,
    })
}

impl SessionStore {
    pub(super) async fn prepare_generated_markdown(
        &self,
        operation_root: &Path,
        session_id: &str,
        markdown: &str,
    ) -> Result<(String, String, serde_json::Value), StoreError> {
        let title = self
            .read_recovered_metadata_snapshot(Some(operation_root), session_id)
            .await?
            .ok_or_else(|| StoreError::Conflict("session was deleted".into()))?
            .meta
            .title;
        let markdown = if title.trim().is_empty() {
            markdown.to_owned()
        } else {
            stamp_title(markdown, &title)
        };
        if markdown.len() > self.content_limits.note_write_bytes {
            return Err(StoreError::ResourceLimit(
                "Generated document including session title is too large".into(),
            ));
        }
        let parse = markdown.clone();
        let content = self
            .spawn_blocking(move || hypr_tiptap::md_to_tiptap_json(&parse))
            .await
            .map_err(|cause| StoreError::Io(cause.to_string()))?
            .map_err(StoreError::Serialize)?;
        Ok((title, markdown, content))
    }
}

pub(super) fn stamp_title(markdown: &str, title: &str) -> String {
    let title = title.trim();
    let content = markdown.trim_start();
    if content.lines().next() == Some(&format!("# {title}")) {
        return markdown.into();
    }
    format!("# {title}\n\n{content}").trim().into()
}

fn capture_note(directory: &Path, limit: usize) -> CapturedNote {
    for (name, legacy) in [("notes.md", false), ("_memo.md", true)] {
        match read_file(&directory.join(name), limit) {
            Ok(Some(raw)) => {
                if crate::strip_leading_frontmatter(raw.clone())
                    .trim()
                    .is_empty()
                {
                    return CapturedNote::Skip(RawNoteTitleOutcome::Empty);
                }
                return CapturedNote::Ready { raw, legacy };
            }
            Ok(None) => {}
            Err(_) => return CapturedNote::Skip(RawNoteTitleOutcome::Unavailable),
        }
    }
    CapturedNote::Skip(RawNoteTitleOutcome::Absent)
}

fn capture_documents(
    directory: &Path,
    id: &str,
    limit: usize,
) -> Result<Vec<CapturedDocument>, StoreError> {
    let mut documents = Vec::new();
    let mut used = 0usize;
    if let Some(raw) = read_file(&directory.join("summary.md"), limit)? {
        used += raw.len();
        documents.push(CapturedDocument {
            path: "summary.md".into(),
            stamp: !raw.trim().is_empty(),
            raw,
            document: None,
        });
    }
    let enhanced = directory.join("enhanced");
    if regular_directory(&enhanced)? {
        for (count, entry) in std::fs::read_dir(&enhanced).map_err(io_error)?.enumerate() {
            if count >= MAX_ENTRIES {
                return Err(StoreError::ResourceLimit(
                    "Title directory exceeds its inspection limit".into(),
                ));
            }
            let entry = entry.map_err(io_error)?;
            let name = entry.file_name();
            let path = Path::new(&name);
            let Some(stem) = path.file_stem().and_then(|stem| stem.to_str()) else {
                continue;
            };
            if stem.starts_with('.')
                || stem.contains(".conflict-")
                || path.extension().and_then(|ext| ext.to_str()) != Some("md")
            {
                continue;
            }
            crate::validate_session_id(stem)?;
            if documents.len() >= MAX_DOCUMENTS {
                return Err(StoreError::ResourceLimit("Too many title outputs".into()));
            }
            let raw = read_file(&entry.path(), limit.min(MAX_MATERIAL.saturating_sub(used)))?
                .ok_or_else(|| StoreError::Conflict("Title output disappeared".into()))?;
            used += raw.len();
            if used > MAX_MATERIAL {
                return Err(StoreError::ResourceLimit(
                    "Title outputs exceed the aggregate limit".into(),
                ));
            }
            let document = hypr_vault_read::parse_enhanced_file(stem, id, &raw)?;
            documents.push(CapturedDocument {
                path: Path::new("enhanced").join(name),
                stamp: !document.markdown.trim().is_empty(),
                raw,
                document: Some(document),
            });
        }
    }
    let summaries = documents
        .iter()
        .filter_map(|doc| doc.document.as_ref())
        .filter(|doc| doc.kind == "summary")
        .collect::<Vec<_>>();
    if summaries.len() > 1 {
        return Err(StoreError::Conflict(
            "multiple legacy summaries; files left unchanged".into(),
        ));
    }
    let legacy = summaries.first().map(|doc| doc.id.clone());
    if let Some(legacy) = legacy {
        for doc in &mut documents {
            if doc
                .document
                .as_ref()
                .is_some_and(|value| value.kind == "summary" && value.id != legacy)
                || doc.document.is_none()
            {
                doc.stamp = false;
            }
        }
    }
    documents.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(documents)
}

fn read_file(path: &Path, limit: usize) -> Result<Option<String>, StoreError> {
    let Some(file) = crate::media::open_regular_file(path)? else {
        return Ok(None);
    };
    if file.metadata().map_err(io_error)?.len() > limit as u64 {
        return Err(StoreError::ResourceLimit(
            "Title source exceeds its inspection limit".into(),
        ));
    }
    let mut bytes = Vec::new();
    file.take(limit.saturating_add(1) as u64)
        .read_to_end(&mut bytes)
        .map_err(io_error)?;
    if bytes.len() > limit {
        return Err(StoreError::ResourceLimit(
            "Title source exceeds its inspection limit".into(),
        ));
    }
    String::from_utf8(bytes)
        .map(Some)
        .map_err(|cause| StoreError::Serialize(cause.to_string()))
}
fn regular_directory(path: &Path) -> Result<bool, StoreError> {
    match path.symlink_metadata() {
        Ok(meta) if meta.is_dir() && !meta.file_type().is_symlink() => Ok(true),
        Ok(_) => Err(StoreError::Io("Title directory is not regular".into())),
        Err(cause) if cause.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(cause) => Err(io_error(cause)),
    }
}
fn validate_intent(
    intent: &TitleIntent,
    title_limit: usize,
    write_limit: usize,
) -> Result<(), StoreError> {
    if intent.schema_version != 4
        || !intent.before_title.trim().is_empty()
        || intent.documents.len() > MAX_DOCUMENTS
    {
        return Err(StoreError::Serialize("Invalid title recovery plan".into()));
    }
    crate::validate_session_id(&intent.session_id)?;
    validate_candidate(&intent.title, title_limit)?;
    let mut seen = std::collections::HashSet::new();
    let mut used = 0usize;
    for doc in &intent.documents {
        let parts = doc.path.components().collect::<Vec<_>>();
        let safe = match parts.as_slice() {
            [Component::Normal(name)] => *name == "summary.md",
            [Component::Normal(parent), Component::Normal(name)] if *parent == "enhanced" => {
                Path::new(name).extension().is_some_and(|ext| ext == "md")
                    && Path::new(name)
                        .file_stem()
                        .and_then(|id| id.to_str())
                        .is_some_and(|id| {
                            crate::validate_session_id(id).is_ok() && !id.contains(".conflict-")
                        })
            }
            _ => false,
        };
        if !safe
            || !seen.insert(&doc.path)
            || !is_hash(&doc.before_hash)
            || doc.before_bytes > MAX_MATERIAL
        {
            return Err(StoreError::Serialize("Invalid title output guard".into()));
        }
        if let Some(after) = &doc.after {
            if after.len() > write_limit.min(MAX_MATERIAL) {
                return Err(StoreError::ResourceLimit(
                    "Titled output exceeds its size limit".into(),
                ));
            }
            used = used.saturating_add(after.len());
        } else {
            used = used.saturating_add(doc.before_bytes);
        }
    }
    if let NotePlan::Stamp {
        before_hash, after, ..
    } = &intent.note
    {
        if !is_hash(before_hash) || after.len() > write_limit.min(MAX_MATERIAL) {
            return Err(StoreError::Serialize("Invalid title note guard".into()));
        }
        used = used.saturating_add(after.len());
    }
    if matches!(
        intent.note,
        NotePlan::Skip {
            outcome: RawNoteTitleOutcome::Applied
        }
    ) {
        return Err(StoreError::Serialize("Invalid skipped note outcome".into()));
    }
    if used > MAX_MATERIAL {
        return Err(StoreError::ResourceLimit(
            "Title plan exceeds the aggregate limit".into(),
        ));
    }
    Ok(())
}
fn is_hash(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}
fn io_error(cause: impl std::fmt::Display) -> StoreError {
    StoreError::Io(cause.to_string())
}
fn worker_error(cause: tokio::task::JoinError) -> StoreError {
    StoreError::Io(format!("Title worker stopped: {cause}"))
}

#[cfg(test)]
mod tests;
