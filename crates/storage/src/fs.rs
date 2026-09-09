use std::io::Write;
use std::path::Path;

use tempfile::NamedTempFile;

pub fn rename_with_retry(source: &Path, target: &Path) -> std::io::Result<()> {
    #[cfg(target_os = "windows")]
    {
        // Antivirus and sync clients briefly hold handles without FILE_SHARE_DELETE.
        // Keep the original file in place; never fall back to delete-then-rename.
        for delay_ms in [10, 20, 40, 80, 160, 320] {
            match std::fs::rename(source, target) {
                Ok(()) => return Ok(()),
                Err(error) if matches!(error.raw_os_error(), Some(5 | 32 | 33)) => {
                    std::thread::sleep(std::time::Duration::from_millis(delay_ms));
                }
                Err(error) => return Err(error),
            }
        }
    }
    std::fs::rename(source, target)
}

pub fn atomic_write(target: &Path, content: &str) -> std::io::Result<()> {
    let parent = target.parent().ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::InvalidInput, "target has no parent")
    })?;
    std::fs::create_dir_all(parent)?;

    let mut temp = NamedTempFile::new_in(parent)?;
    temp.write_all(content.as_bytes())?;
    temp.as_file().sync_all()?;
    rename_with_retry(temp.path(), target)?;
    Ok(())
}

pub async fn atomic_write_async(target: &Path, content: &str) -> std::io::Result<()> {
    let target = target.to_path_buf();
    let content = content.to_owned();
    tokio::task::spawn_blocking(move || atomic_write(&target, &content))
        .await
        .map_err(std::io::Error::other)?
}

pub async fn copy_dir_recursive(
    src: &Path,
    dst: &Path,
    skip_filename: Option<&str>,
) -> std::io::Result<()> {
    let mut entries = tokio::fs::read_dir(src).await?;

    while let Some(entry) = entries.next_entry().await? {
        let src_path = entry.path();
        let file_name = entry.file_name();
        let dst_path = dst.join(&file_name);

        if let Some(skip) = skip_filename
            && file_name == skip
        {
            continue;
        }

        let file_type = entry.file_type().await?;
        if file_type.is_dir() {
            tokio::fs::create_dir_all(&dst_path).await?;
            Box::pin(copy_dir_recursive(&src_path, &dst_path, skip_filename)).await?;
        } else {
            tokio::fs::copy(&src_path, &dst_path).await?;
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[cfg(target_os = "windows")]
    #[test]
    fn retries_a_sync_client_handle_without_losing_the_previous_file() {
        use std::os::windows::fs::OpenOptionsExt;
        let dir = tempdir().unwrap();
        let target = dir.path().join("notes.md");
        fs::write(&target, "old note").unwrap();
        let reader = fs::OpenOptions::new()
            .read(true)
            .share_mode(1)
            .open(&target)
            .unwrap();
        let release = std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(100));
            drop(reader);
        });
        atomic_write(&target, "new note").unwrap();
        release.join().unwrap();
        assert_eq!(fs::read_to_string(target).unwrap(), "new note");
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn a_persistent_lock_preserves_the_original_and_cleans_up_staging_files() {
        use std::os::windows::fs::OpenOptionsExt;
        let dir = tempdir().unwrap();
        let target = dir.path().join("notes.md");
        fs::write(&target, "old note").unwrap();
        let _reader = fs::OpenOptions::new()
            .read(true)
            .share_mode(1)
            .open(&target)
            .unwrap();
        assert!(atomic_write(&target, "new note").is_err());
        assert_eq!(fs::read_to_string(target).unwrap(), "old note");
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
    }

    #[test]
    fn atomic_write_creates_file() {
        let temp = tempdir().unwrap();
        let target = temp.path().join("new_file.json");

        atomic_write(&target, r#"{"key": "value"}"#).unwrap();

        assert_eq!(fs::read_to_string(&target).unwrap(), r#"{"key": "value"}"#);
    }

    #[test]
    fn atomic_write_creates_parent_dirs() {
        let temp = tempdir().unwrap();
        let target = temp.path().join("nested").join("dir").join("file.json");

        atomic_write(&target, "content").unwrap();

        assert_eq!(fs::read_to_string(&target).unwrap(), "content");
    }

    #[test]
    fn atomic_write_overwrites_existing() {
        let temp = tempdir().unwrap();
        let target = temp.path().join("file.json");
        fs::write(&target, "old").unwrap();

        atomic_write(&target, "new").unwrap();

        assert_eq!(fs::read_to_string(&target).unwrap(), "new");
    }

    #[tokio::test]
    async fn atomic_write_async_creates_file() {
        let temp = tempdir().unwrap();
        let target = temp.path().join("async_file.json");

        atomic_write_async(&target, r#"{"async": true}"#)
            .await
            .unwrap();

        assert_eq!(fs::read_to_string(&target).unwrap(), r#"{"async": true}"#);
    }

    #[tokio::test]
    async fn atomic_write_async_creates_parent_dirs() {
        let temp = tempdir().unwrap();
        let target = temp.path().join("async").join("nested").join("file.json");

        atomic_write_async(&target, "async content").await.unwrap();

        assert_eq!(fs::read_to_string(&target).unwrap(), "async content");
    }

    #[tokio::test]
    async fn copy_dir_recursive_copies_files() {
        let temp = tempdir().unwrap();
        let src = temp.path().join("src");
        let dst = temp.path().join("dst");

        fs::create_dir_all(&src).unwrap();
        fs::write(src.join("file1.txt"), "content1").unwrap();
        fs::write(src.join("file2.txt"), "content2").unwrap();

        fs::create_dir_all(&dst).unwrap();
        copy_dir_recursive(&src, &dst, None).await.unwrap();

        assert_eq!(
            fs::read_to_string(dst.join("file1.txt")).unwrap(),
            "content1"
        );
        assert_eq!(
            fs::read_to_string(dst.join("file2.txt")).unwrap(),
            "content2"
        );
    }

    #[tokio::test]
    async fn copy_dir_recursive_copies_subdirs() {
        let temp = tempdir().unwrap();
        let src = temp.path().join("src");
        let dst = temp.path().join("dst");

        fs::create_dir_all(src.join("subdir")).unwrap();
        fs::write(src.join("subdir").join("nested.txt"), "nested content").unwrap();

        fs::create_dir_all(&dst).unwrap();
        copy_dir_recursive(&src, &dst, None).await.unwrap();

        assert_eq!(
            fs::read_to_string(dst.join("subdir").join("nested.txt")).unwrap(),
            "nested content"
        );
    }

    #[tokio::test]
    async fn copy_dir_recursive_skips_specified_file() {
        let temp = tempdir().unwrap();
        let src = temp.path().join("src");
        let dst = temp.path().join("dst");

        fs::create_dir_all(&src).unwrap();
        fs::write(src.join("keep.txt"), "keep").unwrap();
        fs::write(src.join("skip.txt"), "skip").unwrap();

        fs::create_dir_all(&dst).unwrap();
        copy_dir_recursive(&src, &dst, Some("skip.txt"))
            .await
            .unwrap();

        assert!(dst.join("keep.txt").exists());
        assert!(!dst.join("skip.txt").exists());
    }
}
