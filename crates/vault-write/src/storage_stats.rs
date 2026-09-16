use std::fs::{self, ReadDir};
use std::path::Path;
use std::time::{Duration, Instant};

use serde::Serialize;

use super::{SessionStore, StoreError};

const CACHE_TTL: Duration = Duration::from_secs(60);
const MAX_ENTRIES: usize = 250_000;
const MAX_DEPTH: usize = 64;
const SCAN_BUDGET: Duration = Duration::from_secs(10);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum VaultStorageCategory {
    #[serde(rename = "mp3")]
    Mp3,
    Wav,
    Images,
    Pdf,
    Json,
    Markdown,
    Other,
}

#[derive(Debug, Clone, PartialEq, Serialize, specta::Type)]
pub struct VaultStorageBucket {
    pub category: VaultStorageCategory,
    pub bytes: u64,
    pub files: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, specta::Type)]
pub struct VaultStorageStats {
    pub total_bytes: u64,
    pub files: u64,
    pub categories: Vec<VaultStorageBucket>,
    pub trash_bytes: u64,
    pub trash_files: u64,
    pub unreadable_entries: u64,
    pub skipped_links: u64,
    pub scan_limited: bool,
    pub measured_at: String,
}

impl SessionStore {
    pub async fn vault_storage_stats(
        &self,
        refresh: bool,
    ) -> Result<VaultStorageStats, StoreError> {
        let requested_at = Instant::now();
        let cache = self.storage_stats.clone();
        let base = self.vault_base.clone();
        // The task owns the lock so cancelling an IPC request cannot start a second disk walk.
        tokio::spawn(async move {
            let mut cached = cache.lock().await;
            if let Some((completed_at, stats)) = cached.as_ref()
                && (*completed_at >= requested_at
                    || (!refresh && completed_at.elapsed() < CACHE_TTL))
            {
                return Ok(stats.clone());
            }
            let stats = tokio::task::spawn_blocking(move || scan(&base, MAX_ENTRIES, SCAN_BUDGET))
                .await
                .map_err(|error| StoreError::Io(format!("storage scan failed: {error}")))??;
            *cached = Some((Instant::now(), stats.clone()));
            Ok(stats)
        })
        .await
        .map_err(|error| StoreError::Io(format!("storage task failed: {error}")))?
    }
}

fn category(path: &Path) -> VaultStorageCategory {
    use VaultStorageCategory::*;
    if path
        .file_name()
        .is_some_and(|name| name.to_string_lossy().starts_with('.'))
    {
        return Other;
    }
    match path
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase()
        .as_str()
    {
        "mp3" => Mp3,
        "wav" | "wave" => Wav,
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "heic" | "heif" | "avif" | "svg" | "tif"
        | "tiff" | "bmp" | "ico" => Images,
        "pdf" => Pdf,
        "json" => Json,
        "md" | "markdown" => Markdown,
        _ => Other,
    }
}

fn scan(
    base: &Path,
    max_entries: usize,
    budget: Duration,
) -> Result<VaultStorageStats, StoreError> {
    use VaultStorageCategory::*;
    let started = Instant::now();
    let root = fs::read_dir(base)
        .map_err(|error| StoreError::Io(format!("cannot measure vault: {error}")))?;
    let mut stats = VaultStorageStats {
        total_bytes: 0,
        files: 0,
        categories: [Mp3, Wav, Images, Pdf, Json, Markdown, Other]
            .into_iter()
            .map(|category| VaultStorageBucket {
                category,
                bytes: 0,
                files: 0,
            })
            .collect(),
        trash_bytes: 0,
        trash_files: 0,
        unreadable_entries: 0,
        skipped_links: 0,
        scan_limited: false,
        measured_at: String::new(),
    };
    // Stream each directory rather than collecting every path; at most MAX_DEPTH handles are open.
    let mut stack: Vec<(ReadDir, bool)> = vec![(root, false)];
    let mut visited = 0;
    while !stack.is_empty() {
        if visited >= max_entries || started.elapsed() >= budget {
            stats.scan_limited = true;
            break;
        }
        let depth = stack.len();
        let (entries, in_trash) = stack.last_mut().unwrap();
        let in_trash = *in_trash;
        let Some(entry) = entries.next() else {
            stack.pop();
            continue;
        };
        visited += 1;
        if visited % 256 == 0 {
            std::thread::sleep(Duration::from_millis(1));
        }
        let Ok(entry) = entry else {
            stats.unreadable_entries += 1;
            continue;
        };
        let path = entry.path();
        if depth == 1
            && (entry.file_name() == ".loofah-lock"
                || entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with(".loofah-recording-"))
        {
            continue;
        }
        let Ok(metadata) = fs::symlink_metadata(&path) else {
            stats.unreadable_entries += 1;
            continue;
        };
        if metadata.is_symlink() {
            stats.skipped_links += 1;
            continue;
        }
        let in_trash = in_trash || (depth == 1 && entry.file_name() == ".trash");
        if metadata.is_dir() {
            if depth >= MAX_DEPTH {
                stats.scan_limited = true;
            } else {
                match fs::read_dir(&path) {
                    Ok(entries) => stack.push((entries, in_trash)),
                    Err(_) => stats.unreadable_entries += 1,
                }
            }
        } else if metadata.is_file() {
            // Legacy iCloud stubs contain no reliable logical size for the original file.
            if path.extension().is_some_and(|ext| ext == "icloud") {
                stats.unreadable_entries += 1;
                continue;
            }
            let file_category = category(&path);
            let bucket = stats
                .categories
                .iter_mut()
                .find(|bucket| bucket.category == file_category)
                .unwrap();
            bucket.bytes += metadata.len();
            bucket.files += 1;
            stats.total_bytes += metadata.len();
            stats.files += 1;
            if in_trash {
                stats.trash_bytes += metadata.len();
                stats.trash_files += 1;
            }
        }
    }
    stats.categories.sort_by(|a, b| b.bytes.cmp(&a.bytes));
    stats.measured_at = chrono::Utc::now().to_rfc3339();
    tracing::debug!(
        entries = visited,
        elapsed_ms = started.elapsed().as_millis(),
        limited = stats.scan_limited,
        "vault storage measured"
    );
    Ok(stats)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_attachments_hidden_files_and_trash_once_without_reading_contents() {
        let vault = tempfile::tempdir().unwrap();
        for (path, bytes) in [
            ("sessions/example/audio.MP3", 100),
            ("sessions/example/audio/audio.wav", 200),
            ("sessions/example/unknown/nested/photo.HEIC", 30),
            ("sessions/example/contract.pdf", 40),
            ("sessions/example/transcript.json", 50),
            ("sessions/example/notes.md", 60),
            ("sessions/example/.hidden.json", 7),
            (".trash/old/audio.mp3", 20),
            (".trash/old/notes.md", 3),
            ("config.json", 10),
            ("sessions/example/empty.txt", 0),
        ] {
            let target = vault.path().join(path);
            fs::create_dir_all(target.parent().unwrap()).unwrap();
            fs::File::create(target).unwrap().set_len(bytes).unwrap();
        }
        let result = scan(vault.path(), MAX_ENTRIES, SCAN_BUDGET).unwrap();
        assert_eq!(result.total_bytes, 520);
        assert_eq!(result.files, 11);
        assert_eq!(result.trash_bytes, 23);
        assert_eq!(result.trash_files, 2);
        assert_eq!(
            result.categories.iter().map(|c| c.bytes).sum::<u64>(),
            result.total_bytes
        );
        assert_eq!(
            result.categories.iter().map(|c| c.files).sum::<u64>(),
            result.files
        );
        assert_eq!(result.categories[0].category, VaultStorageCategory::Wav);
        assert_eq!(
            result
                .categories
                .iter()
                .find(|c| c.category == VaultStorageCategory::Other)
                .unwrap()
                .bytes,
            7
        );
        assert!(!result.scan_limited);
        assert_eq!(result.unreadable_entries, 0);
    }

    #[test]
    fn empty_missing_and_bounded_scans_are_distinct() {
        let vault = tempfile::tempdir().unwrap();
        let empty = scan(vault.path(), MAX_ENTRIES, SCAN_BUDGET).unwrap();
        assert_eq!(empty.total_bytes, 0);
        assert_eq!(empty.categories.len(), 7);
        assert!(!empty.scan_limited);
        assert!(scan(&vault.path().join("missing"), MAX_ENTRIES, SCAN_BUDGET).is_err());
        fs::write(vault.path().join("a.md"), b"a").unwrap();
        fs::write(vault.path().join("b.md"), b"b").unwrap();
        let partial = scan(vault.path(), 1, SCAN_BUDGET).unwrap();
        assert!(partial.scan_limited);
        assert_eq!(partial.files, 1);
        assert!(
            scan(vault.path(), MAX_ENTRIES, Duration::ZERO)
                .unwrap()
                .scan_limited
        );
    }

    #[test]
    fn cloud_stubs_are_reported_as_unmeasured() {
        let vault = tempfile::tempdir().unwrap();
        fs::write(vault.path().join(".audio.mp3.icloud"), b"stub").unwrap();
        let result = scan(vault.path(), MAX_ENTRIES, SCAN_BUDGET).unwrap();
        assert_eq!(result.files, 0);
        assert_eq!(result.unreadable_entries, 1);
    }

    #[cfg(unix)]
    #[test]
    fn never_follows_symlinks_or_reads_special_files() {
        let vault = tempfile::tempdir().unwrap();
        let external = tempfile::NamedTempFile::new().unwrap();
        external.as_file().set_len(1000).unwrap();
        std::os::unix::fs::symlink(external.path(), vault.path().join("outside.mp3")).unwrap();
        std::os::unix::fs::symlink(vault.path(), vault.path().join("loop")).unwrap();
        let _socket = std::os::unix::net::UnixListener::bind(vault.path().join("socket")).unwrap();
        let result = scan(vault.path(), MAX_ENTRIES, SCAN_BUDGET).unwrap();
        assert_eq!(result.total_bytes, 0);
        assert_eq!(result.skipped_links, 2);
        assert!(!result.scan_limited);
    }

    #[tokio::test]
    async fn caches_per_vault_and_refreshes_without_touching_the_index() {
        let vault = tempfile::tempdir().unwrap();
        let store = SessionStore::new(vault.path().into());
        let initial = store.vault_storage_stats(false).await.unwrap();
        fs::write(vault.path().join("attachment.md"), b"hello").unwrap();
        assert_eq!(store.vault_storage_stats(false).await.unwrap(), initial);
        let refreshed = store.vault_storage_stats(true).await.unwrap();
        assert_eq!(refreshed.total_bytes, 5);
        assert!(store.session_ids().is_empty());
        let other = tempfile::tempdir().unwrap();
        assert_eq!(
            SessionStore::new(other.path().into())
                .vault_storage_stats(false)
                .await
                .unwrap()
                .total_bytes,
            0
        );
    }

    #[tokio::test]
    async fn concurrent_requests_share_one_measurement() {
        let vault = tempfile::tempdir().unwrap();
        let store = SessionStore::new(vault.path().into());
        let (first, second) = tokio::join!(
            store.vault_storage_stats(true),
            store.vault_storage_stats(true)
        );
        assert_eq!(first.unwrap(), second.unwrap());
    }

    #[tokio::test]
    async fn a_large_scan_does_not_hold_the_store_write_lock() {
        let vault = tempfile::tempdir().unwrap();
        for index in 0..2_000 {
            fs::File::create(vault.path().join(format!("attachment-{index}.wav")))
                .unwrap()
                .set_len(1_000_000)
                .unwrap();
        }
        let store = SessionStore::new(vault.path().into());
        let _write_guard = store.lock_writes().await.unwrap();
        let measured =
            tokio::time::timeout(Duration::from_secs(5), store.vault_storage_stats(false))
                .await
                .expect("storage accounting must not wait on recording or note writes")
                .unwrap();
        assert_eq!(measured.files, 2_000);
        assert_eq!(measured.total_bytes, 2_000_000_000);
        assert!(!measured.scan_limited);
    }
}
