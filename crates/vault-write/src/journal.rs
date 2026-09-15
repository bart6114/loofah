use std::collections::HashMap;
use std::path::Path;
use std::sync::Mutex;

#[derive(Debug)]
pub struct WriteJournal(Mutex<HashMap<String, String>>);

impl WriteJournal {
    pub fn new() -> Self {
        Self(Mutex::new(HashMap::new()))
    }

    pub fn record(&self, relative: &str, hash: &str) {
        let relative = hypr_storage::fs::relative_path_key(relative);
        if let Ok(mut journal) = self.0.lock() {
            journal.insert(relative.to_string(), hash.to_string());
        }
    }

    /// Re-home every journal entry under a renamed directory: keys under
    /// `old_prefix` move to `new_prefix` so late filesystem events for the new
    /// paths still match their hashes, and no entry keeps claiming the old path
    /// (a write landing there again would be a new file, not ours).
    pub fn remap_prefix(&self, old_prefix: &str, new_prefix: &str) {
        let old_prefix = hypr_storage::fs::relative_path_key(old_prefix);
        let new_prefix = hypr_storage::fs::relative_path_key(new_prefix);
        let Ok(mut journal) = self.0.lock() else {
            return;
        };
        let moved: Vec<(String, String)> = journal
            .keys()
            .filter_map(|key| {
                let rest = key.strip_prefix(old_prefix.as_ref())?;
                if !rest.is_empty() && !rest.starts_with('/') {
                    return None;
                }
                Some((key.clone(), format!("{new_prefix}{rest}")))
            })
            .collect();
        for (old_key, new_key) in moved {
            if let Some(hash) = journal.remove(&old_key) {
                journal.insert(new_key, hash);
            }
        }
    }

    pub fn matches_current_file(&self, vault_base: &Path, relative: &str) -> bool {
        let relative = hypr_storage::fs::relative_path_key(relative);
        let abs = vault_base.join(relative.as_ref());
        let stored_hash = self
            .0
            .lock()
            .ok()
            .and_then(|j| j.get(relative.as_ref()).cloned());

        if let Ok(current_bytes) = std::fs::read(&abs) {
            let current_hash = sha256(&current_bytes);
            return stored_hash.as_deref() == Some(&current_hash);
        }

        false
    }
}

fn sha256(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    use std::fmt::Write;
    Sha256::digest(bytes)
        .iter()
        .fold(String::with_capacity(64), |mut output: String, byte| {
            write!(output, "{byte:02x}").expect("writing to String cannot fail");
            output
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[cfg(target_os = "windows")]
    fn windows_watch_events_match_native_writes_after_directory_renames() {
        let vault = tempfile::tempdir().unwrap();
        let journal = WriteJournal::new();
        journal.record(r"sessions\old\notes.md", &sha256(b"note"));
        let directory = vault.path().join("sessions/new");
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::write(directory.join("notes.md"), "note").unwrap();
        journal.remap_prefix(r"sessions\old", r"sessions\new");
        assert!(journal.matches_current_file(vault.path(), "sessions/new/notes.md"));
        assert!(journal.matches_current_file(vault.path(), r"sessions\new\notes.md"));
    }

    #[test]
    fn journal_records_and_validates_hashes() {
        let journal = WriteJournal::new();
        let data = b"test content";
        let hash = sha256(data);

        journal.record("test.txt", &hash);
        assert!(journal.0.lock().unwrap().contains_key("test.txt"));
        assert_eq!(journal.0.lock().unwrap().len(), 1);
    }

    #[test]
    fn remap_prefix_moves_entries_under_the_renamed_directory_only() {
        let journal = WriteJournal::new();
        let vault = tempfile::tempdir().unwrap();
        let data = b"bytes";
        journal.record("sessions/old name/notes.md", &sha256(data));
        journal.record("sessions/old name/enhanced/d1.md", &sha256(data));
        journal.record("sessions/old name sibling/notes.md", &sha256(data));

        journal.remap_prefix("sessions/old name", "sessions/new name");

        let new_dir = vault.path().join("sessions/new name/enhanced");
        std::fs::create_dir_all(&new_dir).unwrap();
        std::fs::write(new_dir.join("d1.md"), data).unwrap();
        assert!(journal.matches_current_file(vault.path(), "sessions/new name/enhanced/d1.md"));

        let old_dir = vault.path().join("sessions/old name");
        std::fs::create_dir_all(&old_dir).unwrap();
        std::fs::write(old_dir.join("notes.md"), data).unwrap();
        assert!(
            !journal.matches_current_file(vault.path(), "sessions/old name/notes.md"),
            "the old path must no longer be claimed as an own write"
        );

        // A sibling directory that merely shares the prefix string stays untouched.
        let sibling = vault.path().join("sessions/old name sibling");
        std::fs::create_dir_all(&sibling).unwrap();
        std::fs::write(sibling.join("notes.md"), data).unwrap();
        assert!(journal.matches_current_file(vault.path(), "sessions/old name sibling/notes.md"));
    }

    #[test]
    fn matches_current_file_returns_false_for_missing_file() {
        let journal = WriteJournal::new();
        let vault = tempfile::tempdir().unwrap();
        let data = b"data";
        let hash = sha256(data);
        journal.record("missing.txt", &hash);

        assert!(!journal.matches_current_file(vault.path(), "missing.txt"));
    }

    #[test]
    fn matches_current_file_returns_true_for_unchanged_content() {
        let journal = WriteJournal::new();
        let vault = tempfile::tempdir().unwrap();
        let file_path = vault.path().join("test.txt");

        let data = b"hello world";
        let hash = sha256(data);
        journal.record("test.txt", &hash);
        std::fs::write(&file_path, data).unwrap();

        assert!(journal.matches_current_file(vault.path(), "test.txt"));
    }

    #[test]
    fn matches_current_file_returns_false_for_changed_content() {
        let journal = WriteJournal::new();
        let vault = tempfile::tempdir().unwrap();
        let file_path = vault.path().join("test.txt");

        let hash = sha256(b"hello world");
        journal.record("test.txt", &hash);
        std::fs::write(&file_path, b"goodbye world").unwrap();

        assert!(!journal.matches_current_file(vault.path(), "test.txt"));
    }
}
