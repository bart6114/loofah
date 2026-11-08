use std::path::{Path, PathBuf};

pub const VAULT_CONFIG_FILENAME: &str = "global.json";
pub const DEV_BUNDLE_ID: &str = "io.loofah.dev";
pub const STABLE_BUNDLE_ID: &str = "io.loofah.stable";
pub const STAGING_BUNDLE_ID: &str = "io.loofah.staging";
const RELEASE_APP_FOLDER: &str = "loofah";
const LEGACY_DEV_APP_FOLDER: &str = "org.freemeetingtranscriber.dev";
const LEGACY_RELEASE_APP_FOLDER: &str = "free-meeting-transcriber";
const LEGACY_STABLE_APP_FOLDER: &str = "org.freemeetingtranscriber.stable";
const LEGACY_STAGING_APP_FOLDER: &str = "org.freemeetingtranscriber.staging";

pub fn compute_vault_config_path(base: &Path) -> PathBuf {
    base.join(VAULT_CONFIG_FILENAME)
}

pub fn compute_default_base(bundle_id: &str) -> Option<PathBuf> {
    if let Some(value) = std::env::var_os("LOOFAH_GLOBAL_BASE") {
        return explicit_base(&value);
    }
    let data_dir = dirs::data_dir()?;
    let app_folder = resolve_app_folder(bundle_id, cfg!(debug_assertions));
    let current = data_dir.join(app_folder);
    let legacy =
        legacy_app_folder(bundle_id, cfg!(debug_assertions)).map(|name| data_dir.join(name));

    Some(match legacy {
        Some(legacy) => migrate_legacy_base(&legacy, &current),
        None => current,
    })
}

fn explicit_base(value: &std::ffi::OsStr) -> Option<PathBuf> {
    let path = PathBuf::from(value);
    (path.is_absolute() && path.to_str().is_some() && (!path.exists() || path.is_dir()))
        .then_some(path)
}

/// Dev builds and the staging channel use the raw bundle id as the folder
/// name; release builds always use the `loofah` folder.
fn resolve_app_folder<'a>(bundle_id: &'a str, is_debug: bool) -> &'a str {
    if is_debug || bundle_id == STAGING_BUNDLE_ID {
        return bundle_id;
    }

    RELEASE_APP_FOLDER
}

fn legacy_app_folder(bundle_id: &str, is_debug: bool) -> Option<&'static str> {
    match bundle_id {
        DEV_BUNDLE_ID => Some(LEGACY_DEV_APP_FOLDER),
        STAGING_BUNDLE_ID => Some(LEGACY_STAGING_APP_FOLDER),
        STABLE_BUNDLE_ID if is_debug => Some(LEGACY_STABLE_APP_FOLDER),
        STABLE_BUNDLE_ID => Some(LEGACY_RELEASE_APP_FOLDER),
        _ => None,
    }
}

fn migrate_legacy_base(legacy: &Path, current: &Path) -> PathBuf {
    if !legacy.is_dir() || legacy == current {
        return current.to_path_buf();
    }

    if !current.exists() {
        return match std::fs::rename(legacy, current) {
            Ok(()) => current.to_path_buf(),
            Err(_) => legacy.to_path_buf(),
        };
    }

    if merge_directory(legacy, current).is_ok() {
        current.to_path_buf()
    } else if compute_vault_config_path(current).exists() {
        current.to_path_buf()
    } else {
        legacy.to_path_buf()
    }
}

fn merge_directory(source: &Path, target: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(target)?;
    for entry in std::fs::read_dir(source)? {
        let entry = entry?;
        let source_path = entry.path();
        let target_path = target.join(entry.file_name());
        if !target_path.exists() {
            std::fs::rename(&source_path, &target_path)?;
        } else if source_path.is_dir() && target_path.is_dir() {
            merge_directory(&source_path, &target_path)?;
        }
    }
    let _ = std::fs::remove_dir(source);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_global_base_never_falls_back_to_or_migrates_an_existing_profile() {
        let root = tempfile::tempdir().unwrap();
        let isolated = root.path().join("isolated");
        assert_eq!(explicit_base(isolated.as_os_str()), Some(isolated.clone()));
        assert!(!isolated.exists());
        for invalid in ["", ".", "relative/profile"] {
            assert_eq!(explicit_base(std::ffi::OsStr::new(invalid)), None);
        }
        let file = root.path().join("file");
        std::fs::write(&file, "untouched").unwrap();
        assert_eq!(explicit_base(file.as_os_str()), None);
        assert_eq!(std::fs::read(&file).unwrap(), b"untouched");
    }

    #[test]
    fn resolve_app_folder_uses_new_folder_for_new_stable_installs() {
        assert_eq!(
            resolve_app_folder(STABLE_BUNDLE_ID, false),
            RELEASE_APP_FOLDER
        );
    }

    #[test]
    fn resolve_app_folder_returns_bundle_id_for_staging() {
        assert_eq!(
            resolve_app_folder(STAGING_BUNDLE_ID, false),
            STAGING_BUNDLE_ID
        );
    }

    #[test]
    fn resolve_app_folder_returns_bundle_id_in_debug_builds() {
        assert_eq!(resolve_app_folder(STABLE_BUNDLE_ID, true), STABLE_BUNDLE_ID);
    }

    #[test]
    fn maps_new_identifiers_to_legacy_app_folders() {
        assert_eq!(
            legacy_app_folder(STABLE_BUNDLE_ID, false),
            Some(LEGACY_RELEASE_APP_FOLDER)
        );
        assert_eq!(
            legacy_app_folder(STAGING_BUNDLE_ID, false),
            Some(LEGACY_STAGING_APP_FOLDER)
        );
        assert_eq!(
            legacy_app_folder(DEV_BUNDLE_ID, true),
            Some(LEGACY_DEV_APP_FOLDER)
        );
        assert_eq!(
            legacy_app_folder(STABLE_BUNDLE_ID, true),
            Some(LEGACY_STABLE_APP_FOLDER)
        );
    }

    #[test]
    fn migrates_an_untouched_legacy_base_by_renaming_it() {
        let dir = tempfile::tempdir().unwrap();
        let legacy = dir.path().join(LEGACY_RELEASE_APP_FOLDER);
        let current = dir.path().join(RELEASE_APP_FOLDER);
        std::fs::create_dir_all(&legacy).unwrap();
        std::fs::write(compute_vault_config_path(&legacy), "{}").unwrap();

        assert_eq!(migrate_legacy_base(&legacy, &current), current);
        assert!(compute_vault_config_path(&current).is_file());
        assert!(!legacy.exists());
    }

    #[test]
    fn merges_legacy_data_into_an_existing_new_base() {
        let dir = tempfile::tempdir().unwrap();
        let legacy = dir.path().join(LEGACY_RELEASE_APP_FOLDER);
        let current = dir.path().join(RELEASE_APP_FOLDER);
        std::fs::create_dir_all(legacy.join("models")).unwrap();
        std::fs::create_dir_all(&current).unwrap();
        std::fs::write(compute_vault_config_path(&legacy), "{}").unwrap();
        std::fs::write(legacy.join("models/model.bin"), "model").unwrap();

        assert_eq!(migrate_legacy_base(&legacy, &current), current);
        assert!(compute_vault_config_path(&current).is_file());
        assert!(current.join("models/model.bin").is_file());
    }
}
