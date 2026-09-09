//! External-edit ingestion: vault file watcher -> index-only refresh.
//!
//! `plugins/notify` recursively watches `vault_base` (debounced ~900ms) and
//! emits a `FileChanged { path }` event for every changed path that isn't
//! one of its own TTL-filtered own-writes (`is_external_path` in
//! `plugins/notify/src/ext.rs`, driven by `mark_own_writes`). This module is
//! that event's first listener.
//!
//! # The incident this rewrite fixes
//!
//! The *previous* version of this module misread the app's own export/trash
//! renames as "session folder removed externally" and soft-hid a live
//! session -- `plugins/notify`'s own-write TTL (1.8s) was shorter than the
//! FSEvents delivery latency it was racing, so a real own-write occasionally
//! arrived *after* the TTL window closed and got treated as an external
//! delete. That version also called the legacy import plugin's files-win
//! reconcile, which could itself write DB rows in response.
//!
//! This version never does either of those things:
//!
//! 1. **The event -> action pipeline is pure and index-only** --
//!    `classify_event` below. There is no delete verb, no soft-hide, no
//!    write-to-vault path. The worst an incorrectly-classified event can do
//!    is call `store.refresh_session` or `store.rebuild_index`, both of
//!    which are read-only on the filesystem and transactional on the index
//!    (see `session_store/rebuild.rs`): a session whose folder is genuinely
//!    gone loses its index rows (correct), a session whose folder is
//!    untouched gets re-indexed as a no-op (harmless). Files are **never**
//!    touched by this module, in either direction.
//!
//! 2. **The own-write filter is the write journal, not a TTL.**
//!    `SessionStore`'s `write_file` (used by every `write_meta`/`write_note`/
//!    `write_enhanced_doc` call) records the sha256 of exactly what it wrote to
//!    exactly that relative path, with no expiry
//!    (`session_store::journal::WriteJournal::matches_current_file`). A
//!    `FileChanged` for a path whose current on-disk bytes still match the
//!    last hash this store wrote there is *always* recognized as this
//!    store's own write, no matter how late the filesystem event arrives --
//!    that's what `own_write_is_ignored_even_if_late` below asserts.
//!    `plugins/notify`'s `mark_own_writes`/TTL mechanism still exists
//!    upstream -- this module may incidentally benefit from that filtering
//!    (fewer events reach it at all), but its own correctness never depends
//!    on it: every event that *does* arrive here is re-checked against the
//!    journal from scratch.
//!
//! # What "external" means for a path outside `sessions/`
//!
//! Only paths under a *cataloged* session directory (or the directory
//! itself, e.g. a rename's old endpoint) ever produce a `Refresh` -- the
//! session directory is no longer assumed to be named after the id, so
//! ownership comes from `SessionStore::session_id_for_relative_path`
//! (longest-prefix, NFC-normalized). Any other path under `sessions/` --
//! a new/copied/renamed directory, a rename's new endpoint, or the bare
//! `sessions` root -- is structural and produces one coalesced
//! `RebuildSessions` instead of a guessed id.
//! Everything else -- `.trash/`, this app's own index/export bookkeeping
//! (`app.db*`, `search_index/`, `AGENTS.md`, the export marker file), and
//! any in-flight `.tmp-`-prefixed atomic-write temp file -- is `Ignore`d
//! outright, independent of the journal check. `plugins/notify`'s own
//! `should_skip_path` already filters most of these upstream (tmp-prefixed
//! basenames, `search_index/`), but this module re-asserts the ones that
//! matter for the incident this rewrite fixes -- above all `.trash/`, since
//! that's exactly where the old watcher's misfire pointed.
//!
//! # Coalescing
//!
//! A burst of events (a sync client delivering several files back-to-back,
//! or a single folder move producing separate old-path/new-path events) is
//! collected for a sliding `COALESCE_WINDOW` quiet period and reduced to a
//! `RefreshPlan` (distinct session ids plus one structural-rebuild flag)
//! before any store call is made -- one refresh per session per burst, not
//! one per raw path, and at most one `rebuild_index` per burst, which then
//! subsumes the per-id refreshes (a rename's old endpoint would otherwise
//! be refreshed as "gone" before rediscovery re-homed it).
//!
//! # Startup ordering
//!
//! Wired from `lib.rs`'s app-level `setup()` closure, after the session
//! store is constructed and `.manage()`d and its startup `rebuild_index`
//! pass has completed -- see `lib.rs`'s comments at the `vault_watch::spawn`
//! call site for the full ordering rationale (a live edit only has to
//! account for vault state from here on).

use std::collections::HashSet;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use tauri::{AppHandle, Manager};
use tauri_plugin_notify::FileChanged;
use tauri_specta::Event;

use crate::session_store::SessionStore;

/// How long to wait for the *next* `FileChanged` event before treating a
/// burst of external edits as finished and refreshing everything seen so
/// far. Sliding quiet-window, not a fixed tick: `run`'s inner loop resets a
/// fresh `COALESCE_WINDOW` timeout after every event it receives, so an
/// active, ongoing burst keeps extending the wait rather than being cut off
/// at a fixed mark from the first event. Wider than `plugins/notify`'s own
/// ~900ms internal debounce (which only coalesces raw filesystem events into
/// one `FileChanged` emission per path) because a sync client or an
/// editor's save-then-touch-metadata sequence can still spread related
/// events across a couple of emissions a few hundred ms apart.
const COALESCE_WINDOW: Duration = Duration::from_secs(2);

/// What a vault-watch event can lead to. No delete/hide verb exists here on
/// purpose -- `Refresh`/`RebuildSessions` are the only session actions, and
/// `refresh_session`/`rebuild_index` themselves (not this module) are what
/// decide whether a missing file means removing index rows.
#[derive(Debug, PartialEq, Eq)]
pub enum WatchAction {
    Ignore,
    Refresh(String),
    /// A structural change under `sessions/`: a path no cataloged session
    /// directory owns (new/copied/renamed directory, a rename endpoint, an
    /// unknown nested dir) or the bare `sessions` root. Identity lives in
    /// `_meta.json`, so the only safe response is one read-only rediscovery
    /// pass (`rebuild_index`) rather than an id guessed from the path.
    RebuildSessions,
    /// An external edit under `templates/` -- rescans the in-memory templates index
    /// (Phase E1; templates have no SQL half to refresh).
    RefreshTemplates,
    /// An external edit of the vault-root `people.json` -- rescans the in-memory
    /// people index (same shape as templates: file-canonical, no per-id granularity).
    RefreshPeople,
    /// An external edit of the vault-root `tags.json` -- rescans the in-memory
    /// tags index (same shape as people).
    RefreshTags,
}

/// Pure routing decision: given a vault-relative path, whether its current
/// on-disk bytes match this store's own last write there, and which logical
/// session (if any) the store's location catalog says owns the path, decide
/// what the watcher should do.
///
/// `journal_match` is checked first and unconditionally wins -- an own
/// write is ignored no matter how the path would otherwise classify. Only
/// after that does path shape matter: a path under `sessions/` that the
/// catalog attributes to a session (`catalog_session_id`, resolved by the
/// caller via `SessionStore::session_id_for_relative_path` so this function
/// stays pure) is a `Refresh` for that logical id, whether the change is an
/// edit, a create, or a delete (this function never inspects the
/// filesystem, so "deleted" and "edited" look identical to it --
/// `refresh_session` is what tells them apart). A `sessions/` path no
/// cataloged directory owns -- including the bare `sessions` root -- is
/// structural: `RebuildSessions`. Everything else -- non-session paths,
/// `.trash/`, this app's own bookkeeping files, in-flight atomic-write temp
/// files -- is `Ignore`.
pub fn classify_event(
    relative: &str,
    journal_match: bool,
    catalog_session_id: Option<&str>,
) -> WatchAction {
    let relative = hypr_storage::fs::relative_path_key(relative);
    let relative = relative.as_ref();
    if journal_match {
        return WatchAction::Ignore;
    }

    if is_ignored_relative_path(relative) {
        return WatchAction::Ignore;
    }

    if is_templates_path(relative) {
        return WatchAction::RefreshTemplates;
    }

    if is_people_path(relative) {
        return WatchAction::RefreshPeople;
    }

    if is_tags_path(relative) {
        return WatchAction::RefreshTags;
    }

    if is_sessions_path(relative) {
        // Hidden entries (`.DS_Store`, AppleDouble `._*`, sync-client dot-dirs)
        // are never session artifacts and layout discovery skips them entirely,
        // so they must not read as structural changes -- every Finder-touched
        // `sessions/.DS_Store` would otherwise trigger a full rebuild.
        if has_hidden_session_component(relative) {
            return WatchAction::Ignore;
        }
        return match catalog_session_id {
            Some(id) => WatchAction::Refresh(id.to_string()),
            None => WatchAction::RebuildSessions,
        };
    }

    WatchAction::Ignore
}

/// Any dot-prefixed component below the `sessions/` root (the file itself or a
/// containing directory).
fn has_hidden_session_component(relative: &str) -> bool {
    relative
        .split('/')
        .skip(1)
        .any(|component| component.starts_with('.'))
}

/// Any change under `templates/` (including `.deleted-defaults.json` -- a tombstone
/// edit changes which defaults exist) rescans the whole templates index; the map is
/// small enough that per-file granularity isn't worth the bookkeeping.
fn is_templates_path(relative: &str) -> bool {
    relative == "templates" || relative.starts_with("templates/")
}

fn is_people_path(relative: &str) -> bool {
    relative == "people.json"
}

fn is_tags_path(relative: &str) -> bool {
    relative == "tags.json"
}

/// Anything at or under the `sessions/` root, including the bare root
/// itself (whose own change events -- e.g. a directory created or removed
/// directly inside it -- are structural).
fn is_sessions_path(relative: &str) -> bool {
    relative == "sessions" || relative.starts_with("sessions/")
}

fn is_trash_path(relative: &str) -> bool {
    relative == ".trash" || relative.starts_with(".trash/")
}

/// The retired SQLite database and its sidecars. The app-data dir that used
/// to hold `app.db` is also the *default* vault base, so these can genuinely
/// sit inside the vault. `legacy_db` renames the live names (`app.db`,
/// `app.db-wal`, `app.db-shm`) to `app.db.pre-files-backup*` at startup, and
/// a user may restore a backup by hand at any time, so the prefix match
/// deliberately covers both spellings: neither a not-yet-retired database nor
/// a restored backup may ever trigger a refresh.
fn is_app_db_path(relative: &str) -> bool {
    relative
        .split('/')
        .next()
        .is_some_and(|top| top.starts_with("app.db"))
}

/// Legacy location only: the search index now lives in the app-data dir,
/// not the vault (mirrors `plugins/notify/src/path.rs`'s `should_skip_path`
/// guard for the same stale-leftover case).
fn is_search_index_path(relative: &str) -> bool {
    relative == "search_index" || relative.starts_with("search_index/")
}

fn is_agents_md_path(relative: &str) -> bool {
    relative == "AGENTS.md"
}

/// The retired legacy exporter's first-run marker file. The worker that
/// wrote it is gone (Task 13), but the file itself still sits at the root of
/// every previously-migrated vault, so a touch/sync of it must stay ignored.
const LEGACY_EXPORT_MARKER_FILENAME: &str = ".fmt-export-version";

fn is_export_marker_path(relative: &str) -> bool {
    relative == LEGACY_EXPORT_MARKER_FILENAME
}

/// `hypr_fs_sync_core::export::tmp_sibling_path` names atomic-write temp
/// files `.tmp-{pid}-{nonce}-{original_name}` as a *sibling* of the real
/// file, so the prefix is always the very start of the basename -- anchored
/// `starts_with`, not `contains`, so a legitimate user file that merely
/// embeds the substring (e.g. `notes.tmp-ideas.md`) still refreshes.
/// `plugins/notify`'s own `should_skip_path` already filters basenames that
/// simply *start* with `.tmp` before a `FileChanged` is even emitted; this
/// is a second, independent check on the exact pattern this app's own
/// writers use, kept here so this module's correctness doesn't depend on
/// that upstream filter either.
fn has_tmp_write_basename(relative: &str) -> bool {
    relative
        .rsplit('/')
        .next()
        .is_some_and(|name| name.starts_with(".tmp-"))
}

/// Everything this module ignores independent of the journal check -- see
/// the module doc's "What 'external' means" section for why each rule
/// exists.
fn is_ignored_relative_path(relative: &str) -> bool {
    is_trash_path(relative)
        || is_app_db_path(relative)
        || is_search_index_path(relative)
        || is_agents_md_path(relative)
        || is_export_marker_path(relative)
        || has_tmp_write_basename(relative)
}

/// Runs `classify_event` for every path in a coalesced batch against the
/// store's real journal, returning the distinct set of session ids that
/// need a refresh. Factored out from `run` so it's directly testable
/// against a real `SessionStore` (see the `real_journal_end_to_end` tests
/// below) without needing a live FSEvents stream.
async fn ids_to_refresh(store: &SessionStore, changed: &HashSet<String>) -> RefreshPlan {
    let mut plan = RefreshPlan::default();
    for relative in changed {
        let journal_match = store.journal_matches_current_file(relative);
        let catalog_session_id = store.session_id_for_relative_path(Path::new(relative));
        match classify_event(relative, journal_match, catalog_session_id.as_deref()) {
            WatchAction::Refresh(id) => {
                plan.session_ids.insert(id);
            }
            WatchAction::RebuildSessions => plan.rebuild_sessions = true,
            WatchAction::RefreshTemplates => plan.templates = true,
            WatchAction::RefreshPeople => plan.people = true,
            WatchAction::RefreshTags => plan.tags = true,
            WatchAction::Ignore => {}
        }
    }
    plan
}

#[derive(Debug, Default, PartialEq)]
struct RefreshPlan {
    session_ids: HashSet<String>,
    /// One flag for the whole batch: any structural `sessions/` change means
    /// one `rebuild_index` pass, which rediscovers every session and thereby
    /// subsumes the per-id refreshes in `session_ids`.
    rebuild_sessions: bool,
    templates: bool,
    people: bool,
    tags: bool,
}

/// Refreshes every id in `ids`, one at a time. A failure on one session is
/// logged and never aborts the rest, and never crashes the watcher loop --
/// `refresh_session` failures are typically transient I/O (see its own
/// doc), and the next `FileChanged` burst for the same session (or the next
/// window-focus rescan) will simply retry.
async fn refresh_ids(store: &SessionStore, ids: HashSet<String>) {
    for id in ids {
        match store.refresh_session(&id).await {
            Ok(()) => {
                tracing::info!(session_id = %id, "vault watch: refreshed session index from external change");
            }
            Err(error) => {
                tracing::warn!(session_id = %id, %error, "vault watch: failed to refresh session index");
            }
        }
    }
}

async fn handle_batch(store: &SessionStore, changed: &HashSet<String>) {
    let plan = ids_to_refresh(store, changed).await;
    if plan.rebuild_sessions {
        // One rebuild for the whole batch, and it subsumes every per-id
        // refresh: a rename burst delivers the old endpoint (cataloged ->
        // Refresh) and the new endpoint (unknown -> RebuildSessions)
        // together, and refreshing the old id alone would drop its index
        // rows before rediscovery re-homed the directory.
        match store.rebuild_index().await {
            Ok(report) => {
                tracing::info!(
                    sessions = report.sessions,
                    errors = report.errors.len(),
                    "vault watch: rebuilt session index from structural external change"
                );
            }
            Err(error) => {
                tracing::warn!(%error, "vault watch: failed to rebuild session index");
                // The rebuild was meant to subsume these; a failed rebuild must
                // not also swallow the batch's known-session edits.
                refresh_ids(store, plan.session_ids).await;
            }
        }
    } else {
        refresh_ids(store, plan.session_ids).await;
    }
    if plan.templates {
        store.index_refresh_templates().await;
        tracing::info!("vault watch: refreshed templates index from external change");
    }
    if plan.people {
        store.index_refresh_people().await;
        tracing::info!("vault watch: refreshed people index from external change");
    }
    if plan.tags {
        store.index_refresh_tags().await;
        tracing::info!("vault watch: refreshed tags index from external change");
    }
}

pub fn spawn(app: AppHandle) {
    let Some(store) = app
        .try_state::<Arc<SessionStore>>()
        .map(|state| state.inner().clone())
    else {
        tracing::error!(
            "vault watch: session store is not managed; external-edit ingestion is disabled"
        );
        return;
    };

    let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<String>();
    FileChanged::listen(&app, move |event| {
        let _ = tx.send(event.payload.path);
    });

    tauri::async_runtime::spawn(async move {
        run(store, rx).await;
    });
}

async fn run(
    store: Arc<SessionStore>,
    mut changed_paths: tokio::sync::mpsc::UnboundedReceiver<String>,
) {
    loop {
        let Some(first) = changed_paths.recv().await else {
            break;
        };
        let mut changed = HashSet::from([first]);

        loop {
            match tokio::time::timeout(COALESCE_WINDOW, changed_paths.recv()).await {
                Ok(Some(path)) => {
                    changed.insert(path);
                }
                Ok(None) => break,
                Err(_elapsed) => break,
            }
        }

        handle_batch(&store, &changed).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session_store::SessionMeta;

    // -- the four brief-mandated routing tests --

    #[test]
    fn own_write_is_ignored_even_if_late() {
        assert!(matches!(
            classify_event("sessions/s1/notes.md", true, Some("s1")),
            WatchAction::Ignore
        ));
    }

    #[test]
    fn external_session_edit_refreshes() {
        assert!(matches!(
            classify_event("sessions/s1/_meta.json", false, Some("s1")),
            WatchAction::Refresh(id) if id == "s1"
        ));
    }

    #[test]
    fn deleted_meta_is_still_only_a_refresh() {
        // refresh_session handles absence by removing index rows; watcher has no delete verb
        assert!(matches!(
            classify_event("sessions/s1/_meta.json", false, Some("s1")),
            WatchAction::Refresh(_)
        ));
    }

    #[test]
    fn non_session_paths_ignored() {
        assert!(matches!(
            classify_event("AGENTS.md", false, None),
            WatchAction::Ignore
        ));
        assert!(matches!(
            classify_event(".trash/2026-07-24/sessions/s1", false, None),
            WatchAction::Ignore
        ));
    }

    // -- additional routing coverage --

    /// Enhanced docs live one level deeper (`sessions/<id>/enhanced/<doc>.md`), but
    /// session-id extraction only looks at the first two segments -- a nested external
    /// edit must still refresh its session, and a deleted doc's trash destination must
    /// stay ignored like all `.trash/` paths.
    #[test]
    fn nested_enhanced_doc_paths_refresh_their_session() {
        assert!(matches!(
            classify_event("sessions/s1/enhanced/doc-1.md", false, Some("s1")),
            WatchAction::Refresh(id) if id == "s1"
        ));
        assert!(matches!(
            classify_event(
                ".trash/2026-07-26/sessions/s1/enhanced/doc-1.md",
                false,
                None
            ),
            WatchAction::Ignore
        ));
        assert!(matches!(
            classify_event(
                "sessions/s1/enhanced/.tmp-1234-5678-doc-1.md",
                false,
                Some("s1")
            ),
            WatchAction::Ignore
        ));
    }

    #[test]
    fn bare_session_folder_path_refreshes() {
        // e.g. a rename's old endpoint: the catalog still owns the exact
        // directory path, so it refreshes as that logical id.
        assert!(matches!(
            classify_event("sessions/s1", false, Some("s1")),
            WatchAction::Refresh(id) if id == "s1"
        ));
    }

    /// The `Refresh` id is whatever the catalog says owns the path -- with
    /// readable directory names the basename is presentation only, and the
    /// logical id (a full UUID) never comes from parsing the path.
    #[test]
    fn readable_dir_refreshes_under_its_catalog_id_not_its_basename() {
        let full_id = "550e8400-e29b-41d4-a716-446655440000";
        assert!(matches!(
            classify_event(
                "sessions/2026-03-20 — Planning — 550e84/notes.md",
                false,
                Some(full_id)
            ),
            WatchAction::Refresh(id) if id == full_id
        ));
    }

    /// Structural paths under `sessions/`: an unknown directory (new, copied,
    /// or a rename's new endpoint) and anything nested in it rebuild instead
    /// of guessing an id from the path.
    #[test]
    fn unknown_session_paths_trigger_rebuild() {
        assert!(matches!(
            classify_event("sessions/2026-03-21 — Copied in — abc123", false, None),
            WatchAction::RebuildSessions
        ));
        assert!(matches!(
            classify_event(
                "sessions/2026-03-21 — Copied in — abc123/_meta.json",
                false,
                None
            ),
            WatchAction::RebuildSessions
        ));
        assert!(matches!(
            classify_event("sessions/Work/unknown dir/nested.md", false, None),
            WatchAction::RebuildSessions
        ));
    }

    /// Phase E1: external edits under `templates/` refresh the in-memory templates
    /// index. Own writes still win, and trashed template files stay ignored.
    #[test]
    fn template_paths_refresh_templates_unless_own_write_or_trash() {
        assert!(matches!(
            classify_event("templates/t-1.json", false, None),
            WatchAction::RefreshTemplates
        ));
        assert!(matches!(
            classify_event("templates/.deleted-defaults.json", false, None),
            WatchAction::RefreshTemplates
        ));
        assert!(matches!(
            classify_event("templates/t-1.json", true, None),
            WatchAction::Ignore
        ));
        assert!(matches!(
            classify_event(".trash/2026-07-26/templates/t-1.json", false, None),
            WatchAction::Ignore
        ));
    }

    /// External edits of the vault-root `people.json` refresh the in-memory people
    /// index. Own writes still win, trashed copies stay ignored, and a session-local
    /// file that happens to be named `people.json` refreshes its session instead.
    #[test]
    fn people_path_refreshes_people_unless_own_write_or_trash() {
        assert!(matches!(
            classify_event("people.json", false, None),
            WatchAction::RefreshPeople
        ));
        assert!(matches!(
            classify_event("people.json", true, None),
            WatchAction::Ignore
        ));
        assert!(matches!(
            classify_event(".trash/2026-08-06/people.json", false, None),
            WatchAction::Ignore
        ));
        assert!(matches!(
            classify_event("sessions/s1/people.json", false, Some("s1")),
            WatchAction::Refresh(id) if id == "s1"
        ));
    }

    /// Same contract as `people.json` for the vault-root `tags.json` registry.
    #[test]
    fn tags_path_refreshes_tags_unless_own_write_or_trash() {
        assert!(matches!(
            classify_event("tags.json", false, None),
            WatchAction::RefreshTags
        ));
        assert!(matches!(
            classify_event("tags.json", true, None),
            WatchAction::Ignore
        ));
        assert!(matches!(
            classify_event(".trash/2026-08-06/tags.json", false, None),
            WatchAction::Ignore
        ));
        assert!(matches!(
            classify_event("sessions/s1/tags.json", false, Some("s1")),
            WatchAction::Refresh(id) if id == "s1"
        ));
    }

    /// Something changed directly at the `sessions/` root (a directory
    /// appearing or disappearing, most likely) -- structural, so rebuild.
    #[test]
    fn bare_sessions_root_triggers_rebuild() {
        assert!(matches!(
            classify_event("sessions", false, None),
            WatchAction::RebuildSessions
        ));
    }

    #[test]
    fn app_db_paths_are_ignored() {
        assert!(matches!(
            classify_event("app.db", false, None),
            WatchAction::Ignore
        ));
        assert!(matches!(
            classify_event("app.db-wal", false, None),
            WatchAction::Ignore
        ));
        for retired in [
            "app.db.pre-files-backup",
            "app.db.pre-files-backup-wal",
            "app.db.pre-files-backup-shm",
        ] {
            assert!(
                matches!(classify_event(retired, false, None), WatchAction::Ignore),
                "{retired} must be ignored"
            );
        }
    }

    /// `migrate.rs`'s one-time-repair marker is gone along with the SQLite
    /// store it repaired, but the file it wrote still sits at the root of
    /// every vault that ran it. Top-level non-session files classify as
    /// `Ignore` by default, so the leftover is inert without a named rule.
    #[test]
    fn the_retired_store_migration_marker_is_inert() {
        assert!(matches!(
            classify_event(".store-migrated-v1", false, None),
            WatchAction::Ignore
        ));
    }

    #[test]
    fn search_index_paths_are_ignored() {
        assert!(matches!(
            classify_event("search_index/abc.term", false, None),
            WatchAction::Ignore
        ));
    }

    #[test]
    fn export_marker_path_is_ignored() {
        assert!(matches!(
            classify_event(LEGACY_EXPORT_MARKER_FILENAME, false, None),
            WatchAction::Ignore
        ));
    }

    /// Tmp/trash rules keep precedence over session classification whether or
    /// not the catalog knows the enclosing directory -- an atomic-write temp
    /// file inside a brand-new session dir must not trigger a rebuild.
    #[test]
    fn tmp_write_paths_under_a_session_are_ignored() {
        assert!(matches!(
            classify_event("sessions/s1/.tmp-1234-5678-notes.md", false, Some("s1")),
            WatchAction::Ignore
        ));
        assert!(matches!(
            classify_event("sessions/unknown dir/.tmp-1234-5678-notes.md", false, None),
            WatchAction::Ignore
        ));
    }

    /// Finder/sync droppings (`.DS_Store`, AppleDouble `._*`, dot-dirs like
    /// `.stversions`) are invisible to layout discovery, so they must never read
    /// as structural changes -- otherwise every Finder browse of the vault would
    /// trigger a full rebuild.
    #[test]
    fn hidden_entries_under_sessions_are_ignored_not_structural() {
        for path in [
            "sessions/.DS_Store",
            "sessions/Work/.DS_Store",
            "sessions/._resource-fork",
            "sessions/.stversions/2026-01-01 — Old — abc123/_meta.json",
            "sessions/s1/.hidden-file",
        ] {
            assert!(
                matches!(classify_event(path, false, None), WatchAction::Ignore),
                "{path}"
            );
        }
        // The bare root itself stays structural.
        assert!(matches!(
            classify_event("sessions", false, None),
            WatchAction::RebuildSessions
        ));
    }

    #[test]
    fn a_user_file_merely_containing_tmp_dash_still_refreshes() {
        assert!(matches!(
            classify_event("sessions/s1/notes.tmp-ideas.md", false, Some("s1")),
            WatchAction::Refresh(id) if id == "s1"
        ));
    }

    #[test]
    fn journal_match_wins_over_an_otherwise_refreshable_path() {
        // Same path as external_session_edit_refreshes, but own-write this time.
        assert!(matches!(
            classify_event("sessions/s1/_meta.json", true, Some("s1")),
            WatchAction::Ignore
        ));
        // An own write also wins over an otherwise-structural unknown path.
        assert!(matches!(
            classify_event("sessions/unknown dir/_meta.json", true, None),
            WatchAction::Ignore
        ));
    }

    // -- integration: real journal, real SessionStore, no live FSEvents stream --

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

    async fn test_store() -> (SessionStore, tempfile::TempDir) {
        let temp = tempfile::tempdir().unwrap();
        let vault = temp.path().to_path_buf();
        let store = SessionStore::new(vault);
        (store, temp)
    }

    #[tokio::test]
    async fn real_journal_end_to_end_own_write_is_ignored() {
        let (store, _vault) = test_store().await;
        store.write_meta(&meta("s1", "One")).await.unwrap();
        store.write_note("s1", "hello").await.unwrap();

        // Simulate the FileChanged event vault_watch would receive for its own note write.
        let changed = HashSet::from(["sessions/s1/notes.md".to_string()]);
        let ids = ids_to_refresh(&store, &changed).await.session_ids;

        assert!(
            ids.is_empty(),
            "own write must never be queued for refresh, even though nothing marked it upstream"
        );
    }

    #[tokio::test]
    async fn real_journal_end_to_end_external_edit_is_queued_for_refresh() {
        let (store, vault) = test_store().await;
        store.write_meta(&meta("s1", "One")).await.unwrap();
        // The created directory has a readable name; the simulated FileChanged path
        // must be the real vault-relative path, resolved through the store.
        let rel = store.session_dir("s1").await.unwrap();

        // Bypass write_meta entirely -- an external editor/sync client would too.
        std::fs::write(
            vault.path().join(&rel).join("_meta.json"),
            serde_json::to_vec_pretty(&meta("s1", "Edited outside")).unwrap(),
        )
        .unwrap();

        let changed = HashSet::from([format!("{}/_meta.json", rel.to_str().unwrap())]);
        let ids = ids_to_refresh(&store, &changed).await.session_ids;

        assert_eq!(ids, HashSet::from(["s1".to_string()]));
    }

    #[tokio::test]
    async fn real_journal_end_to_end_deleted_file_is_still_queued_and_refresh_clears_the_index() {
        let (store, vault) = test_store().await;
        store.write_meta(&meta("s1", "One")).await.unwrap();
        store.write_note("s1", "keep me").await.unwrap();
        let rel = store.session_dir("s1").await.unwrap();
        let dir = vault.path().join(&rel);
        std::fs::remove_file(dir.join("_meta.json")).unwrap();

        let changed = HashSet::from([format!("{}/_meta.json", rel.to_str().unwrap())]);
        let ids = ids_to_refresh(&store, &changed).await.session_ids;
        assert_eq!(ids, HashSet::from(["s1".to_string()]));

        handle_batch(&store, &changed).await;

        assert!(
            store.session_get("s1").is_none(),
            "index entry must be gone"
        );
        assert!(
            dir.join("notes.md").is_file(),
            "the watcher must never touch files -- only the index row is affected"
        );
    }

    #[tokio::test]
    async fn handle_batch_collapses_multiple_paths_for_the_same_session_into_one_refresh() {
        let (store, vault) = test_store().await;
        store.write_meta(&meta("s1", "One")).await.unwrap();
        let rel = store.session_dir("s1").await.unwrap();
        let dir = vault.path().join(&rel);

        std::fs::write(
            dir.join("_meta.json"),
            serde_json::to_vec_pretty(&meta("s1", "Edited outside")).unwrap(),
        )
        .unwrap();
        std::fs::write(dir.join("other.md"), b"note").unwrap();

        let changed = HashSet::from([
            format!("{}/_meta.json", rel.to_str().unwrap()),
            format!("{}/other.md", rel.to_str().unwrap()),
        ]);
        let plan = ids_to_refresh(&store, &changed).await;

        assert_eq!(plan.session_ids, HashSet::from(["s1".to_string()]));
        assert!(!plan.rebuild_sessions);
    }

    // -- readable directory names: catalog-backed classification --

    const READABLE_DIR: &str = "sessions/2026-03-20 — Planning — 6ba7b8";

    fn seed_session_dir(vault: &std::path::Path, relative: &str, meta: &SessionMeta) {
        let dir = vault.join(relative);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("_meta.json"),
            serde_json::to_vec_pretty(meta).unwrap(),
        )
        .unwrap();
    }

    #[tokio::test]
    async fn artifact_edit_under_readable_cataloged_dir_refreshes_its_logical_id() {
        let (store, vault) = test_store().await;
        let id = "6ba7b8aa-1111-2222-3333-444455556666";
        seed_session_dir(vault.path(), READABLE_DIR, &meta(id, "Planning"));
        store.rebuild_index().await.unwrap();

        std::fs::write(vault.path().join(READABLE_DIR).join("_memo.md"), b"edited").unwrap();

        let changed = HashSet::from([format!("{READABLE_DIR}/_memo.md")]);
        let plan = ids_to_refresh(&store, &changed).await;

        assert_eq!(
            plan.session_ids,
            HashSet::from([id.to_string()]),
            "the refresh must carry the full logical id from the catalog, never the basename"
        );
        assert!(!plan.rebuild_sessions);

        handle_batch(&store, &changed).await;
        assert!(store.session_get(id).is_some());
    }

    #[tokio::test]
    async fn unknown_new_session_dir_burst_plans_exactly_one_rebuild_and_indexes_it() {
        let (store, vault) = test_store().await;
        store.rebuild_index().await.unwrap();

        let id = "7ba7b8aa-1111-2222-3333-444455556666";
        let new_dir = "sessions/2026-03-21 — Copied in — 7ba7b8";
        seed_session_dir(vault.path(), new_dir, &meta(id, "Copied in"));
        std::fs::write(vault.path().join(new_dir).join("notes.md"), b"note").unwrap();

        let changed = HashSet::from([
            new_dir.to_string(),
            format!("{new_dir}/_meta.json"),
            format!("{new_dir}/notes.md"),
        ]);
        let plan = ids_to_refresh(&store, &changed).await;

        // One flag for the whole burst == exactly one rebuild_index call in
        // handle_batch, and no per-id refreshes guessed from the paths.
        assert!(plan.rebuild_sessions);
        assert!(plan.session_ids.is_empty());

        handle_batch(&store, &changed).await;
        assert!(
            store.session_get(id).is_some(),
            "the rebuild must have discovered the new directory by its _meta.json id"
        );
    }

    #[tokio::test]
    async fn bare_sessions_root_event_plans_a_rebuild() {
        let (store, _vault) = test_store().await;
        let plan = ids_to_refresh(&store, &HashSet::from(["sessions".to_string()])).await;
        assert!(plan.rebuild_sessions);
    }

    #[tokio::test]
    async fn external_rename_endpoints_keep_the_session_indexed_under_its_full_id() {
        let (store, vault) = test_store().await;
        let id = "8ba7b8aa-1111-2222-3333-444455556666";
        store
            .write_meta(&meta(id, "Renamed outside"))
            .await
            .unwrap();
        store.write_note(id, "keep me").await.unwrap();
        let old_rel = store.session_dir(id).await.unwrap();

        let new_rel = "sessions/2026-03-22 — Renamed by hand — 8ba7b8";
        std::fs::rename(vault.path().join(&old_rel), vault.path().join(new_rel)).unwrap();

        // The watcher sees both rename endpoints: the old one is still
        // cataloged (Refresh), the new one is unknown (RebuildSessions).
        let changed = HashSet::from([old_rel.to_str().unwrap().to_string(), new_rel.to_string()]);
        let plan = ids_to_refresh(&store, &changed).await;
        assert!(plan.rebuild_sessions);

        handle_batch(&store, &changed).await;

        assert!(
            store.session_get(id).is_some(),
            "the rebuild subsumes the old-endpoint refresh -- refreshing the old id alone \
             would have dropped its index rows"
        );
        assert_eq!(
            store.session_dir(id).await.unwrap(),
            std::path::PathBuf::from(new_rel),
            "the catalog must now home the id in the renamed directory"
        );
    }
}
