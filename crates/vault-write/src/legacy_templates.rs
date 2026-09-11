use std::{io, path::Path};

/// Archive retired app-owned files without following links or touching vault attachments.
/// No completion marker: failures and templates restored by sync are retried next startup.
pub fn archive(vault_base: &Path) -> io::Result<usize> {
    let directory = vault_base.join("templates");
    match std::fs::symlink_metadata(&directory) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => return Ok(0),
        Ok(_) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(0),
        Err(error) => return Err(error),
    }
    let mut archived = 0;
    let mut failures = Vec::new();
    for entry in std::fs::read_dir(directory)? {
        let result = (|| -> io::Result<()> {
            let entry = entry?;
            if !entry.file_type()?.is_file() {
                return Ok(());
            }
            let name = entry.file_name();
            let Some(name) = name.to_str() else {
                return Ok(());
            };
            if name != ".deleted-defaults.json"
                && (name.starts_with('.') || !name.ends_with(".json"))
            {
                return Ok(());
            }
            let path = entry.path();
            match hypr_fs_sync_core::export::move_to_trash(vault_base, &path) {
                Ok(Some(_)) => archived += 1,
                Ok(None) => {}
                Err(error) => return Err(io::Error::other(format!("{}: {error}", path.display()))),
            }
            Ok(())
        })();
        if let Err(error) = result {
            tracing::warn!(%error, "could not archive legacy template; source retained");
            failures.push(error.to_string());
        }
    }
    if failures.is_empty() {
        Ok(archived)
    } else {
        Err(io::Error::other(failures.join("; ")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fresh_vault_is_unchanged() {
        let vault = tempfile::tempdir().unwrap();
        assert_eq!(archive(vault.path()).unwrap(), 0);
        assert!(!vault.path().join("templates").exists());
        assert!(!vault.path().join(".trash").exists());
    }

    #[test]
    fn archives_legacy_files_once_and_preserves_other_content() {
        let vault = tempfile::tempdir().unwrap();
        let base = vault.path();
        std::fs::create_dir_all(base.join("templates/subdirectory.json")).unwrap();
        std::fs::create_dir_all(base.join("sessions/s/enhanced")).unwrap();
        for (path, content) in [
            ("templates/custom.json", "custom template bytes"),
            ("templates/.deleted-defaults.json", "tombstones"),
            ("templates/.hidden.json", "hidden"),
            ("templates/attachment.txt", "attachment"),
            (
                "sessions/s/enhanced/summary.md",
                "---\ntemplate_id: custom\nkind: template_output\n---\nSaved summary",
            ),
        ] {
            std::fs::write(base.join(path), content).unwrap();
        }
        assert_eq!(archive(base).unwrap(), 2);
        assert_eq!(archive(base).unwrap(), 0);
        let trash = base
            .join(".trash")
            .join(chrono::Utc::now().format("%Y-%m-%d").to_string())
            .join("templates");
        assert_eq!(
            std::fs::read_to_string(trash.join("custom.json")).unwrap(),
            "custom template bytes"
        );
        assert_eq!(
            std::fs::read_to_string(trash.join(".deleted-defaults.json")).unwrap(),
            "tombstones"
        );
        assert_eq!(
            std::fs::read_to_string(base.join("templates/attachment.txt")).unwrap(),
            "attachment"
        );
        assert!(base.join("templates/.hidden.json").is_file());
        assert!(base.join("templates/subdirectory.json").is_dir());
        assert_eq!(
            std::fs::read_to_string(base.join("sessions/s/enhanced/summary.md")).unwrap(),
            "---\ntemplate_id: custom\nkind: template_output\n---\nSaved summary"
        );
        std::fs::write(base.join("templates/custom.json"), "restored template").unwrap();
        assert_eq!(archive(base).unwrap(), 1);
        assert_eq!(
            std::fs::read_to_string(trash.join("custom.json")).unwrap(),
            "custom template bytes"
        );
        assert_eq!(
            std::fs::read_to_string(trash.join("custom-1.json")).unwrap(),
            "restored template"
        );
    }

    #[test]
    fn failed_archive_keeps_source_and_retries() {
        let vault = tempfile::tempdir().unwrap();
        let base = vault.path();
        std::fs::create_dir(base.join("templates")).unwrap();
        std::fs::write(base.join("templates/custom.json"), "original").unwrap();
        std::fs::write(base.join(".trash"), "blocked").unwrap();
        assert!(archive(base).is_err());
        assert_eq!(
            std::fs::read_to_string(base.join("templates/custom.json")).unwrap(),
            "original"
        );
        std::fs::remove_file(base.join(".trash")).unwrap();
        assert_eq!(archive(base).unwrap(), 1);
        assert_eq!(archive(base).unwrap(), 0);
    }

    #[cfg(unix)]
    #[test]
    fn skips_symlink_files_and_directory() {
        use std::os::unix::fs::symlink;
        let vault = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("custom.json"), "external").unwrap();
        symlink(outside.path(), vault.path().join("templates")).unwrap();
        assert_eq!(archive(vault.path()).unwrap(), 0);
        std::fs::remove_file(vault.path().join("templates")).unwrap();
        std::fs::create_dir(vault.path().join("templates")).unwrap();
        symlink(
            outside.path().join("custom.json"),
            vault.path().join("templates/custom.json"),
        )
        .unwrap();
        assert_eq!(archive(vault.path()).unwrap(), 0);
        assert!(vault.path().join("templates/custom.json").is_symlink());
        assert_eq!(
            std::fs::read_to_string(outside.path().join("custom.json")).unwrap(),
            "external"
        );
    }
}
