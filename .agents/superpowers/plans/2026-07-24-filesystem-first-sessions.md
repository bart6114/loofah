# Filesystem-First Session Storage Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Session content (meta, note, summaries, transcript, recordings) lives in `sessions/<id>/` files as the single source of truth, written only by a new Rust session store; SQLite is demoted to config + a rebuildable index; humans/orgs, calendar, and cloudsync/e2ee/workspaces are removed.

**Architecture:** A Rust `session_store` module owns all file I/O under `sessions/` (atomic tmp+rename writes, `mkdir -p` semantics, write journal) and updates the derived index tables in the same operation. The watcher and startup rescan feed the same store and may only refresh index rows — never write to the vault. The old bidirectional sync (dirty-queue export worker + files-win reconcile + soft-hide import) is deleted after a one-time final export sweep.

**Tech Stack:** Rust (tauri 2, sqlx/SQLite, tokio, specta commands), TypeScript/React (tanstack-query, Zustand), existing `hypr-fs-sync-core` atomic-write helpers, `hypr-fs-format` transcript schema.

**Spec:** `docs/superpowers/specs/2026-07-24-filesystem-first-sessions-design.md`

## Global Constraints

- After TS changes: `pnpm -F desktop typecheck`. After Rust changes: `cargo check`. After any edit: `pnpm exec dprint fmt`. (AGENTS.md)
- Branch naming: `refactor/` prefix. Work on `refactor/filesystem-first-sessions`.
- Recent merges already landed: chat feature removed, debrand done, field-recorder UI redesign merged. Any file reference under `apps/desktop/src/session/` must be re-verified with grep before editing — the redesign moved UI files.
- No new soft-delete semantics anywhere: session deletion = move folder to `.trash/<date>/`, user-initiated only.
- A write that persists nothing must return an error. `unwrap_or_default()` on content serialization is forbidden.
- Transcript file format stays wire-compatible with today's `TranscriptJson` (`crates/fs-format/src/transcript.rs`) so existing vault files parse unchanged.
- Existing index table names (`sessions`, `session_documents`, `transcripts`) are kept so FE live-queries keep working.

## Phase ordering constraint

The old `vault_export` worker and its renderers must survive until Task 12 (migration sweep) has drained one final export. Do not delete `vault_export.rs`, `vault_watch.rs`, or the import machinery before Task 13.

---

## Phase 1 — Feature removals

### Task 1: Remove contacts/humans/orgs frontend

**Files:**
- Delete: `apps/desktop/src/contacts/` (whole directory: `index.tsx`, `details.tsx`, `humans.tsx`, `new-person-form.tsx`, `organization-details.tsx`, `organization-item.tsx`, `person-item.tsx`, `queries.ts`, `queries.test.tsx`, `shared.tsx`)
- Delete: `apps/desktop/src/sidebar/contacts.tsx`
- Modify: route tree, sidebar composition, shortcuts (`apps/desktop/src/main/useShortcuts.test.tsx` references organizations), transcript speaker-assignment UI
- Test: existing suites must stay green

**Interfaces:**
- Consumes: nothing from other tasks.
- Produces: speaker labels in the transcript UI are plain strings (renaming a speaker edits the hint's `value` string, no `human_id`). Later tasks rely on no FE import of `humans`/`organizations`/`session_participants` remaining.

- [ ] **Step 1: Inventory every reference before deleting**

Run and save output:
```bash
grep -rln "contacts\|humans\|organizations\|session_participants\|human_id" apps/desktop/src --include='*.ts' --include='*.tsx' | grep -v test | sort
```

- [ ] **Step 2: Delete the contacts feature directory and sidebar entry**

```bash
git rm -r apps/desktop/src/contacts apps/desktop/src/sidebar/contacts.tsx
```

- [ ] **Step 3: Remove routes and navigation**

Remove the contacts route from the route tree (search `routeTree.gen.ts` regenerates; edit the route source files it points to), sidebar links, and any `Cmd`-shortcut entries surfaced by:
```bash
grep -rn "contacts" apps/desktop/src --include='*.tsx' --include='*.ts' | grep -v test
```
Expected end state: the grep returns zero hits outside i18n catalogs.

- [ ] **Step 4: Convert speaker assignment to plain-string rename**

In the transcript renderer (verify current paths first):
```bash
grep -rln "assignTranscriptSpeaker\|human_id\|getHumanName" apps/desktop/src/stt apps/desktop/src/session --include='*.ts' --include='*.tsx'
```
- `apps/desktop/src/stt/queries.ts`: keep `assignTranscriptSpeaker` but change its parameter `humanId: string` to `speakerLabel: string`; the hint written into `speaker_hints_json` stores `{ type: "speaker_label", value: <label string> }` instead of a human id. Delete `useTranscriptHumans`, `useSessionParticipantHumanIds`, `useTranscriptLabelContext`'s human lookups — labels render straight from hint values.
- Replace the "assign to person" picker component with an inline text-rename control (`<input>` prefilled with current label, save on Enter/blur calling `assignTranscriptSpeaker`).

- [ ] **Step 5: Typecheck, fix fallout, format**

```bash
pnpm -F desktop typecheck && pnpm exec dprint fmt
```
Expected: PASS. Update/delete tests that asserted humans behavior (`useShortcuts.test.tsx`, transcript label tests) to match plain-string labels.

- [ ] **Step 6: Run the FE test suite**

```bash
pnpm -F desktop test
```
Expected: PASS.

- [ ] **Step 7: Commit**

```bash
git add -A && git commit -m "refactor: remove contacts/humans/orgs feature; speaker labels are plain strings"
```

### Task 2: Remove calendar frontend + notifications

**Files:**
- Delete: `apps/desktop/src/calendar/` (`hooks.ts`, `queries.ts`, `queries.test.ts`, `ignored-events.ts`, `ignored-events.test.tsx`, `calendar-storage.test.tsx`)
- Modify: settings pages (calendar section), notification service (meeting-start notifications), session creation from events, `apps/desktop/src/settings/general/notification.tsx`
- Test: existing suites stay green

**Interfaces:**
- Consumes: nothing.
- Produces: no FE reference to `calendars`/`events` tables, event pickers, or meeting notifications. Sessions are created only manually or by starting a recording.

- [ ] **Step 1: Inventory**

```bash
grep -rln "calendar\|events\b\|event_id" apps/desktop/src --include='*.ts' --include='*.tsx' | grep -v test | sort
grep -rn "calendar" apps/desktop/src-tauri/tauri.conf.json apps/desktop/src-tauri/capabilities/ 2>/dev/null
ls plugins | grep -i -E "calendar|event"
```

- [ ] **Step 2: Delete the calendar directory and prune every consumer**

`git rm -r apps/desktop/src/calendar`. Then work the Step 1 inventory to zero: settings sections, onboarding steps, session-creation-from-event flows, sidebar/timeline event chips, notification scheduling. Keep the generic notification plumbing (used by recording auto-stop notifications in `apps/desktop/src/stt/auto-stop-notification.ts`).

- [ ] **Step 3: Remove any calendar Tauri plugin wiring**

If Step 1 found a calendar plugin (`plugins/*calendar*`): remove plugin registration from `apps/desktop/src-tauri/src/lib.rs` and the workspace member from root `Cargo.toml`.

- [ ] **Step 4: Verify, format, test, commit**

```bash
pnpm -F desktop typecheck && cargo check && pnpm exec dprint fmt && pnpm -F desktop test
git add -A && git commit -m "refactor: remove calendar integration and meeting notifications"
```

### Task 3: Remove calendar + humans/orgs from the Rust data layer

**Files:**
- Delete: `crates/db-app/src/calendar_ops.rs`, `calendar_types.rs`, `event_ops.rs`, `event_types.rs`
- Modify: `crates/db-app/src/lib.rs` (module decls, migration registry, command exports), `crates/db-app/src/session_ops.rs`/`session_types.rs` (strip `event_id`/`event_json`/participants join), `plugins/db/src/commands.rs` (delete calendar/event/human/org commands), search-index triggers
- Create: `crates/db-app/migrations/20260724100000_drop_calendar_humans.sql`
- Test: `cargo test -p hypr-db-app`

**Interfaces:**
- Consumes: Tasks 1–2 (no FE callers left).
- Produces: `sessions` rows without `event_id`/`event_json`; no `humans`/`organizations`/`session_participants`/`calendars`/`events` tables. Later tasks build the index schema on this slimmed model.

- [ ] **Step 1: Write the drop migration**

`crates/db-app/migrations/20260724100000_drop_calendar_humans.sql`:
```sql
DROP TRIGGER IF EXISTS search_index_humans_insert;
DROP TRIGGER IF EXISTS search_index_humans_update;
DROP TRIGGER IF EXISTS search_index_humans_delete;
DROP TRIGGER IF EXISTS search_index_organizations_insert;
DROP TRIGGER IF EXISTS search_index_organizations_update;
DROP TRIGGER IF EXISTS search_index_organizations_delete;
DROP TABLE IF EXISTS session_participants;
DROP TABLE IF EXISTS humans;
DROP TABLE IF EXISTS organizations;
DROP TABLE IF EXISTS events;
DROP TABLE IF EXISTS calendars;
DROP TABLE IF EXISTS calendar_event_tombstones;
```
Check trigger names against `crates/db-app/migrations/20260714120400_search_index_humans_triggers.sql` and `20260714120500_search_index_organizations_triggers.sql` and copy them verbatim. Register the migration in `crates/db-app/src/lib.rs` following the existing `MIGRATIONS` slice pattern.

- [ ] **Step 2: Delete the Rust modules and prune exports**

`git rm` the four files; remove `mod`/`pub use` lines from `crates/db-app/src/lib.rs`; delete calendar/event/human/org Tauri commands from `plugins/db/src/commands.rs` and their specta registrations in `apps/desktop/src-tauri/src/lib.rs`.

- [ ] **Step 3: Strip session query fields**

In `crates/db-app/src/session_ops.rs` and `session_types.rs`: remove `event_id`, `external_event_id`, `series_id`, `event_json` from structs and SQL; remove participant joins/loads from `get_session`/`list_sessions`. Fix compile errors this surfaces in `plugins/db` and `apps/desktop/src-tauri` (search-index projection reads, `vault_export.rs` meta rendering — for now pass empty values into the renderer, it dies in Task 13).

- [ ] **Step 4: Test and commit**

```bash
cargo check && cargo test -p hypr-db-app && pnpm -F desktop typecheck && pnpm exec dprint fmt
git add -A && git commit -m "refactor(db): drop calendar, events, humans, organizations, participants"
```

### Task 4: Remove cloudsync/e2ee/workspaces/sharing

**Files:**
- Delete: `crates/db-app/src/cloudsync.rs`, `crates/db-app/src/e2ee.rs`, `plugins/db/src/e2ee_witness.rs`
- Modify: `crates/db-app/src/lib.rs`, `plugins/db/src/lib.rs`, `plugins/db/src/runtime.rs`, `plugins/db/src/commands.rs`, `apps/desktop/src-tauri/src/db.rs`, `apps/desktop/src-tauri/src/lib.rs` (drop `init_with_cloudsync` in favor of a plain `init`), FE billing/sharing surfaces found by grep
- Create: `crates/db-app/migrations/20260724110000_drop_cloud_tables.sql`
- Test: `cargo test -p hypr-db-app -p tauri-plugin-db`

**Interfaces:**
- Consumes: Task 3 (compiles without calendar/humans).
- Produces: no `workspace_id` anywhere in session/document/transcript structs or SQL; DB plugin initializes without cloudsync. Task 7+ index writes assume this.

- [ ] **Step 1: Drop migration**

`crates/db-app/migrations/20260724110000_drop_cloud_tables.sql`:
```sql
DROP TABLE IF EXISTS cloudsync_session_evictions;
DROP TABLE IF EXISTS cloudsync_writable_workspaces;
DROP TABLE IF EXISTS e2ee_local_device;
DROP TABLE IF EXISTS e2ee_local_state;
DROP TABLE IF EXISTS e2ee_records;
DROP TABLE IF EXISTS e2ee_witness_records;
DROP TABLE IF EXISTS e2ee_witness_state;
DROP TABLE IF EXISTS session_share_sync_state;
DROP TABLE IF EXISTS shared_session_cache;
DROP TABLE IF EXISTS shared_session_attachment_cache;
DROP TABLE IF EXISTS attachment_transfer_jobs;
DROP TABLE IF EXISTS attachment_local_state;
DROP TABLE IF EXISTS session_attachments;
DROP TABLE IF EXISTS workspace_memberships;
DROP TABLE IF EXISTS workspaces;
```
Register in the `MIGRATIONS` slice.

- [ ] **Step 2: Delete modules, strip `workspace_id`**

`git rm` the three files. Remove every `workspace_id` column reference from `session_ops.rs`, `session_types.rs`, `legacy_import.rs`, `plugins/db/src/commands.rs` SQL, and the FE (`grep -rn "workspace" apps/desktop/src --include='*.ts' --include='*.tsx' | grep -v test`). Replace `init_with_cloudsync` in `apps/desktop/src-tauri/src/lib.rs` with the plain init path (keep the synchronous `sync_from_vault` call — it dies in Task 13, not here).

- [ ] **Step 3: Verify no stragglers, test, commit**

```bash
grep -rn "cloudsync\|e2ee\|workspace" crates/db-app/src plugins/db/src apps/desktop/src-tauri/src --include='*.rs' | grep -v test
```
Expected: zero hits (comments included — delete them). Then:
```bash
cargo check && cargo test -p hypr-db-app -p tauri-plugin-db && pnpm -F desktop typecheck && pnpm exec dprint fmt
git add -A && git commit -m "refactor: rip out cloudsync, e2ee, workspaces, sharing, attachments"
```

---

## Phase 2 — Session store

### Task 5: Session store scaffold — paths, atomic writes, write journal

**Files:**
- Create: `apps/desktop/src-tauri/src/session_store/mod.rs`, `session_store/paths.rs`, `session_store/journal.rs`
- Modify: `apps/desktop/src-tauri/src/lib.rs` (declare module)
- Test: inline `#[cfg(test)]` in each file

**Interfaces:**
- Consumes: `hypr_fs_sync_core::export::{write_file_atomic, tmp_sibling_path, move_to_trash}`.
- Produces (later tasks build on these exact signatures):

```rust
pub struct SessionStore {
    vault_base: PathBuf,
    pool: SqlitePool,
    journal: journal::WriteJournal,
    live: tokio::sync::Mutex<HashMap<String, LiveTranscriptBuffer>>, // Task 8
}
pub enum StoreError { Io(String), Db(String), Serialize(String) } // impl Display + From<sqlx::Error>

// paths.rs — all relative to vault_base
pub fn session_dir(id: &str) -> PathBuf;          // sessions/<id>
pub fn meta_path(id: &str) -> PathBuf;            // sessions/<id>/_meta.json
pub fn note_path(id: &str) -> PathBuf;            // sessions/<id>/_memo.md
pub fn document_path(id: &str, kind: &str) -> PathBuf; // sessions/<id>/<kind>.md
pub fn transcript_path(id: &str) -> PathBuf;      // sessions/<id>/transcript.json
pub fn audio_dir(id: &str) -> PathBuf;            // sessions/<id>/audio

// journal.rs
pub struct WriteJournal(Mutex<HashMap<String, String>>); // relative path -> sha256 of last written bytes
impl WriteJournal {
    pub fn record(&self, relative: &str, bytes: &[u8]);
    pub fn matches_current_file(&self, vault_base: &Path, relative: &str) -> bool; // hash file on disk, compare
}

impl SessionStore {
    pub fn new(vault_base: PathBuf, pool: SqlitePool) -> Self;
    /// mkdir -p parent, write tmp, rename, record in journal. Never trashes the target first.
    async fn write_file(&self, relative: PathBuf, bytes: Vec<u8>) -> Result<(), StoreError>;
}
```

- [ ] **Step 1: Write failing tests for `write_file` + journal**

In `session_store/mod.rs` tests (pattern: `tempfile::tempdir` as vault, in-memory sqlite pool via `hypr_db_core` test helper — copy the setup used in `vault_export.rs` tests):
```rust
#[tokio::test]
async fn write_file_creates_parents_and_is_atomic() {
    let (store, vault) = test_store().await;
    store.write_file(paths::note_path("s1"), b"hello".to_vec()).await.unwrap();
    assert_eq!(std::fs::read(vault.join("sessions/s1/_memo.md")).unwrap(), b"hello");
    // no tmp leftovers
    assert_eq!(std::fs::read_dir(vault.join("sessions/s1")).unwrap().count(), 1);
}

#[tokio::test]
async fn journal_recognizes_own_write_and_external_change() {
    let (store, vault) = test_store().await;
    store.write_file(paths::note_path("s1"), b"hello".to_vec()).await.unwrap();
    assert!(store.journal.matches_current_file(&vault, "sessions/s1/_memo.md"));
    std::fs::write(vault.join("sessions/s1/_memo.md"), b"edited outside").unwrap();
    assert!(!store.journal.matches_current_file(&vault, "sessions/s1/_memo.md"));
}
```

- [ ] **Step 2: Run to verify failure** — `cargo test -p hypr-desktop session_store` — expected: compile error (module missing).

- [ ] **Step 3: Implement `paths.rs`, `journal.rs`, `SessionStore::new` + `write_file`**

`write_file`: `tokio::task::spawn_blocking` around `std::fs::create_dir_all(parent)` + `hypr_fs_sync_core::export::write_file_atomic(&vault_base, &abs, &tmp, &bytes)`; on success `journal.record(relative_str, &bytes)`. Do **not** call any trash helper. `matches_current_file`: `sha256(std::fs::read(abs).ok()?) == stored`, absent file → `false`.

- [ ] **Step 4: Run tests** — `cargo test -p hypr-desktop session_store` — expected: PASS.

- [ ] **Step 5: Commit** — `git add -A && git commit -m "feat(store): session store scaffold with atomic writes and write journal"`

### Task 6: Meta + note + document read/write with index upsert

**Files:**
- Create: `apps/desktop/src-tauri/src/session_store/content.rs`
- Modify: `session_store/mod.rs`
- Test: inline

**Interfaces:**
- Consumes: Task 5 `write_file`, `paths::*`.
- Produces:

```rust
#[derive(serde::Serialize, serde::Deserialize, specta::Type, Clone)]
pub struct SessionMeta {
    pub id: String,
    pub title: String,
    pub started_at: Option<String>,
    pub ended_at: Option<String>,
    pub created_at: String,
    pub tags: Vec<String>,
}
impl SessionStore {
    pub async fn write_meta(&self, meta: &SessionMeta) -> Result<(), StoreError>;
    pub async fn read_meta(&self, id: &str) -> Result<Option<SessionMeta>, StoreError>;
    pub async fn write_note(&self, id: &str, markdown: &str) -> Result<(), StoreError>;
    pub async fn read_note(&self, id: &str) -> Result<Option<String>, StoreError>;
    pub async fn write_document(&self, id: &str, kind: &str, markdown: &str) -> Result<(), StoreError>;
    pub async fn delete_session(&self, id: &str) -> Result<(), StoreError>; // move_to_trash(session_dir)
}
```
Index rules (same transaction-shaped sequence: file write first, index second):
- `write_meta` upserts `sessions (id, title, started_at, ended_at, created_at, updated_at)`.
- `write_note` upserts `session_documents (id = <session_id>, session_id, kind='note', body_format='md', body, updated_at)`.
- `write_document(kind)` upserts `session_documents (id = <session_id>:<kind>, kind, ...)`.
- `delete_session` moves the folder to `.trash/<date>/` and deletes the session's index rows (`sessions`, `session_documents`, `transcripts` by `session_id`).

- [ ] **Step 1: Failing test — round-trip + index row**

```rust
#[tokio::test]
async fn write_meta_writes_file_and_index() {
    let (store, vault) = test_store().await;
    store.write_meta(&meta("s1", "Jury feedback")).await.unwrap();
    assert!(vault.join("sessions/s1/_meta.json").is_file());
    let title: String = sqlx::query_scalar("SELECT title FROM sessions WHERE id='s1'")
        .fetch_one(store.pool()).await.unwrap();
    assert_eq!(title, "Jury feedback");
    assert_eq!(store.read_meta("s1").await.unwrap().unwrap().title, "Jury feedback");
}
```
Plus equivalents for note and document, and `delete_session_moves_folder_to_trash_and_clears_index`.

- [ ] **Step 2: Run to verify failure**, **Step 3: implement**, **Step 4: run to pass** — `cargo test -p hypr-desktop session_store`.

`_meta.json` serialization: `serde_json::to_vec_pretty(meta)` — serialization error is `StoreError::Serialize`, never a default.

- [ ] **Step 5: Commit** — `git commit -am "feat(store): meta/note/document write-through with index upsert"`

### Task 7: Transcript buffer — append, debounced flush, forced flush

**Files:**
- Create: `apps/desktop/src-tauri/src/session_store/transcript.rs`
- Modify: `session_store/mod.rs`
- Test: inline

**Interfaces:**
- Consumes: Task 5 `write_file`; `hypr_fs_format::transcript::{TranscriptJson, TranscriptWithData, TranscriptWord, TranscriptSpeakerHint}`.
- Produces:

```rust
#[derive(serde::Deserialize, specta::Type, Clone)]
pub struct TranscriptDelta {
    pub transcript_id: String,
    pub new_words: Vec<TranscriptWord>,
    pub replaced_ids: Vec<String>,
    pub new_hints: Vec<TranscriptSpeakerHint>,
    pub started_at_ms: f64,
}
pub struct LiveTranscriptBuffer { /* words, hints, dirty flag, transcript_id, started_at_ms */ }
impl SessionStore {
    /// Buffers the delta and schedules a flush ~1s later. Never fails on missing folder/index row.
    pub async fn append_transcript(&self, session_id: &str, delta: TranscriptDelta) -> Result<(), StoreError>;
    /// Writes transcript.json from the buffer (or re-reads existing file and merges), updates transcripts index row.
    pub async fn flush_transcript(&self, session_id: &str) -> Result<(), StoreError>;
    pub async fn flush_all(&self) -> Result<(), StoreError>; // app-exit hook
    /// Replace a whole transcript (batch/upload path) — writes file + index in one call.
    pub async fn write_transcript(&self, session_id: &str, t: TranscriptWithData) -> Result<(), StoreError>;
}
```
Flush details: the debounce is a `tokio::spawn`ed task per session started on first buffered delta (`tokio::time::sleep(Duration::from_secs(1))` then flush if still dirty). `flush_transcript` reads any existing `transcript.json`, replaces/creates the entry whose `id == transcript_id` from the buffer, serializes the full `TranscriptJson` — a word that fails to serialize returns `StoreError::Serialize`, never `[]`. Index upsert: `transcripts (id, session_id, started_at_ms, memo, words_json, speaker_hints_json, updated_at)` with **no session-existence gate**.

- [ ] **Step 1: Failing tests**

```rust
#[tokio::test]
async fn append_then_flush_writes_words_to_file_and_index() {
    let (store, vault) = test_store().await;
    store.append_transcript("s1", delta_with_words(&["hello", "world"])).await.unwrap();
    store.flush_transcript("s1").await.unwrap();
    let json: serde_json::Value =
        serde_json::from_slice(&std::fs::read(vault.join("sessions/s1/transcript.json")).unwrap()).unwrap();
    assert_eq!(json["transcripts"][0]["words"].as_array().unwrap().len(), 2);
    let words: String = sqlx::query_scalar("SELECT words_json FROM transcripts WHERE session_id='s1'")
        .fetch_one(store.pool()).await.unwrap();
    assert!(words.contains("hello"));
}

/// REGRESSION for the 2026-07-23 data loss: no index row, no folder, no _meta.json — words still land.
#[tokio::test]
async fn recording_into_unknown_session_still_persists() {
    let (store, vault) = test_store().await;
    // deliberately: no write_meta, no sessions row
    store.append_transcript("ghost", delta_with_words(&["survives"])).await.unwrap();
    store.flush_transcript("ghost").await.unwrap();
    assert!(vault.join("sessions/ghost/transcript.json").is_file());
}

#[tokio::test]
async fn debounce_flushes_without_explicit_flush() {
    let (store, vault) = test_store().await;
    store.append_transcript("s1", delta_with_words(&["auto"])).await.unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(1500)).await;
    assert!(vault.join("sessions/s1/transcript.json").is_file());
}
```

- [ ] **Step 2: Run to verify failure**, **Step 3: implement**, **Step 4: run to pass** — `cargo test -p hypr-desktop session_store`.

- [ ] **Step 5: Commit** — `git commit -am "feat(store): live transcript buffer with debounced flush; regression test for silent-loss incident"`

### Task 8: Index rebuild — the startup scan

**Files:**
- Create: `apps/desktop/src-tauri/src/session_store/rebuild.rs`
- Modify: `session_store/mod.rs`
- Test: inline

**Interfaces:**
- Consumes: Tasks 5–7 read/parse helpers.
- Produces:

```rust
pub struct RebuildReport { pub sessions: usize, pub notes: usize, pub transcripts: usize, pub errors: Vec<String> }
impl SessionStore {
    /// One-way: scan sessions/*/ -> upsert index rows; delete index rows whose folder is gone.
    /// Never writes to the vault. Unparseable file -> logged in report.errors, row left as-is.
    pub async fn rebuild_index(&self) -> Result<RebuildReport, StoreError>;
    /// Watcher + focus entry point: re-read one session's files, refresh its index rows.
    /// Missing _meta.json -> delete the session's index rows. Never touches files.
    pub async fn refresh_session(&self, session_id: &str) -> Result<(), StoreError>;
}
```

- [ ] **Step 1: Failing tests**

```rust
#[tokio::test]
async fn rebuild_is_idempotent() {
    let (store, _vault) = test_store().await;
    store.write_meta(&meta("s1", "One")).await.unwrap();
    store.write_note("s1", "# hi").await.unwrap();
    // index_dump helper: SELECT * from the three index tables, ordered by id
    store.rebuild_index().await.unwrap();
    let first = index_dump(store.pool()).await;
    store.rebuild_index().await.unwrap();
    assert_eq!(first, index_dump(store.pool()).await);
}

#[tokio::test]
async fn rebuild_from_empty_db_restores_index_from_files() {
    let (store, vault) = test_store().await;
    store.write_meta(&meta("s1", "One")).await.unwrap();
    sqlx::query("DELETE FROM sessions").execute(store.pool()).await.unwrap();
    store.rebuild_index().await.unwrap();
    let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM sessions").fetch_one(store.pool()).await.unwrap();
    assert_eq!(n, 1);
}

#[tokio::test]
async fn refresh_missing_meta_removes_index_row_but_no_files() {
    let (store, vault) = test_store().await;
    store.write_meta(&meta("s1", "One")).await.unwrap();
    store.write_note("s1", "keep me").await.unwrap();
    std::fs::remove_file(vault.join("sessions/s1/_meta.json")).unwrap();
    store.refresh_session("s1").await.unwrap();
    let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM sessions WHERE id='s1'").fetch_one(store.pool()).await.unwrap();
    assert_eq!(n, 0);
    assert!(vault.join("sessions/s1/_memo.md").is_file()); // vault untouched
}
```

- [ ] **Step 2: fail**, **Step 3: implement** (scan = `read_dir(sessions/)`, parse `_meta.json`/`_memo.md`/`*.md`/`transcript.json`, upsert; collect folder ids, delete index rows not in the set), **Step 4: pass**.

- [ ] **Step 5: Commit** — `git commit -am "feat(store): one-way index rebuild and per-session refresh"`

### Task 9: Tauri commands + frontend write-path rewiring

**Files:**
- Create: `apps/desktop/src-tauri/src/session_store/commands.rs`
- Modify: `apps/desktop/src-tauri/src/lib.rs` (manage `SessionStore`, register commands in the specta builder, `flush_all` in the exit handler), `apps/desktop/src/stt/queries.ts`, `apps/desktop/src/stt/useStartListening.ts`, note editor save path, session create/delete mutations
- Test: FE tests + manual smoke

**Interfaces:**
- Consumes: Tasks 6–8.
- Produces specta commands (exact names the FE bindings use):
`session_write_meta(meta: SessionMeta)`, `session_write_note(session_id: String, markdown: String)`, `session_write_document(session_id: String, kind: String, markdown: String)`, `session_append_transcript(session_id: String, delta: TranscriptDelta)`, `session_flush_transcript(session_id: String)`, `session_write_transcript(session_id: String, transcript: TranscriptWithData)`, `session_delete(session_id: String)`, `session_read_note(session_id: String) -> Option<String>`, `session_rebuild_index() -> RebuildReport`.

- [ ] **Step 1: Implement commands.rs** — thin `#[tauri::command] #[specta::specta]` wrappers mapping `StoreError` to `String`. Register in `lib.rs`'s existing `specta_builder`; `app.manage(Arc<SessionStore>)` in setup (constructed with the db pool + `settings().vault_base()`); call `store.flush_all()` in the exit/`on_window_event` close path alongside existing shutdown work.

- [ ] **Step 2: Regenerate bindings** — run the existing bindings generation (`pnpm -F desktop tauri:dev` boot or the repo's specta export task; check `plugins/windows/js/bindings.gen.ts` pattern for where desktop bindings land).

- [ ] **Step 3: Rewire live transcript persistence**

`apps/desktop/src/stt/useStartListening.ts` `handlePersist` (currently lines 381–410) becomes:
```ts
const handlePersist: LiveTranscriptPersistCallback = (delta) => {
  if (delta.new_words.length === 0 && delta.replaced_ids.length === 0) return;
  if (!transcriptId) transcriptId = id();
  trackTranscriptWrite(
    commands.sessionAppendTranscript(sessionId, {
      transcript_id: transcriptId,
      new_words: delta.new_words,
      replaced_ids: delta.replaced_ids,
      new_hints: delta.new_hints ?? [],
      started_at_ms: startedAt,
    }),
  );
};
```
In `onStopped`: `await commands.sessionFlushTranscript(sessionId)` before the enhance step. `trackTranscriptWrite`'s error handler additionally raises a visible toast (`sonnerToast.error("Transcript is NOT being saved: " + error)`) — a persistence failure must never be console-only again.

- [ ] **Step 4: Rewire the other writers**

- `apps/desktop/src/stt/queries.ts`: `createTranscript`/`createLiveTranscript`/`appendTranscriptWordsAndHints`/`mutateTranscript` DB-SQL bodies are replaced by `sessionWriteTranscript`/`sessionAppendTranscript` command calls; `softDeleteTranscript` becomes a `sessionWriteTranscript` with the entry removed. Read hooks (`useSessionTranscripts`, `useTranscript`) stay on the index tables unchanged.
- Note editor save (find with `grep -rn "session_documents" apps/desktop/src/session apps/desktop/src/editor* --include='*.ts*' | grep -i "update\|insert"` — post-redesign path): save calls `sessionWriteNote`; note load calls `sessionReadNote` (file-canonical) with the index as fallback for lists/previews.
- Session create/delete mutations (`apps/desktop/src/session/queries.ts` `softDeleteSession` etc.): create → `sessionWriteMeta`; delete → `sessionDelete`; remove `deleted_at` filters from list queries.
- Enhance/summary success path (`apps/desktop/src/store/zustand/ai-task/task-configs/enhance-success.ts`): summary write → `sessionWriteDocument(sessionId, "summary", md)`.
- Audio: recording output path moves to `sessions/<id>/audio/<startedAt>.wav` — change the catalog step in `onStopped` (`catalogLocalSessionAudio`) to move the finished file there via a small `session_store` command `session_store_audio(session_id, source_path) -> String` (implement beside the others: `std::fs::rename` into `audio_dir`, fallback copy+delete across volumes); retention deletion targets that folder.

- [ ] **Step 5: Markdown round-trip test** — add an FE test beside the editor save path: load a `_memo.md` fixture containing headings, bold, lists, and a code fence into the editor model, serialize back, assert byte-equal output (or document the exact normalizations if TipTap reorders anything — the test pins them so external edits aren't churned).

- [ ] **Step 6: Verify** — `pnpm -F desktop typecheck && cargo check && pnpm exec dprint fmt && pnpm -F desktop test`. Manual smoke via `pnpm -F @hypr/desktop tauri:dev`: create note, type, record 10s, confirm `sessions/<id>/transcript.json` grows ~1s after words appear, quit mid-recording and confirm the file survives.

- [ ] **Step 7: Commit** — `git commit -am "feat(store): FE writes through session store commands; loud persistence errors"`

### Task 10: Startup rescan + focus rescan replace sync_from_vault

**Files:**
- Modify: `apps/desktop/src-tauri/src/lib.rs` (replace the db plugin's `sync_from_vault` startup call with `store.rebuild_index()`; add focus-rescan), `plugins/db/src/lib.rs` (stop exporting/calling `sync_from_vault` from plugin setup)
- Test: `cargo test` + manual

**Interfaces:**
- Consumes: Task 8 `rebuild_index`.
- Produces: startup order = migrations → `rebuild_index` → UI; `tauri::WindowEvent::Focused(true)` triggers a debounced (5s min interval) `rebuild_index`.

- [ ] **Step 1: Swap the startup call** — in the db plugin setup (find `import::import_legacy_data` / `sync_from_vault` call in `plugins/db/src/lib.rs`), remove it; in app `setup()` after `app.manage(SessionStore)`, run `hypr_tauri_utils::block_on(store.rebuild_index())` and `tracing::info!` the report. Wire `WindowEvent::Focused(true)` in `on_window_event` to a `tokio::spawn(store.rebuild_index())` guarded by an `Instant` throttle.

- [ ] **Step 2: Verify + commit** — `cargo check && cargo test -p tauri-plugin-db`; boot dev app twice, second boot must show unchanged index (report: 0 errors). `git commit -am "feat(store): startup and focus rescans replace sync_from_vault"`

### Task 11: Watcher rewrite — index-only, journal-filtered

**Files:**
- Modify: `apps/desktop/src-tauri/src/vault_watch.rs` (rewrite body; keep the notify-plugin subscription plumbing), delete its calls into `tauri_plugin_db::import_paths`
- Test: inline Rust test for the routing logic + manual

**Interfaces:**
- Consumes: Task 5 journal, Task 8 `refresh_session`.
- Produces: watcher pipeline = event path → if under `sessions/<id>/` → if `journal.matches_current_file` → drop → else `store.refresh_session(id)`. No other verbs. Non-session paths ignored.

- [ ] **Step 1: Failing test** — factor the routing decision into a pure function and test it:
```rust
pub enum WatchAction { Ignore, Refresh(String) }
pub fn classify_event(relative: &str, journal_match: bool) -> WatchAction;

#[test]
fn own_write_is_ignored_even_if_late() {
    assert!(matches!(classify_event("sessions/s1/_memo.md", true), WatchAction::Ignore));
}
#[test]
fn external_session_edit_refreshes() {
    assert!(matches!(classify_event("sessions/s1/_meta.json", false), WatchAction::Refresh(id) if id == "s1"));
}
#[test]
fn deleted_meta_is_still_only_a_refresh() {
    // refresh_session handles absence by removing index rows; watcher has no delete verb
    assert!(matches!(classify_event("sessions/s1/_meta.json", false), WatchAction::Refresh(_)));
}
#[test]
fn non_session_paths_ignored() {
    assert!(matches!(classify_event("AGENTS.md", false), WatchAction::Ignore));
    assert!(matches!(classify_event(".trash/2026-07-24/sessions/s1", false), WatchAction::Ignore));
}
```

- [ ] **Step 2: fail → implement → pass** — `cargo test -p hypr-desktop vault_watch`.

- [ ] **Step 3: Manual verification of the incident scenario** — dev app running, edit `sessions/<id>/_memo.md` externally → UI refreshes; `mv` the folder away → session disappears from list, **no** trash activity, `mv` it back → session reappears; start recording, `rm` `_meta.json` mid-recording → transcript keeps writing.

- [ ] **Step 4: Commit** — `git commit -am "refactor(watch): index-only watcher with journal own-write filtering"`

### Task 12: One-time migration — final export sweep

**Files:**
- Create: `apps/desktop/src-tauri/src/session_store/migrate.rs`
- Modify: `apps/desktop/src-tauri/src/lib.rs` (run before first `rebuild_index`)
- Test: inline

**Interfaces:**
- Consumes: the *old* export machinery (`vault_export::enqueue_all_entities` + drain — still alive until Task 13), old DB tables.
- Produces: marker file `.store-migrated-v1` at vault root; sweep runs exactly once.

- [ ] **Step 1: Failing test** — seed old-style rows (session + transcript with int-ms words, the shape the int/float bug never exported), run `migrate::run_once`, assert `sessions/<id>/transcript.json` exists with the words and the marker file exists; run again, assert file mtimes unchanged (second run is a no-op).

- [ ] **Step 2: Implement** — `run_once(app, pool, vault_base)`: if marker exists → return. Else call the existing `enqueue_all_entities(pool)` then drain synchronously (reuse the worker's drain loop directly, not the spawned task), then write the marker via the store. Fix the int/float word issue at the source while sweeping: the sweep parses `words_json` leniently (`serde_json::Value`, coercing int→float) before rendering, so nothing is dropped.

- [ ] **Step 3: pass → wire into `lib.rs` startup before `rebuild_index` → commit** — `git commit -am "feat(store): one-time final export sweep with marker"`

### Task 13: Delete the old machinery

**Files:**
- Delete: `apps/desktop/src-tauri/src/vault_export.rs` (worker; keep only renderers the sweep inlined — move any still-needed render helpers into `session_store/`), `plugins/db/src/import/` (whole module: `mod.rs`, `legacy_vault.rs`, `calendars.rs`, `events.rs`, `templates.rs`), `crates/db-app/src/legacy_import.rs`
- Create: `crates/db-app/migrations/20260724120000_drop_sync_machinery.sql`
- Modify: `plugins/db/src/lib.rs`, `crates/db-app/src/lib.rs`, `apps/desktop/src-tauri/src/lib.rs` (remove `vault_export::spawn`, `export_vault_now` command, settings "Re-export all files" button → now calls `session_rebuild_index`)
- Test: full suite

**Interfaces:**
- Consumes: Tasks 9–12 complete (nothing calls the old paths).
- Produces: repo has no files-win reconcile, no dirty queue, no soft-hide. `grep -rn "sync_from_vault\|vault_export_dirty\|external_soft_hide\|reconcile" --include='*.rs' crates plugins apps` returns zero hits.

- [ ] **Step 1: Drop migration**

```sql
DROP TABLE IF EXISTS vault_export_dirty;
DROP TABLE IF EXISTS migration_import_runs;
DROP TABLE IF EXISTS migration_import_items;
DROP TABLE IF EXISTS migration_import_targets;
DROP TABLE IF EXISTS storage_migration_state;
-- index tables lose soft-delete/import columns: recreate clean (they are derived; Task 10's rebuild repopulates)
DROP TABLE IF EXISTS sessions;
DROP TABLE IF EXISTS session_documents;
DROP TABLE IF EXISTS transcripts;
CREATE TABLE sessions (
  id TEXT PRIMARY KEY NOT NULL, title TEXT NOT NULL DEFAULT '',
  started_at TEXT, ended_at TEXT,
  created_at TEXT NOT NULL DEFAULT '', updated_at TEXT NOT NULL DEFAULT ''
);
CREATE TABLE session_documents (
  id TEXT PRIMARY KEY NOT NULL, session_id TEXT NOT NULL,
  kind TEXT NOT NULL DEFAULT 'note', title TEXT NOT NULL DEFAULT '',
  body_format TEXT NOT NULL DEFAULT 'md', body TEXT NOT NULL DEFAULT '',
  updated_at TEXT NOT NULL DEFAULT ''
);
CREATE TABLE transcripts (
  id TEXT PRIMARY KEY NOT NULL, session_id TEXT NOT NULL,
  started_at_ms REAL NOT NULL DEFAULT 0, memo TEXT NOT NULL DEFAULT '',
  words_json TEXT NOT NULL DEFAULT '[]', speaker_hints_json TEXT NOT NULL DEFAULT '[]',
  updated_at TEXT NOT NULL DEFAULT ''
);
```
Re-check the search-index trigger migrations (`20260714120100`–`120300`) — recreate those triggers after the table recreation in this same migration file (copy their bodies verbatim, minus any dropped columns).

- [ ] **Step 2: Delete the modules, prune callers, fix compile** — work `cargo check` to green; FE: remove `deleted_at` from every remaining session/transcript query (`grep -rn "deleted_at" apps/desktop/src --include='*.ts*'`), retarget the Settings→Storage "Re-export all files" button (`apps/desktop/src/settings/general/storage/reexport-all.tsx`) to `sessionRebuildIndex`.

- [ ] **Step 3: Full verification**

```bash
cargo check && cargo test --workspace && pnpm -F desktop typecheck && pnpm -F desktop test && pnpm exec dprint fmt
grep -rn "sync_from_vault\|vault_export_dirty\|external_soft_hide\|import_paths\|reconcile" crates plugins apps/desktop/src-tauri --include='*.rs' | grep -v target
```
Expected: suites PASS; grep empty.

- [ ] **Step 4: Commit** — `git commit -am "refactor: delete bidirectional sync machinery; index tables recreated clean"`

### Task 14: fs-format hardening + end-to-end QA

**Files:**
- Modify: `crates/fs-sync-core/src/export.rs` (`render_transcripts` — only if still referenced after Task 13's renderer consolidation; otherwise delete the file’s dead parts), `crates/fs-format/src/transcript.rs`
- Test: unit + `qa-critical-ux` manual pass

- [ ] **Step 1: Kill the silent-empty pattern** — any remaining `unwrap_or_default()` on content deserialization in surviving render/parse helpers becomes a returned error. Add a unit test: a word list containing one malformed entry produces `Err`, not `words: []`.

- [ ] **Step 2: Delete now-dead code** — `grep -rn "render_session_meta\|render_transcripts\|render_chat" crates/fs-sync-core/src` — anything with zero callers goes, with its tests.

- [ ] **Step 3: Full QA pass** — run the `qa-critical-ux` skill flow **minus its calendar section** (feature removed): note creation, recording, live transcript visible, quit-and-restart shows the transcript (the original bug's scenario), summary generation, external `_memo.md` edit round-trip, delete session → folder in `.trash`.

- [ ] **Step 4: Format, commit, wrap up branch**

```bash
pnpm exec dprint fmt && git add -A && git commit -m "chore: harden fs-format parsing; remove dead renderers"
```
Then use superpowers:finishing-a-development-branch to merge `refactor/filesystem-first-sessions` into `main`.
