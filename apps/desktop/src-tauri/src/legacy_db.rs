//! Retirement of the pre-files-model SQLite database.
//!
//! Files are now the only source of truth, so `app.db` is dead weight. It is
//! not deleted -- it is the only copy of anything a rebuild sweep failed to
//! recover from disk -- but it must stop looking like a live database, so it
//! is renamed in place to `app.db.pre-files-backup` (with the WAL/SHM
//! sidecars alongside it). Renaming in place rather than moving keeps the
//! bytes wherever the user put them, including inside a vault.
//!
//! This runs once at startup, where the schema preparation used to. It never
//! fails startup: every error is logged and swallowed, because a stale
//! `app.db` sitting around is harmless -- nothing reads it anymore.

use std::path::{Path, PathBuf};

const DB_FILENAME: &str = "app.db";
const RETIRED_BASENAME: &str = "app.db.pre-files-backup";

/// SQLite sidecars, as (`app.db` suffix, retired-name suffix). The empty pair
/// is the main database file itself.
const SUFFIXES: [(&str, &str); 3] = [("", ""), ("-wal", "-wal"), ("-shm", "-shm")];

/// Every directory a pre-files-model build could have put `app.db` in: the
/// current default app-data base, plus the legacy `data_dir/<identifier>`
/// directory earlier builds used. Both are swept, so a user who has a stale
/// copy in the legacy spot gets it retired too.
fn candidate_dirs(identifier: &str) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Some(default_dir) = hypr_storage::global::compute_default_base(identifier) {
        dirs.push(default_dir);
    }
    if let Some(data_dir) = dirs::data_dir() {
        let identifier_dir = data_dir.join(identifier);
        if !dirs.contains(&identifier_dir) {
            dirs.push(identifier_dir);
        }
    }
    dirs
}

/// Renames any surviving `app.db` (and its sidecars) out of the way. Silent
/// when there is nothing to retire, which is the steady state after the first
/// run.
pub fn retire_app_db(identifier: &str) {
    for dir in candidate_dirs(identifier) {
        retire_in_dir(&dir);
    }
}

fn retire_in_dir(dir: &Path) {
    if !dir.join(DB_FILENAME).is_file() {
        return;
    }

    // The database and its sidecars are retired as one group or not at all. A `-wal` holds
    // transactions that exist nowhere else until it is checkpointed into the database it
    // belongs to, so renaming it away from a live `app.db` (because only *its* backup name
    // was free) would destroy them permanently.
    let mut moves = Vec::new();
    for (from_suffix, to_suffix) in SUFFIXES {
        let from = dir.join(format!("{DB_FILENAME}{from_suffix}"));
        if !from.is_file() {
            continue;
        }
        let to = dir.join(format!("{RETIRED_BASENAME}{to_suffix}"));
        if to.exists() {
            tracing::warn!(
                path = %from.display(),
                backup = %to.display(),
                "legacy app.db backup already exists; leaving the database and its sidecars in place"
            );
            return;
        }
        moves.push((from, to));
    }

    let mut retired = Vec::new();
    for (from, to) in moves {
        match std::fs::rename(&from, &to) {
            Ok(()) => retired.push(to),
            Err(error) => tracing::error!(
                %error,
                path = %from.display(),
                "failed to retire legacy app.db file"
            ),
        }
    }

    if !retired.is_empty() {
        tracing::info!(
            dir = %dir.display(),
            files = retired.len(),
            "retired legacy SQLite database; files are now the only source of truth"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renames_the_database_and_its_sidecars_in_place() {
        let dir = tempfile::tempdir().unwrap();
        for name in ["app.db", "app.db-wal", "app.db-shm"] {
            std::fs::write(dir.path().join(name), name).unwrap();
        }

        retire_in_dir(dir.path());

        assert!(!dir.path().join("app.db").exists());
        assert!(!dir.path().join("app.db-wal").exists());
        assert!(!dir.path().join("app.db-shm").exists());
        assert_eq!(
            std::fs::read_to_string(dir.path().join("app.db.pre-files-backup")).unwrap(),
            "app.db"
        );
        assert_eq!(
            std::fs::read_to_string(dir.path().join("app.db.pre-files-backup-wal")).unwrap(),
            "app.db-wal"
        );
        assert_eq!(
            std::fs::read_to_string(dir.path().join("app.db.pre-files-backup-shm")).unwrap(),
            "app.db-shm"
        );
    }

    #[test]
    fn a_directory_without_a_database_is_left_completely_alone() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("notes.md"), "hi").unwrap();

        retire_in_dir(dir.path());

        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
        assert!(!dir.path().join("app.db.pre-files-backup").exists());
    }

    /// An existing backup is never clobbered -- it is the older, more
    /// complete copy -- and the second run must still not panic.
    #[test]
    fn an_existing_backup_is_not_overwritten() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("app.db"), "new").unwrap();
        std::fs::write(dir.path().join("app.db.pre-files-backup"), "old").unwrap();

        retire_in_dir(dir.path());

        assert_eq!(
            std::fs::read_to_string(dir.path().join("app.db.pre-files-backup")).unwrap(),
            "old"
        );
        assert_eq!(
            std::fs::read_to_string(dir.path().join("app.db")).unwrap(),
            "new"
        );
    }

    /// REGRESSION (reviewer-found): the three renames are one group. With only the main
    /// backup name taken, the old per-file loop still renamed `app.db-wal` away from the
    /// live `app.db` it belongs to -- permanently losing every transaction that lived only
    /// in the WAL, since a detached WAL can never be checkpointed back.
    #[test]
    fn an_existing_backup_keeps_the_wal_and_shm_with_their_database() {
        let dir = tempfile::tempdir().unwrap();
        for name in ["app.db", "app.db-wal", "app.db-shm"] {
            std::fs::write(dir.path().join(name), name).unwrap();
        }
        std::fs::write(dir.path().join("app.db.pre-files-backup"), "old").unwrap();

        retire_in_dir(dir.path());

        for name in ["app.db", "app.db-wal", "app.db-shm"] {
            assert_eq!(
                std::fs::read_to_string(dir.path().join(name)).unwrap(),
                name,
                "{name} must stay next to the database it belongs to"
            );
        }
        assert!(!dir.path().join("app.db.pre-files-backup-wal").exists());
        assert!(!dir.path().join("app.db.pre-files-backup-shm").exists());
        assert_eq!(
            std::fs::read_to_string(dir.path().join("app.db.pre-files-backup")).unwrap(),
            "old",
            "the pre-existing backup is never clobbered"
        );
    }

    /// The retired names must stay inside the vault watcher's `app.db*`
    /// ignore rule, or a restored backup would trigger a refresh storm.
    #[test]
    fn retired_names_stay_within_the_vault_watcher_ignore_rule() {
        for (_, to_suffix) in SUFFIXES {
            let name = format!("{RETIRED_BASENAME}{to_suffix}");
            assert_eq!(
                crate::vault_watch::classify_event(&name, false),
                crate::vault_watch::WatchAction::Ignore,
                "{name} must be ignored by the vault watcher"
            );
        }
    }
}
