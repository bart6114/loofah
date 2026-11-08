use super::VaultLease;
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeSet,
    ffi::OsString,
    io,
    path::{Component, Path, PathBuf},
};

const BUNDLES: [&str; 3] = ["io.loofah.stable", "io.loofah.dev", "io.loofah.staging"];

/// Hash an already canonical vault path using the existing Native storage-key recipe.
pub fn canonical_vault_key(canonical_vault: &Path) -> String {
    Sha256::digest(canonical_vault.as_os_str().as_encoded_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

pub fn local_operation_root(
    lease: &VaultLease,
    app_local_data: &Path,
    native_application_root: Option<&Path>,
) -> io::Result<PathBuf> {
    let base = resolve_host_directory(app_local_data)?;
    let applications = BUNDLES
        .map(|bundle| resolve_host_directory(&base.join(bundle)))
        .into_iter()
        .collect::<io::Result<Vec<_>>>()?;
    if let Some(actual) = native_application_root {
        let actual = resolve_host_directory(actual)?;
        if !applications.contains(&actual) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Nonstandard Native application storage cannot be shared safely with the CLI; configure a common supported local backend root before continuing",
            ));
        }
    }
    let key = canonical_vault_key(lease.canonical_vault());
    let mut existing = BTreeSet::new();
    for application in &applications {
        let mut path = application.clone();
        let mut present = true;
        for name in ["backend", key.as_str(), "creates"] {
            path.push(name);
            match std::fs::symlink_metadata(&path) {
                Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {}
                Ok(_) => {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        format!(
                            "Local backend state is not a regular directory: {}",
                            path.display()
                        ),
                    ));
                }
                Err(cause) if cause.kind() == io::ErrorKind::NotFound => present = false,
                Err(cause) => return Err(cause),
            }
        }
        if present {
            existing.insert(path);
        }
    }
    match existing.len() {
        0 => Ok(applications[0].join("backend").join(key).join("creates")),
        1 => Ok(existing.into_iter().next().unwrap()),
        _ => Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            format!(
                "Multiple local backend state directories exist for this vault ({}). Stop all Loofah versions and resolve their recovery state before continuing; no parallel journal was selected",
                existing
                    .iter()
                    .map(|p| p.display().to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        )),
    }
}

fn resolve_host_directory(path: &Path) -> io::Result<PathBuf> {
    if !path.is_absolute()
        || path
            .components()
            .any(|part| matches!(part, Component::ParentDir))
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Local application storage must be an absolute host directory without parent traversal",
        ));
    }
    let mut prefix = path.to_owned();
    let mut missing: Vec<OsString> = Vec::new();
    loop {
        match std::fs::symlink_metadata(&prefix) {
            Ok(_) => {
                // Configured host ancestors may contain aliases; owned descendants may not.
                let mut resolved = prefix.canonicalize()?;
                if !resolved.is_dir() {
                    return Err(io::Error::new(
                        io::ErrorKind::NotADirectory,
                        "Local application storage is not a directory",
                    ));
                }
                for name in missing.into_iter().rev() {
                    resolved.push(name);
                }
                return Ok(resolved);
            }
            Err(cause) if cause.kind() == io::ErrorKind::NotFound => {
                missing.push(
                    prefix
                        .file_name()
                        .ok_or_else(|| {
                            io::Error::new(
                                io::ErrorKind::InvalidInput,
                                "Invalid local application storage root",
                            )
                        })?
                        .to_owned(),
                );
                if missing.len() > 256 {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "Local application storage path has too many components",
                    ));
                }
                prefix.pop();
            }
            Err(cause) => return Err(cause),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vault::LeaseAttempt;

    fn lease(path: &Path) -> VaultLease {
        match VaultLease::try_acquire_existing(path).unwrap() {
            LeaseAttempt::Acquired(lease) => lease,
            LeaseAttempt::Busy => panic!("unexpected owner"),
        }
    }
    fn root(base: &Path, bundle: &str, lease: &VaultLease) -> PathBuf {
        base.join(bundle)
            .join("backend")
            .join(canonical_vault_key(lease.canonical_vault()))
            .join("creates")
    }

    #[test]
    fn no_existing_state_selects_stable_without_creating_directories() {
        let vault = tempfile::tempdir().unwrap();
        let lease = lease(vault.path());
        let base = vault.path().canonicalize().unwrap().join("not-created");
        assert_eq!(
            local_operation_root(&lease, &base, None).unwrap(),
            root(&base, "io.loofah.stable", &lease)
        );
        assert!(!base.exists());
    }

    #[test]
    fn either_host_reuses_one_existing_channel_root_and_preserves_all_bytes() {
        for bundle in BUNDLES {
            let vault = tempfile::tempdir().unwrap();
            let lease = lease(vault.path());
            let base = vault.path().canonicalize().unwrap().join("data");
            let path = root(&base, bundle, &lease);
            std::fs::create_dir_all(&path).unwrap();
            std::fs::write(
                path.join("unknown-intent"),
                b"never guess or merge this file",
            )
            .unwrap();
            for actual in [
                None,
                Some(base.join("io.loofah.stable")),
                Some(base.join("io.loofah.dev")),
                Some(base.join("io.loofah.staging")),
            ] {
                assert_eq!(
                    local_operation_root(&lease, &base, actual.as_deref()).unwrap(),
                    path
                );
            }
            assert_eq!(
                std::fs::read(path.join("unknown-intent")).unwrap(),
                b"never guess or merge this file"
            );
        }
    }

    #[test]
    fn multiple_existing_roots_never_select_or_seed_a_parallel_journal() {
        let vault = tempfile::tempdir().unwrap();
        let lease = lease(vault.path());
        let base = vault.path().canonicalize().unwrap().join("data");
        for bundle in [&BUNDLES[1], &BUNDLES[2]] {
            std::fs::create_dir_all(root(&base, bundle, &lease)).unwrap();
        }
        assert_eq!(
            local_operation_root(&lease, &base, None)
                .unwrap_err()
                .kind(),
            io::ErrorKind::AlreadyExists
        );
        assert!(!root(&base, BUNDLES[0], &lease).exists());
    }

    #[test]
    fn explicit_nonstandard_native_roots_fail_instead_of_shadowing_their_intents() {
        let vault = tempfile::tempdir().unwrap();
        let lease = lease(vault.path());
        let base = vault.path().canonicalize().unwrap().join("data");
        let custom = vault.path().canonicalize().unwrap().join("custom-app");
        std::fs::create_dir(&custom).unwrap();
        std::fs::write(custom.join("intent"), b"preserve").unwrap();
        assert_eq!(
            local_operation_root(&lease, &base, Some(&custom))
                .unwrap_err()
                .kind(),
            io::ErrorKind::InvalidInput
        );
        assert!(!base.exists());
        assert_eq!(std::fs::read(custom.join("intent")).unwrap(), b"preserve");
    }

    #[test]
    fn malformed_known_host_directory_preserves_user_files_and_fails_closed() {
        let vault = tempfile::tempdir().unwrap();
        let lease = lease(vault.path());
        let base = vault.path().canonicalize().unwrap().join("file");
        std::fs::write(&base, b"user file").unwrap();
        assert!(local_operation_root(&lease, &base, None).is_err());
        assert_eq!(std::fs::read(base).unwrap(), b"user file");
    }

    #[test]
    fn relative_or_traversing_host_paths_are_never_interpreted_from_the_cli_cwd() {
        let vault = tempfile::tempdir().unwrap();
        let lease = lease(vault.path());
        assert!(local_operation_root(&lease, Path::new("relative"), None).is_err());
        assert!(
            local_operation_root(
                &lease,
                &vault.path().canonicalize().unwrap().join("missing/../data"),
                None
            )
            .is_err()
        );
    }

    #[cfg(unix)]
    #[test]
    fn configured_base_aliases_resolve_once_and_channel_aliases_do_not_count_twice() {
        let vault = tempfile::tempdir().unwrap();
        let lease = lease(vault.path());
        let base = vault.path().canonicalize().unwrap().join("data");
        let path = root(&base, BUNDLES[1], &lease);
        std::fs::create_dir_all(&path).unwrap();
        std::os::unix::fs::symlink(base.join(BUNDLES[1]), base.join(BUNDLES[0])).unwrap();
        let alias = vault.path().canonicalize().unwrap().join("alias");
        std::os::unix::fs::symlink(&base, &alias).unwrap();
        assert_eq!(
            local_operation_root(&lease, &alias, Some(&alias.join(BUNDLES[0]))).unwrap(),
            path
        );
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_owned_journal_descendants_are_not_followed_or_replaced() {
        let vault = tempfile::tempdir().unwrap();
        let lease = lease(vault.path());
        let base = vault.path().canonicalize().unwrap().join("data");
        let path = root(&base, BUNDLES[0], &lease);
        let outside = vault.path().canonicalize().unwrap().join("outside");
        std::fs::create_dir(&outside).unwrap();
        std::fs::write(outside.join("user"), b"preserve").unwrap();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::os::unix::fs::symlink(&outside, &path).unwrap();
        assert!(local_operation_root(&lease, &base, None).is_err());
        assert_eq!(std::fs::read(outside.join("user")).unwrap(), b"preserve");
    }
}
