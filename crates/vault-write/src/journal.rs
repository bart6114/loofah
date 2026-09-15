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
        if let Ok(mut journal) = self.0.lock() {
            journal.insert(relative.to_string(), hash.to_string());
        }
    }

    pub fn matches_current_file(&self, vault_base: &Path, relative: &str) -> bool {
        let abs = vault_base.join(relative);
        let stored_hash = self.0.lock().ok().and_then(|j| j.get(relative).cloned());

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
    fn journal_records_and_validates_hashes() {
        let journal = WriteJournal::new();
        let data = b"test content";
        let hash = sha256(data);

        journal.record("test.txt", &hash);
        assert!(journal.0.lock().unwrap().contains_key("test.txt"));
        assert_eq!(journal.0.lock().unwrap().len(), 1);
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
