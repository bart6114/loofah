use std::collections::HashMap;
use std::io::Read;
use std::path::Path;
use std::sync::Mutex;

use crate::{file_snapshot::FileSnapshot, media::open_regular_file};

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

    pub fn matches_current_file(&self, vault_base: &Path, relative: &str) -> bool {
        let relative = hypr_storage::fs::relative_path_key(relative);
        let abs = vault_base.join(relative.as_ref());
        let stored_hash = self
            .0
            .lock()
            .ok()
            .and_then(|j| j.get(relative.as_ref()).cloned());

        let Some(stored_hash) = stored_hash else {
            return false;
        };
        let Ok(Some(file)) = open_regular_file(&abs) else {
            return false;
        };
        let Ok(snapshot) = FileSnapshot::new(file) else {
            return false;
        };
        matches_snapshot(&abs, &stored_hash, snapshot).unwrap_or(false)
    }
}

fn matches_snapshot(
    path: &Path,
    expected: &str,
    snapshot: FileSnapshot,
) -> Result<bool, crate::StoreError> {
    let length = snapshot.file().metadata().map_err(io_error)?.len();
    let actual = hash_reader(snapshot.file(), length).map_err(io_error)?;
    if actual.as_deref() != Some(expected) {
        return Ok(false);
    }
    let Some(current) = open_regular_file(path)? else {
        return Ok(false);
    };
    snapshot.unchanged(&FileSnapshot::new(current)?)
}

fn hash_reader(reader: impl Read, length: u64) -> std::io::Result<Option<String>> {
    use sha2::{Digest, Sha256};
    let Some(bound) = length.checked_add(1) else {
        return Ok(None);
    };
    let mut reader = reader.take(bound);
    let mut buffer = [0; 64 * 1024];
    let mut total = 0;
    let mut hash = Sha256::new();
    loop {
        let count = reader.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        total += count as u64;
        if total > length {
            return Ok(None);
        }
        hash.update(&buffer[..count]);
    }
    Ok((total == length).then(|| {
        hash.finalize()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect()
    }))
}

fn io_error(error: std::io::Error) -> crate::StoreError {
    crate::StoreError::Io(error.to_string())
}

#[cfg(test)]
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
    fn windows_watch_events_match_native_writes() {
        let vault = tempfile::tempdir().unwrap();
        let journal = WriteJournal::new();
        journal.record(r"sessions\one\notes.md", &sha256(b"note"));
        let directory = vault.path().join("sessions/one");
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::write(directory.join("notes.md"), "note").unwrap();
        assert!(journal.matches_current_file(vault.path(), "sessions/one/notes.md"));
        assert!(journal.matches_current_file(vault.path(), r"sessions\one\notes.md"));
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

    #[test]
    fn journal_hash_is_bounded_and_refuses_growth_truncation_and_changed_tails() {
        struct BoundedReader<R>(R);
        impl<R: Read> Read for BoundedReader<R> {
            fn read(&mut self, bytes: &mut [u8]) -> std::io::Result<usize> {
                assert!(bytes.len() <= 64 * 1024);
                self.0.read(bytes)
            }
        }
        let mut bytes = vec![7; 256 * 1024 + 13];
        let expected = sha256(&bytes);
        assert_eq!(
            hash_reader(BoundedReader(bytes.as_slice()), bytes.len() as u64).unwrap(),
            Some(expected.clone())
        );
        assert_eq!(hash_reader(std::io::repeat(7), 13).unwrap(), None);
        assert_eq!(hash_reader(&bytes[..12], 13).unwrap(), None);
        *bytes.last_mut().unwrap() = 8;
        assert_ne!(
            hash_reader(BoundedReader(bytes.as_slice()), bytes.len() as u64).unwrap(),
            Some(expected)
        );
    }

    #[test]
    fn a_replaced_named_file_cannot_match_an_old_opened_journal_generation() {
        let vault = tempfile::tempdir().unwrap();
        let path = vault.path().join("recorded.wav");
        std::fs::write(&path, b"same bytes").unwrap();
        let initial = FileSnapshot::new(std::fs::File::open(&path).unwrap()).unwrap();
        std::fs::rename(&path, vault.path().join("previous.wav")).unwrap();
        std::fs::write(&path, b"same bytes").unwrap();
        assert!(!matches_snapshot(&path, &sha256(b"same bytes"), initial).unwrap());
    }
}
