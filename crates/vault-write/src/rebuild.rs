use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use super::index::{
    IndexEntity, SessionEntry, VAULT_TASKS_KEY, apply_map_value, apply_transcript_summary,
};
use super::{SessionMeta, SessionStore, StoreError, paths};

type DiscoveryProgress = std::sync::Arc<dyn Fn(usize) + Send + Sync>;
type RebuildProgress = std::sync::Arc<dyn Fn(usize, usize) + Send + Sync>;

/// Bounded fan-out for the rebuild content refresh. Every read inside `refresh_one`
/// is `spawn_blocking`, so even the single-threaded startup runtime gets real
/// parallel I/O; 8 matches the search projection's batch size and stays far below
/// blocking-pool/file-handle concerns.
const REBUILD_CONCURRENCY: usize = 8;

/// Summary of a `rebuild_index`/`refresh_session` pass. Counts reflect entries *derived
/// from files* this pass, not the resulting index size. `errors` never aborts the scan --
/// an unparseable file is logged here and its existing index entry is left untouched (see
/// the hard rule in each match arm below: corruption must never look like deletion).
#[derive(Debug, Default, Clone, PartialEq, serde::Serialize, specta::Type)]
pub struct RebuildReport {
    pub sessions: usize,
    /// Documents read this pass -- the note (`notes.md`, or the pre-rename `_memo.md`)
    /// and every `enhanced/<doc_id>.md` doc, not just the note.
    pub notes: usize,
    pub transcripts: usize,
    /// Folder ids that have at least one recognized content file (a `<kind>.md` document or
    /// `transcript.json`) but no `_meta.json` -- left deliberately unindexed; files untouched.
    pub ghost_sessions: Vec<String>,
    pub errors: Vec<String>,
}

impl SessionStore {
    /// One-way: scan sessions/*/ -> reconcile the in-memory index; drop index entries whose
    /// folder is gone. Never writes to the vault -- read-only on the filesystem, write-only
    /// on the index. Also reconciles the vault-root tasks file.
    /// Only the entities whose value actually changed are notified onto the change bus
    /// (`PartialEq` diff), so the automatic startup/focus rescans stay silent over
    /// unchanged files -- the search projection and the frontend both ride that bus, and
    /// a no-op rescan must not re-trigger either.
    pub async fn rebuild_index(&self) -> Result<RebuildReport, StoreError> {
        // The scan and the catalog swap run under the store write lock: renames
        // (provisional reconcile, migration) also hold it, so the swap can never
        // revert the catalog to a directory a concurrent rename just moved away
        // from -- which would make the next write recreate the old path.
        let guard = self.lock_writes().await;
        let scan = self.scan_session_locations().await?;
        self.rebuild_with_scan(guard, scan, None).await
    }

    /// Startup entry point: rebuild from the layout snapshot
    /// `normalize_startup_layout` already paid for, instead of scanning again.
    pub async fn rebuild_index_from_startup_layout(
        &self,
        layout: super::StartupLayout,
    ) -> Result<RebuildReport, StoreError> {
        let guard = self.lock_writes().await;
        self.rebuild_with_scan(guard, layout.scan, None).await
    }

    pub async fn rebuild_index_from_startup_layout_with_progress(
        &self,
        layout: super::StartupLayout,
        on_progress: impl Fn(usize, usize) + Send + Sync + 'static,
    ) -> Result<RebuildReport, StoreError> {
        let guard = self.lock_writes().await;
        self.rebuild_with_scan(guard, layout.scan, Some(std::sync::Arc::new(on_progress)))
            .await
    }

    /// Startup layout normalization from ONE discovery walk: legacy readable-name
    /// migration, then provisional-title reconciliation (crash recovery), each
    /// updating the snapshot's paths after a successful rename so the rebuild that
    /// follows indexes the directories as they now are. Runs under the write lock;
    /// no content is read here -- that stays in the rebuild, outside the lock.
    pub async fn normalize_startup_layout(&self) -> Result<super::StartupLayout, StoreError> {
        self.normalize_startup_layout_with_progress(|_| {}).await
    }

    pub async fn normalize_startup_layout_with_progress(
        &self,
        on_sessions_found: impl Fn(usize) + Send + Sync + 'static,
    ) -> Result<super::StartupLayout, StoreError> {
        let guard = self.lock_writes().await;
        let mut scan = self
            .scan_session_locations_with_progress(Some(std::sync::Arc::new(on_sessions_found)))
            .await?;
        let migration = self.migrate_from_scan(&guard, &mut scan).await;
        self.reconcile_provisional_from_scan(&guard, &mut scan)
            .await;
        drop(guard);
        Ok(super::StartupLayout { scan, migration })
    }

    /// The rebuild body shared by every entry point. Takes the write guard by value:
    /// the layout swap runs under it, then it is released before the content refresh
    /// (reading every note/transcript/doc must not block writers).
    async fn rebuild_with_scan(
        &self,
        guard: super::WriteGuard<'_>,
        scan: SessionLayoutScan,
        on_progress: Option<RebuildProgress>,
    ) -> Result<RebuildReport, StoreError> {
        let mut report = RebuildReport::default();

        // Ids the prune below must not touch: an id claimed by multiple directories
        // is ambiguous (not gone), and an id whose known directory -- or any parent
        // of it -- is now corrupt/unreadable is broken (not gone). Descendant
        // matching matters for the unreadable case: a permission error on a personal
        // folder must not make every session homed under it look deleted.
        let protected: HashSet<String> = {
            let catalog = self.locations.read().unwrap();
            catalog
                .iter()
                .filter(|(_, dir)| {
                    scan.broken_dirs
                        .iter()
                        .any(|broken| hypr_vault_read::layout::path_starts_with_nfc(dir, broken))
                })
                .map(|(id, _)| id.clone())
                .chain(scan.duplicate_ids.iter().cloned())
                .collect()
        };

        // Refresh the location catalog from discovery before reconciling any content, so
        // every per-session read below resolves against the physical layout just scanned.
        // Corrupt-protected ids keep their previous location (their directory is still
        // there, just unreadable); duplicated ids are dropped so writes block instead of
        // silently picking one claimant.
        let location_changes: Vec<String> = {
            let mut catalog = self.locations.write().unwrap();
            let previous = std::mem::take(&mut *catalog);
            for (location, _) in &scan.sessions {
                catalog.insert(location.id.clone(), location.relative_dir.clone());
            }
            for id in &protected {
                if scan.duplicate_ids.contains(id) {
                    continue;
                }
                if let Some(dir) = previous.get(id) {
                    catalog.entry(id.clone()).or_insert_with(|| dir.clone());
                }
            }
            // Ids whose physical directory this swap changed, added, or removed --
            // announced below as `Locations` so path caches invalidate off external
            // renames/moves too, not just app-driven ones.
            previous
                .keys()
                .chain(catalog.keys())
                .filter(|id| match (previous.get(*id), catalog.get(*id)) {
                    (Some(a), Some(b)) => !hypr_vault_read::layout::paths_eq_nfc(a, b),
                    (None, None) => false,
                    _ => true,
                })
                .cloned()
                .collect::<HashSet<String>>()
                .into_iter()
                .collect()
        };
        // Duplicate claims persist past the rebuild so lazy per-id resolution can't
        // sidestep them (find_session's legacy fast path would otherwise silently
        // pick the canonical claimant and let writes diverge the copies).
        *self.known_duplicates.write().unwrap() = scan.duplicate_ids.iter().cloned().collect();
        drop(guard);

        self.notify_index_changed(IndexEntity::Locations, location_changes);

        report.errors.extend(scan.errors.iter().cloned());
        report.ghost_sessions = scan.ghost_dirs.clone();

        // Per-session refresh with bounded concurrency. Each task gets the meta the
        // discovery walk already parsed (skipping a re-read of `_meta.json`) and its
        // own sub-report; sub-reports are merged in scan order so `RebuildReport`
        // stays deterministic regardless of completion order. A session deleted
        // between scan and refresh is indexed from its scan-time meta until the next
        // rescan -- the same brief-staleness window the serial loop had, just wider.
        let mut slots: Vec<Option<(RebuildReport, Result<Option<StoreError>, StoreError>)>> =
            (0..scan.sessions.len()).map(|_| None).collect();
        let mut join_failure: Option<StoreError> = None;
        {
            let total = scan.sessions.len();
            let mut completed = 0usize;
            let mut join_set = tokio::task::JoinSet::new();
            let mut next = 0usize;
            while next < scan.sessions.len() || !join_set.is_empty() {
                while next < scan.sessions.len() && join_set.len() < REBUILD_CONCURRENCY {
                    let (location, meta) = &scan.sessions[next];
                    let store = self.clone();
                    let id = location.id.clone();
                    let meta = meta.clone();
                    let pos = next;
                    join_set.spawn(async move {
                        let mut sub = RebuildReport::default();
                        let outcome = store.refresh_one(&id, Some(meta), &mut sub).await;
                        (pos, sub, outcome)
                    });
                    next += 1;
                }
                match join_set.join_next().await {
                    Some(Ok((pos, sub, outcome))) => {
                        slots[pos] = Some((sub, outcome));
                        completed += 1;
                        if let Some(on_progress) = &on_progress {
                            on_progress(completed, total);
                        }
                    }
                    Some(Err(e)) => {
                        completed += 1;
                        if let Some(on_progress) = &on_progress {
                            on_progress(completed, total);
                        }
                        if join_failure.is_none() {
                            join_failure = Some(StoreError::Io(format!("task join error: {e}")));
                        }
                    }
                    None => {}
                }
            }
        }
        // Outer per-session errors (task-join failures inside refresh_one) no longer
        // abort the loop mid-way: every session still refreshes and the prune below
        // still runs (strictly more convergent -- refresh is idempotent), and the
        // first such error is returned at the end.
        let mut first_outer: Option<StoreError> = join_failure;
        for slot in slots {
            let Some((sub, outcome)) = slot else { continue };
            report.sessions += sub.sessions;
            report.notes += sub.notes;
            report.transcripts += sub.transcripts;
            report.ghost_sessions.extend(sub.ghost_sessions);
            report.errors.extend(sub.errors);
            if let Err(e) = outcome {
                report.errors.push(format!("session refresh aborted: {e}"));
                if first_outer.is_none() {
                    first_outer = Some(e);
                }
            }
        }

        // Sessions that vanished from disk are removed (the discovery scan succeeded and
        // came back without them -- the only certainty a prune is allowed to act on;
        // ambiguous/corrupt ids are protected above).
        let present: HashSet<&str> = scan
            .sessions
            .iter()
            .map(|(location, _)| location.id.as_str())
            .collect();
        let stale: Vec<String> = {
            let index = self.index.read().unwrap();
            index
                .sessions
                .keys()
                .chain(index.docs.keys())
                .chain(index.transcripts.keys())
                .chain(index.tasks.keys())
                .filter(|id| {
                    *id != VAULT_TASKS_KEY
                        && !present.contains(id.as_str())
                        && !protected.contains(id.as_str())
                })
                .cloned()
                .collect::<HashSet<String>>()
                .into_iter()
                .collect()
        };
        for id in stale {
            self.index_remove_session_and_notify(&id);
        }

        if let Some(tasks) = self.read_index_tasks(paths::vault_tasks_path()).await {
            let changed = {
                let mut index = self.index.write().unwrap();
                apply_map_value(&mut index.tasks, VAULT_TASKS_KEY, tasks)
            };
            if changed {
                self.notify_index_changed(IndexEntity::Tasks, vec![VAULT_TASKS_KEY.to_string()]);
            }
        }

        self.index_refresh_people().await;
        self.index_refresh_tags().await;

        if let Some(error) = first_outer {
            return Err(error);
        }
        Ok(report)
    }

    /// Watcher + focus entry point: re-read one session's files, refresh its index slice.
    /// Missing `_meta.json` -> remove the session's index entries. Never touches files.
    ///
    /// `Err` does not mean nothing happened: any slices that reconciled before the failing
    /// artifact are already applied to the index. rebuild/refresh are idempotent, so a
    /// caller can simply retry -- the next pass converges on the same result rather than
    /// double-applying anything.
    pub async fn refresh_session(&self, session_id: &str) -> Result<(), StoreError> {
        let mut report = RebuildReport::default();
        let first_error = self.refresh_one(session_id, None, &mut report).await?;

        if let Some(first_error) = first_error {
            // Propagate the original variant (Io/Serialize) rather than relabeling every
            // per-artifact failure -- callers may want to distinguish, e.g., a transient
            // permission error (Io, worth retrying) from real corruption.
            return Err(first_error);
        }
        Ok(())
    }

    /// Shared by `rebuild_index` (looped over every folder) and `refresh_session` (one id).
    /// A missing `_meta.json` means this id has no session identity in the index -- every
    /// entry for it is removed and we return early without inspecting the other files.
    /// Anything else that fails to read/parse is logged and its existing entry is left
    /// exactly as it was; only the entities whose value actually changed are notified.
    ///
    /// Returns the first raw `StoreError` encountered among the per-artifact reads (already
    /// also logged into `report.errors` as a formatted string) so `refresh_session` can hand
    /// its caller the real error variant. The outer `Result` is reserved for failures that
    /// must abort this session's refresh entirely (task-join failures).
    /// `known_meta`: a meta the caller already parsed (the discovery walk's) -- trusted
    /// as-is, skipping the `_meta.json` re-read *and* the missing-meta removal path.
    /// The scan-time snapshot is safe to trust: layout normalization only renames
    /// directories (never rewrites meta content), and a meta write racing in between
    /// is the same brief-staleness window the re-read had, resolved by the next
    /// rescan. Pass `None` (refresh_session) to derive everything from the files.
    async fn refresh_one(
        &self,
        id: &str,
        known_meta: Option<SessionMeta>,
        report: &mut RebuildReport,
    ) -> Result<Option<StoreError>, StoreError> {
        let mut first_error: Option<StoreError> = None;

        let read_meta = match known_meta {
            Some(meta) => Ok(Some(meta)),
            None => self.read_meta(id).await,
        };
        let meta = match read_meta {
            Ok(None) => {
                // The directory (or at least its identity) is gone: drop the catalog
                // entry too, so a later write re-resolves instead of recreating the
                // stale path.
                self.catalog_remove(id);
                self.index_remove_session_and_notify(id);
                match self.session_has_content(id).await {
                    Ok(true) => report.ghost_sessions.push(id.to_string()),
                    Ok(false) => {}
                    Err(e) => record_error(
                        &mut report.errors,
                        &mut first_error,
                        &format!("{id}: ghost-content scan"),
                        e,
                    ),
                }
                return Ok(first_error);
            }
            Ok(Some(meta)) => {
                report.sessions += 1;
                Some(meta)
            }
            Err(e) => {
                // Unreadable/corrupt: keep the old entry.
                record_error(
                    &mut report.errors,
                    &mut first_error,
                    &format!("{id}: _meta.json"),
                    e,
                );
                None
            }
        };

        let note = match self.read_note(id).await {
            Ok(note) => {
                if note.is_some() {
                    report.notes += 1;
                }
                Some(note)
            }
            Err(e) => {
                record_error(
                    &mut report.errors,
                    &mut first_error,
                    &format!("{id}: notes.md"),
                    e,
                );
                None
            }
        };

        // Documents: the enhanced/ listing must succeed before the docs vec is replaced
        // wholesale -- a failed listing can't tell "gone" from "unlistable", and replacing
        // from a partial scan would make corruption look like deletion. Individual files
        // that fail to parse keep their previous entry by id, same invariant.
        let previous_docs: HashMap<String, super::EnhancedDoc> = {
            let index = self.index.read().unwrap();
            index
                .docs
                .get(id)
                .map(|docs| {
                    docs.iter()
                        .map(|doc| (doc.id.clone(), doc.clone()))
                        .collect()
                })
                .unwrap_or_default()
        };
        let mut document_scans_succeeded = true;
        let mut collected_docs = Vec::new();
        match self.scan_enhanced_doc_files(id).await {
            Ok(enhanced_files) => {
                for (doc_id, parsed) in enhanced_files {
                    match parsed {
                        Ok(doc) => {
                            collected_docs.push(doc);
                            report.notes += 1;
                        }
                        Err(e) => {
                            record_error(
                                &mut report.errors,
                                &mut first_error,
                                &format!("{id}: enhanced/{doc_id}.md"),
                                e,
                            );
                            if let Some(old) = previous_docs.get(&doc_id) {
                                collected_docs.push(old.clone());
                            }
                        }
                    }
                }
            }
            Err(e) => {
                document_scans_succeeded = false;
                record_error(&mut report.errors, &mut first_error, id, e);
            }
        }
        let docs = document_scans_succeeded.then_some(collected_docs);

        let transcripts = match self.read_transcript_summary(id).await {
            Ok(summary) => {
                report.transcripts += summary.transcript_ids.len();
                // A missing transcript.json reads as an empty summary, so this also
                // correctly removes the entry when the file itself is gone.
                Some(summary)
            }
            Err(e) => {
                record_error(
                    &mut report.errors,
                    &mut first_error,
                    &format!("{id}: transcript.json"),
                    e,
                );
                None
            }
        };

        let tasks = match self.session_dir(id).await {
            Ok(dir) => {
                self.read_index_tasks(paths::session_tasks_path_in(&dir))
                    .await
            }
            // Unresolvable (e.g. ambiguous) id: leave the existing tasks entry alone,
            // same keep-on-failure contract as every other artifact here.
            Err(_) => None,
        };

        let mut changes = Vec::new();
        {
            let mut index = self.index.write().unwrap();

            if let Some(new_meta) = meta {
                let old = index.sessions.get(id);
                let note_markdown = match note.clone() {
                    Some(note) => note,
                    None => old.and_then(|entry| entry.note_markdown.clone()),
                };
                let entry = SessionEntry {
                    meta: new_meta,
                    note_markdown,
                };
                if old != Some(&entry) {
                    index.sessions.insert(id.to_string(), entry);
                    changes.push((IndexEntity::Sessions, id.to_string()));
                }
            } else if let Some(Some(note)) = note {
                // Meta unreadable (kept as-is) but the note read fine: still refresh it.
                if let Some(entry) = index.sessions.get_mut(id) {
                    if entry.note_markdown.as_ref() != Some(&note) {
                        entry.note_markdown = Some(note);
                        changes.push((IndexEntity::Sessions, id.to_string()));
                    }
                }
            }

            if let Some(new_docs) = docs {
                if apply_map_value(&mut index.docs, id, new_docs) {
                    changes.push((IndexEntity::Docs, id.to_string()));
                }
            }
            if let Some(new_summary) = transcripts {
                if apply_transcript_summary(&mut index.transcripts, id, new_summary) {
                    changes.push((IndexEntity::Transcripts, id.to_string()));
                }
            }
            if let Some(new_tasks) = tasks {
                if apply_map_value(&mut index.tasks, id, new_tasks) {
                    changes.push((IndexEntity::Tasks, id.to_string()));
                }
            }
        }
        self.notify_many(changes);

        Ok(first_error)
    }

    // -- filesystem reads (read-only; never writes to the vault) --

    /// Discovery-backed layout scan: the healthy sessions (identity from
    /// `_meta.json.id`, both legacy UUID-named and readable directories, with their
    /// metadata), plus the diagnostics rebuild needs -- formatted layout errors, the
    /// directories whose metadata is unreadable, the ids claimed by more than one
    /// directory, and ghost directories (session-like content with no metadata). One
    /// traversal: ghosts come out of the same discovery walk.
    pub(super) async fn scan_session_locations(&self) -> Result<SessionLayoutScan, StoreError> {
        self.scan_session_locations_with_progress(None).await
    }

    async fn scan_session_locations_with_progress(
        &self,
        on_sessions_found: Option<DiscoveryProgress>,
    ) -> Result<SessionLayoutScan, StoreError> {
        let vault_base = self.vault_base.clone();
        tokio::task::spawn_blocking(move || -> Result<SessionLayoutScan, StoreError> {
            let started = std::time::Instant::now();
            let discovery =
                hypr_vault_read::discover_sessions_with_progress(&vault_base, |found| {
                    if let Some(on_sessions_found) = &on_sessions_found {
                        on_sessions_found(found);
                    }
                })?;

            let mut scan = SessionLayoutScan {
                ghost_dirs: discovery
                    .ghost_dirs
                    .iter()
                    .map(|dir| {
                        // Historical report shape: relative to `sessions/`, root-level
                        // ghosts as a bare basename.
                        dir.strip_prefix(hypr_vault_read::paths::sessions_root())
                            .unwrap_or(dir)
                            .to_string_lossy()
                            .into_owned()
                    })
                    .collect(),
                ..Default::default()
            };
            for error in &discovery.errors {
                scan.errors.push(error.to_string());
                match error {
                    hypr_vault_read::SessionDiscoveryError::DuplicateId { id, .. } => {
                        scan.duplicate_ids.push(id.clone());
                    }
                    hypr_vault_read::SessionDiscoveryError::CorruptMeta { dir, .. }
                    | hypr_vault_read::SessionDiscoveryError::Unreadable { dir, .. } => {
                        scan.broken_dirs.push(dir.clone());
                    }
                }
            }
            scan.sessions = discovery.sessions;

            tracing::debug!(
                healthy = scan.sessions.len(),
                broken = scan.broken_dirs.len(),
                duplicates = scan.duplicate_ids.len(),
                ghosts = scan.ghost_dirs.len(),
                elapsed_ms = started.elapsed().as_millis() as u64,
                "session layout discovery"
            );
            Ok(scan)
        })
        .await
        .map_err(|e| StoreError::Io(format!("task join error: {e}")))?
    }

    /// Lists every `<doc_id>.md` under `sessions/<id>/enhanced/` and parses each file's
    /// frontmatter+body into an `EnhancedDoc`. Per-file failures ride the inner `Result`
    /// (caller logs and keeps the entry -- the doc id is still reported so pruning never
    /// mistakes "unparseable" for "gone"); an outer `Err` means the directory listing
    /// itself failed and the caller must not prune (that would look like every document
    /// vanished). A missing `enhanced/` dir is simply "no docs" -- most sessions never
    /// get one.
    ///
    /// Nothing scans the session directory itself for documents anymore: the legacy
    /// single-slot `<kind>.md` layout is retired, and any other file directly in the
    /// session dir is a user attachment the app must ignore (see
    /// `hypr_vault_read::reserved`).
    pub(super) async fn scan_enhanced_doc_files(
        &self,
        id: &str,
    ) -> Result<Vec<(String, Result<super::EnhancedDoc, StoreError>)>, StoreError> {
        let dir = self
            .vault_base
            .join(paths::enhanced_dir_in(&self.session_dir(id).await?));
        let session_id = id.to_string();
        tokio::task::spawn_blocking(
            move || -> Result<Vec<(String, Result<super::EnhancedDoc, StoreError>)>, StoreError> {
                let mut out = Vec::new();
                let entries = match std::fs::read_dir(&dir) {
                    Ok(entries) => entries,
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(out),
                    Err(e) => {
                        return Err(StoreError::Io(format!(
                            "failed to read enhanced docs directory: {e}"
                        )));
                    }
                };
                for entry in entries {
                    let entry = entry
                        .map_err(|e| StoreError::Io(format!("failed to read dir entry: {e}")))?;
                    let path = entry.path();
                    if !path.is_file() {
                        continue;
                    }
                    if path.extension().and_then(|e| e.to_str()) != Some("md") {
                        continue;
                    }
                    let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
                        continue;
                    };
                    // Hygiene: hidden files (stale `.tmp-<pid>-<nonce>-<name>` leftovers
                    // from a crashed atomic write, see
                    // `hypr_fs_sync_core::export::tmp_sibling_path`) and conflict backups
                    // from the retired sync machinery (`<stem>.conflict-<timestamp>.md`)
                    // are never live documents -- frozen evidence, not content.
                    if stem.starts_with('.') {
                        continue;
                    }
                    if stem.contains(".conflict-") {
                        continue;
                    }
                    let parsed = std::fs::read_to_string(&path)
                        .map_err(|e| {
                            StoreError::Io(format!("failed to read enhanced/{stem}.md: {e}"))
                        })
                        .and_then(|raw| {
                            super::enhanced::parse_enhanced_file(stem, &session_id, &raw)
                        });
                    out.push((stem.to_string(), parsed));
                }
                Ok(out)
            },
        )
        .await
        .map_err(|e| StoreError::Io(format!("task join error: {e}")))?
    }

    /// Existence-only scan used to populate `RebuildReport.ghost_sessions`: true if the
    /// session directory has at least one recognized content file (a `<kind>.md` document or
    /// `transcript.json`) despite having no `_meta.json`. Never reads file contents.
    async fn session_has_content(&self, id: &str) -> Result<bool, StoreError> {
        let dir = self.vault_base.join(self.session_dir(id).await?);
        tokio::task::spawn_blocking(move || dir_has_session_content(&dir))
            .await
            .map_err(|e| StoreError::Io(format!("task join error: {e}")))?
    }
}

/// Result of `scan_session_locations`: the healthy sessions (location + parsed
/// metadata, so startup normalization can rename without re-reading) plus
/// everything rebuild must know about the parts of the layout that are not
/// healthy sessions.
#[derive(Debug, Default)]
pub(super) struct SessionLayoutScan {
    pub sessions: Vec<(hypr_vault_read::SessionLocation, super::SessionMeta)>,
    /// Formatted layout diagnostics (duplicate ids, corrupt/unreadable metadata),
    /// carrying physical paths; surfaced through `RebuildReport.errors`.
    pub errors: Vec<String>,
    /// Ids claimed by more than one directory -- ambiguous, never pruned or resolved.
    pub duplicate_ids: Vec<String>,
    /// Vault-relative directories whose `_meta.json` exists but cannot be read/parsed.
    pub broken_dirs: Vec<PathBuf>,
    /// Directories (relative to `sessions/`) holding recognized session content (a
    /// `<kind>.md` document or `transcript.json`) with no `_meta.json` at all -- left
    /// deliberately unindexed, files untouched.
    pub ghost_dirs: Vec<String>,
}

fn dir_has_session_content(dir: &std::path::Path) -> Result<bool, StoreError> {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(_) => return Ok(false),
    };
    for entry in entries {
        let Ok(entry) = entry else { continue };
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let is_md = path.extension().and_then(|e| e.to_str()) == Some("md");
        let is_transcript = path.file_name().and_then(|n| n.to_str()) == Some("transcript.json");
        if is_md || is_transcript {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Pushes a human-readable line to `errors` and remembers the first raw `StoreError`
/// encountered this pass, so `refresh_session` can propagate the real variant instead of
/// relabeling every per-artifact failure as `StoreError::Serialize`.
fn record_error(
    errors: &mut Vec<String>,
    first: &mut Option<StoreError>,
    context: &str,
    err: StoreError,
) {
    errors.push(format!("{context}: {err}"));
    if first.is_none() {
        *first = Some(err);
    }
}

#[cfg(test)]
mod tests {
    use hypr_fs_format::TranscriptWithData;

    use super::*;
    use crate::content::SessionMeta;

    fn meta(id: &str, title: &str) -> SessionMeta {
        SessionMeta {
            id: id.to_string(),
            title: title.to_string(),
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

    fn transcript(id: &str, word_text: &str) -> TranscriptWithData {
        TranscriptWithData {
            id: id.to_string(),
            user_id: String::new(),
            created_at: "2026-07-24T00:00:00Z".to_string(),
            session_id: "ignored-by-rebuild".to_string(),
            started_at: 0.0,
            ended_at: None,
            memo_md: String::new(),
            words: vec![hypr_fs_format::TranscriptWord {
                id: Some("w0".to_string()),
                text: word_text.to_string(),
                start_ms: 0.0,
                end_ms: 0.0,
                channel: 0.0,
                speaker: None,
                metadata: None,
            }],
            speaker_hints: vec![],
        }
    }

    async fn test_store() -> (SessionStore, tempfile::TempDir) {
        let temp = tempfile::tempdir().unwrap();
        let store = SessionStore::new(temp.path().to_path_buf());
        (store, temp)
    }

    /// Physical directory of a session: creation now picks a human-readable name, so
    /// tests resolve it through the store instead of assuming `sessions/<id>`.
    async fn session_path(
        store: &SessionStore,
        vault: &tempfile::TempDir,
        id: &str,
    ) -> std::path::PathBuf {
        vault.path().join(store.session_dir(id).await.unwrap())
    }

    /// A second store over the same vault, with a cold (empty) index -- the startup shape:
    /// everything it knows must come back from the files alone.
    fn cold_store(vault: &tempfile::TempDir) -> SessionStore {
        SessionStore::new(vault.path().to_path_buf())
    }

    #[tokio::test]
    async fn startup_callbacks_report_discovery_and_completed_index_work() {
        let (writer, vault) = test_store().await;
        writer.write_meta(&meta("s1", "One")).await.unwrap();
        writer.write_meta(&meta("s2", "Two")).await.unwrap();
        let store = cold_store(&vault);

        let discovered = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let discovered_for_callback = discovered.clone();
        let layout = store
            .normalize_startup_layout_with_progress(move |found| {
                discovered_for_callback.lock().unwrap().push(found);
            })
            .await
            .unwrap();
        assert_eq!(*discovered.lock().unwrap(), vec![1, 2]);

        let indexed = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let indexed_for_callback = indexed.clone();
        store
            .rebuild_index_from_startup_layout_with_progress(layout, move |completed, total| {
                indexed_for_callback
                    .lock()
                    .unwrap()
                    .push((completed, total));
            })
            .await
            .unwrap();

        assert_eq!(*indexed.lock().unwrap(), vec![(1, 2), (2, 2)]);
    }

    /// Drains everything currently queued on the store's change bus. The bus is the
    /// observable that replaced the SQL dirty queue: a no-op rescan must leave it
    /// empty, a genuine change must land on it.
    fn drain_changes(store: &SessionStore) -> Vec<(crate::IndexEntity, Vec<String>)> {
        let mut rx = store
            .take_index_change_receiver()
            .expect("receiver taken once per drain");
        let mut changes = Vec::new();
        while let Ok(change) = rx.try_recv() {
            changes.push(change);
        }
        *store.index_changes_rx.lock().unwrap() = Some(rx);
        changes
    }

    /// The no-op-rescan guarantee (was: "unchanged rebuild must not requeue search
    /// reindexing" against the SQL dirty queue): rebuild_index runs
    /// automatically on every startup and window focus, so a rebuild over unchanged files
    /// must emit nothing on the change bus -- otherwise every boot/focus re-triggers the
    /// search projection and every subscribed webview, forever.
    #[tokio::test]
    async fn rebuild_of_unchanged_files_does_not_notify_the_change_bus() {
        let (store, _vault) = test_store().await;
        store.write_meta(&meta("s1", "One")).await.unwrap();
        store.write_note("s1", "# hi").await.unwrap();
        store.rebuild_index().await.unwrap();
        drain_changes(&store);

        store.rebuild_index().await.unwrap();

        assert_eq!(drain_changes(&store), vec![]);
    }

    /// The mirror image of the no-op test above: the diff must never suppress a *genuine*
    /// change, only a spurious re-fire. Simulates an external edit with a raw
    /// `std::fs::write` (bypassing `write_meta` entirely, the way another device or a
    /// hand-edit would) and asserts both halves of "the reconcile actually reconciled":
    /// the index value changed, and the change bus saw the session.
    #[tokio::test]
    async fn rebuild_of_a_genuinely_changed_file_updates_the_index_and_notifies() {
        let (store, vault) = test_store().await;
        store.write_meta(&meta("s1", "One")).await.unwrap();
        store.rebuild_index().await.unwrap();
        drain_changes(&store);

        let meta_path = session_path(&store, &vault, "s1").await.join("_meta.json");
        let edited = serde_json::to_vec_pretty(&meta("s1", "Two")).unwrap();
        std::fs::write(&meta_path, edited).unwrap();

        store.rebuild_index().await.unwrap();

        assert_eq!(store.session_get("s1").unwrap().meta.title, "Two");
        let changes = drain_changes(&store);
        assert!(
            changes.iter().any(|(entity, ids)| {
                *entity == crate::IndexEntity::Sessions && ids.contains(&"s1".to_string())
            }),
            "a genuine change must still notify, not just no-ops getting skipped: {changes:?}"
        );
    }

    /// The content-hash guarantee on `TranscriptSummary`: an external edit that changes
    /// word *content* without changing the file's shape (same transcript ids, same word
    /// counts) must still notify `Transcripts` -- the summary would otherwise compare
    /// equal and the search projection + frontend would silently serve stale words.
    #[tokio::test]
    async fn rebuild_notifies_transcripts_on_same_shape_word_edit() {
        let (store, vault) = test_store().await;
        store.write_meta(&meta("s1", "One")).await.unwrap();
        store
            .write_transcript("s1", transcript("t1", "hello"))
            .await
            .unwrap();
        store.rebuild_index().await.unwrap();
        drain_changes(&store);

        let path = session_path(&store, &vault, "s1")
            .await
            .join("transcript.json");
        let raw = std::fs::read_to_string(&path).unwrap();
        // Same byte length, same shape -- only the word text differs.
        std::fs::write(&path, raw.replace("hello", "howdy")).unwrap();

        store.rebuild_index().await.unwrap();

        let changes = drain_changes(&store);
        assert!(
            changes.iter().any(|(entity, ids)| {
                *entity == crate::IndexEntity::Transcripts && ids.contains(&"s1".to_string())
            }),
            "a same-shape word edit must still notify Transcripts: {changes:?}"
        );
        assert_eq!(
            store.session_transcripts("s1").await.unwrap()[0].words[0].text,
            "howdy"
        );
    }

    /// Reproduces the failure mode a real boot smoke turned up: a `_memo.md` that already
    /// carries a frontmatter wrapper (as an external edit or the retired legacy exporter
    /// would leave behind) must not have that wrapper compound with each
    /// automatic rebuild pass. Without `strip_leading_frontmatter` in the read path, each of
    /// these calls would index the *previous* pass's own wrapper verbatim, growing the indexed
    /// body by one nested frontmatter block every time -- exactly what the automatic
    /// startup/focus rescans would otherwise do to it on every boot. The fixture deliberately
    /// uses the pre-rename `_memo.md` name, doubling as rebuild coverage for the legacy-note
    /// read fallback.
    #[tokio::test]
    async fn rebuild_of_an_already_wrapped_note_file_does_not_grow_the_indexed_body() {
        let (store, vault) = test_store().await;
        store.write_meta(&meta("s1", "One")).await.unwrap();
        let dir = session_path(&store, &vault, "s1").await;
        std::fs::write(
            dir.join("_memo.md"),
            "---\nid: s1:note\nposition: 0\nsession_id: s1\n---\n\nreal content",
        )
        .unwrap();

        store.rebuild_index().await.unwrap();
        assert_eq!(
            store.session_get("s1").unwrap().note_markdown.as_deref(),
            Some("real content")
        );

        store.rebuild_index().await.unwrap();
        store.rebuild_index().await.unwrap();
        assert_eq!(
            store.session_get("s1").unwrap().note_markdown.as_deref(),
            Some("real content")
        );
    }

    #[tokio::test]
    async fn rebuild_is_idempotent() {
        let (store, _vault) = test_store().await;
        store.write_meta(&meta("s1", "One")).await.unwrap();
        store.write_note("s1", "# hi").await.unwrap();
        store.rebuild_index().await.unwrap();
        let first = store.index.read().unwrap().clone();
        store.rebuild_index().await.unwrap();
        assert_eq!(first, *store.index.read().unwrap());
    }

    /// The whole point of files-as-truth: a brand-new store (cold, empty index -- the
    /// startup shape) rebuilds everything from the vault alone.
    #[tokio::test]
    async fn rebuild_from_a_cold_index_restores_everything_from_files() {
        let (store, vault) = test_store().await;
        store.write_meta(&meta("s1", "One")).await.unwrap();
        store.write_note("s1", "notes").await.unwrap();
        store
            .write_transcript("s1", transcript("t1", "restored-word"))
            .await
            .unwrap();

        let cold = cold_store(&vault);
        assert!(cold.session_get("s1").is_none());
        cold.rebuild_index().await.unwrap();

        let record = cold.session_get("s1").unwrap();
        assert_eq!(record.meta.title, "One");
        assert_eq!(record.note_markdown.as_deref(), Some("notes"));
        assert_eq!(
            cold.transcript_get("t1").await.unwrap().unwrap().words[0].text,
            "restored-word"
        );
    }

    /// A legacy calendar-event envelope on disk (written by a pre-removal build) must
    /// survive a cold rebuild untouched, riding the `extra` catch-all.
    #[tokio::test]
    async fn rebuild_restores_legacy_event_and_folder_from_files() {
        let (store, vault) = test_store().await;
        let mut m = meta("s1", "One");
        m.extra.insert(
            "event".to_string(),
            serde_json::json!({"tracking_id": "evt-1", "meeting_link": ""}),
        );
        m.folder = Some("work".to_string());
        store.write_meta(&m).await.unwrap();

        let cold = cold_store(&vault);
        cold.rebuild_index().await.unwrap();

        let restored = cold.session_get("s1").unwrap().meta;
        assert_eq!(restored.extra.get("event"), m.extra.get("event"));
        assert_eq!(restored.folder.as_deref(), Some("work"));
    }

    /// The no-op property must hold for the widened meta fields too: a meta carrying a
    /// legacy `event` envelope in `extra` re-derives to an identical entry on every
    /// rebuild pass, so an unchanged file must still stay silent on the bus.
    #[tokio::test]
    async fn rebuild_of_unchanged_legacy_event_and_folder_does_not_notify() {
        let (store, _vault) = test_store().await;
        let mut m = meta("s1", "One");
        m.extra.insert(
            "event".to_string(),
            serde_json::json!({"tracking_id": "evt-1", "meeting_link": "x"}),
        );
        m.folder = Some("work".to_string());
        store.write_meta(&m).await.unwrap();
        store.rebuild_index().await.unwrap();
        drain_changes(&store);

        store.rebuild_index().await.unwrap();

        assert_eq!(drain_changes(&store), vec![]);
    }

    #[tokio::test]
    async fn refresh_missing_meta_removes_index_entry_but_no_files() {
        let (store, vault) = test_store().await;
        store.write_meta(&meta("s1", "One")).await.unwrap();
        store.write_note("s1", "keep me").await.unwrap();
        let dir = session_path(&store, &vault, "s1").await;
        std::fs::remove_file(dir.join("_meta.json")).unwrap();
        store.refresh_session("s1").await.unwrap();
        assert!(store.session_get("s1").is_none());
        assert!(dir.join("notes.md").is_file()); // vault untouched
    }

    #[tokio::test]
    async fn rebuild_unparseable_meta_leaves_existing_entry_and_logs_error() {
        let (store, vault) = test_store().await;
        // Seed a legacy uuid-style directory by hand: a corrupt meta can no longer name
        // its id, so the reported error identifies the session by its directory path.
        let dir = vault.path().join("sessions/s1");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("_meta.json"),
            serde_json::to_vec_pretty(&meta("s1", "Original")).unwrap(),
        )
        .unwrap();
        store.rebuild_index().await.unwrap();
        std::fs::write(dir.join("_meta.json"), b"{ not json").unwrap();

        let report = store.rebuild_index().await.unwrap();

        assert_eq!(
            store.session_get("s1").unwrap().meta.title,
            "Original",
            "existing entry must survive a corrupt file"
        );
        assert_eq!(report.errors.len(), 1);
        assert!(report.errors[0].contains("s1"));
    }

    #[tokio::test]
    async fn rebuild_removes_entries_for_vanished_folder_across_all_maps() {
        let (store, vault) = test_store().await;
        store.write_meta(&meta("s1", "One")).await.unwrap();
        store.write_note("s1", "notes").await.unwrap();
        store
            .write_enhanced_doc(&enhanced_doc("s1", "doc-1"))
            .await
            .unwrap();
        store
            .write_transcript("s1", transcript("t1", "hi"))
            .await
            .unwrap();

        std::fs::remove_dir_all(session_path(&store, &vault, "s1").await).unwrap();

        store.rebuild_index().await.unwrap();

        assert!(store.session_get("s1").is_none());
        assert!(store.session_enhanced_docs("s1").is_empty());
        assert!(store.session_transcripts("s1").await.unwrap().is_empty());
    }

    /// REGRESSION: `Path::exists()` swallows read failures as "false", which used to make
    /// a transiently-unreadable `_meta.json` look identical to a missing one and delete a
    /// live session's index entries. read_meta must distinguish "not found" from "exists
    /// but unreadable" and rebuild must treat the latter as an error, not a deletion.
    /// Unreadability is injected by replacing the file with a directory of the same name
    /// (EISDIR on read) -- unlike the previous chmod-0o000 injection, this fails for root
    /// too, so the test holds no matter which user runs it.
    #[tokio::test]
    async fn rebuild_unreadable_meta_leaves_existing_entry_and_logs_error() {
        let (store, vault) = test_store().await;
        store.write_meta(&meta("s1", "Original")).await.unwrap();

        let meta_path = session_path(&store, &vault, "s1").await.join("_meta.json");
        std::fs::remove_file(&meta_path).unwrap();
        std::fs::create_dir(&meta_path).unwrap();

        let report = store.rebuild_index().await.unwrap();

        assert_eq!(
            store.session_get("s1").unwrap().meta.title,
            "Original",
            "existing entry must survive a transiently-unreadable file, not just a corrupt one"
        );
        assert!(
            !report.errors.is_empty(),
            "an unreadable file must be reported, not silently treated as absent"
        );
    }

    /// Vaults created before this branch can still hold the retired sync machinery's
    /// conflict backups (`<stem>.conflict-<timestamp>.md`) and, after a crash mid-write,
    /// `.tmp-<pid>-<nonce>-<name>` atomic-write leftovers. Neither is a live document;
    /// rebuild must not index them as one.
    #[tokio::test]
    async fn rebuild_ignores_conflict_backups_and_tmp_leftovers() {
        let (store, vault) = test_store().await;
        store.write_meta(&meta("s1", "One")).await.unwrap();
        store.write_note("s1", "live note").await.unwrap();
        let dir = session_path(&store, &vault, "s1").await;
        std::fs::write(
            dir.join("_memo.conflict-2026-07-23T12-00-00.123Z.md"),
            "stale conflict copy",
        )
        .unwrap();
        std::fs::write(dir.join(".tmp-1234-5678-_memo.md"), "crashed atomic write").unwrap();

        let report = store.rebuild_index().await.unwrap();

        assert_eq!(report.notes, 1, "only the live note should be indexed");
        assert_eq!(
            store.session_get("s1").unwrap().note_markdown.as_deref(),
            Some("live note")
        );
        let index = store.index.read().unwrap();
        assert!(
            !index.docs.contains_key("s1"),
            "neither leftover may become a document entry: {:?}",
            index.docs.get("s1")
        );
    }

    fn enhanced_doc(session_id: &str, doc_id: &str) -> crate::EnhancedDoc {
        crate::EnhancedDoc {
            id: doc_id.to_string(),
            session_id: session_id.to_string(),
            kind: "template_output".to_string(),
            title: "Customer review".to_string(),
            template_id: "template-1".to_string(),
            sort_order: 2,
            markdown: "# Review\n\n- Point".to_string(),
        }
    }

    /// The whole point of the file home: on a cold index, every metadata field
    /// (title/template_id/sort_order/kind) comes back from the frontmatter alone.
    #[tokio::test]
    async fn rebuild_restores_enhanced_doc_metadata_from_frontmatter() {
        let (store, vault) = test_store().await;
        store.write_meta(&meta("s1", "One")).await.unwrap();
        store
            .write_enhanced_doc(&enhanced_doc("s1", "doc-1"))
            .await
            .unwrap();

        let cold = cold_store(&vault);
        cold.rebuild_index().await.unwrap();

        assert_eq!(
            cold.enhanced_doc_get("doc-1"),
            Some(enhanced_doc("s1", "doc-1"))
        );
    }

    /// The no-op property must hold for enhanced docs too: an unchanged
    /// `enhanced/<doc>.md` must stay silent on the bus across the automatic
    /// startup/focus rebuild passes.
    #[tokio::test]
    async fn rebuild_of_unchanged_enhanced_doc_does_not_notify() {
        let (store, _vault) = test_store().await;
        store.write_meta(&meta("s1", "One")).await.unwrap();
        store
            .write_enhanced_doc(&enhanced_doc("s1", "doc-1"))
            .await
            .unwrap();
        store.rebuild_index().await.unwrap();
        drain_changes(&store);

        store.rebuild_index().await.unwrap();

        assert_eq!(drain_changes(&store), vec![]);
    }

    #[tokio::test]
    async fn rebuild_prunes_enhanced_doc_entry_whose_file_is_gone() {
        let (store, vault) = test_store().await;
        store.write_meta(&meta("s1", "One")).await.unwrap();
        store
            .write_enhanced_doc(&enhanced_doc("s1", "doc-1"))
            .await
            .unwrap();

        std::fs::remove_file(
            session_path(&store, &vault, "s1")
                .await
                .join("enhanced/doc-1.md"),
        )
        .unwrap();

        store.rebuild_index().await.unwrap();

        assert!(store.enhanced_doc_get("doc-1").is_none());
    }

    /// Corruption must never look like deletion: an `enhanced/<doc>.md` whose frontmatter
    /// no longer parses is logged, and its existing index entry survives the pass -- both
    /// the re-derive and the prune must respect it.
    #[tokio::test]
    async fn rebuild_unparseable_enhanced_doc_leaves_existing_entry_and_logs_error() {
        let (store, vault) = test_store().await;
        store.write_meta(&meta("s1", "One")).await.unwrap();
        store
            .write_enhanced_doc(&enhanced_doc("s1", "doc-1"))
            .await
            .unwrap();

        std::fs::write(
            session_path(&store, &vault, "s1")
                .await
                .join("enhanced/doc-1.md"),
            "---\ntitle: [unclosed\n---\n\nbody",
        )
        .unwrap();

        let report = store.rebuild_index().await.unwrap();

        assert_eq!(
            store.enhanced_doc_get("doc-1").unwrap().title,
            "Customer review",
            "existing entry must survive a corrupt file"
        );
        assert!(
            report
                .errors
                .iter()
                .any(|e| e.contains("enhanced/doc-1.md")),
            "the corrupt doc must be reported: {:?}",
            report.errors
        );
    }

    /// Pre-cutover UUID summary entries never had a file home, and the owner's
    /// no-migration directive means they never get one -- rebuild prunes them exactly as
    /// it did before this task (this test pins that preserved behavior rather than
    /// introducing it).
    #[tokio::test]
    async fn rebuild_still_prunes_legacy_index_only_uuid_entries_without_files() {
        let (store, _vault) = test_store().await;
        store.write_meta(&meta("s1", "One")).await.unwrap();
        store
            .write_enhanced_doc(&enhanced_doc("s1", "doc-1"))
            .await
            .unwrap();

        {
            let mut index = store.index.write().unwrap();
            index
                .docs
                .entry("s1".to_string())
                .or_default()
                .push(crate::EnhancedDoc {
                    id: "legacy-uuid".to_string(),
                    session_id: "s1".to_string(),
                    kind: "summary".to_string(),
                    title: String::new(),
                    template_id: String::new(),
                    sort_order: 0,
                    markdown: "{}".to_string(),
                });
        }

        store.rebuild_index().await.unwrap();

        assert!(
            store.enhanced_doc_get("legacy-uuid").is_none(),
            "index-only entries without files stay pruned"
        );
        assert!(
            store.enhanced_doc_get("doc-1").is_some(),
            "file-backed docs must survive the same prune"
        );
    }

    #[tokio::test]
    async fn rebuild_reports_ghost_sessions_without_indexing_them() {
        let (store, _vault) = test_store().await;
        // A "ghost" session: transcript.json written without ever calling write_meta, matching
        // Task 7's recording_into_unknown_session_still_persists regression.
        store
            .write_transcript("ghost", transcript("t1", "hi"))
            .await
            .unwrap();

        let report = store.rebuild_index().await.unwrap();

        assert_eq!(report.ghost_sessions, vec!["ghost".to_string()]);
        assert!(
            store.session_get("ghost").is_none(),
            "ghost sessions must not be indexed"
        );
    }
}
