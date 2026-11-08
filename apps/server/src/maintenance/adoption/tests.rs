use super::*;
use crate::auth::Identity;
use std::path::PathBuf;

struct Fixture {
    _root: tempfile::TempDir,
    storage: PathBuf,
    source: PathBuf,
    output: PathBuf,
    vault: String,
}
impl Fixture {
    async fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let storage = root.path().join("server");
        let source = root.path().join("local");
        let output = root.path().join("adopted");
        fs::create_dir(&source).unwrap();
        let catalog = Catalog::open(&storage).await.unwrap();
        let owner = Identity {
            issuer: "https://company.example".into(),
            subject: "operator-owner".into(),
            name: "Owner".into(),
            expires_at: crate::auth::now() + 3600,
        };
        catalog.provision(&owner).await.unwrap();
        let target = catalog
            .create_shared(
                &owner,
                &uuid::Uuid::new_v4().to_string(),
                "Provisioned empty vault",
            )
            .await
            .unwrap();
        fs::create_dir_all(storage.join("vaults").join(&target.id)).unwrap();
        let target_root = storage.join("vaults").join(&target.id);
        fs::write(
            target_root.join("AGENTS.md"),
            hypr_vault_write::agents_doc::agents_doc_content(),
        )
        .unwrap();
        fs::create_dir(target_root.join("sessions")).unwrap();
        fs::write(
            storage.join("unknown-service-file"),
            b"keep original service bytes",
        )
        .unwrap();
        fs::create_dir_all(source.join("sessions/legacy-name/empty-user-directory")).unwrap();
        fs::write(
            source.join("sessions/legacy-name/_memo.md"),
            b"Legacy note body\n",
        )
        .unwrap();
        fs::write(
            source.join("sessions/legacy-name/unknown.bin"),
            b"unknown bytes\0\xff",
        )
        .unwrap();
        fs::write(
            source.join("sessions/legacy-name/.hidden"),
            b"private dot bytes",
        )
        .unwrap();
        fs::write(source.join("config.json"), br#"{"theme":"dark"}"#).unwrap();
        drop(catalog);
        Self {
            _root: root,
            storage,
            source,
            output,
            vault: target.id,
        }
    }
    async fn run(&self) -> Result<AdoptionReport> {
        adopt_vault(&self.storage, &self.vault, &self.source, &self.output).await
    }
}
fn canonical(root: &Path) -> Inventory {
    copy::inventory(root)
        .unwrap()
        .into_iter()
        .filter(|(name, _)| !name.ends_with(".loofah-vault.lock") && name != ".loofah-server.lock")
        .collect()
}

#[tokio::test]
async fn adoption_preserves_catalog_roles_original_roots_and_all_source_bytes_and_modes() {
    let f = Fixture::new().await;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(
            f.source.join("sessions/legacy-name/unknown.bin"),
            fs::Permissions::from_mode(0o640),
        )
        .unwrap();
        fs::set_permissions(
            f.source.join("sessions/legacy-name/empty-user-directory"),
            fs::Permissions::from_mode(0o750),
        )
        .unwrap();
    }
    let source = canonical(&f.source);
    let server = canonical(&f.storage);
    let report = f.run().await.unwrap();
    assert_eq!(report.vault_id, f.vault);
    assert_eq!(canonical(&f.source), source);
    assert_eq!(canonical(&f.storage), server);
    assert_eq!(canonical(&f.output.join("vaults").join(&f.vault)), source);
    assert_eq!(
        fs::read(f.output.join("catalog.json")).unwrap(),
        fs::read(f.storage.join("catalog.json")).unwrap()
    );
    assert_eq!(
        fs::read(f.output.join("unknown-service-file")).unwrap(),
        b"keep original service bytes"
    );
    assert!(!f.output.join(".loofah-server.lock").exists());
    assert!(f.run().await.is_err());
    assert_eq!(canonical(&f.output.join("vaults").join(&f.vault)), source);
}

#[tokio::test]
async fn target_must_be_catalog_bound_and_fresh_even_without_any_sessions() {
    let f = Fixture::new().await;
    assert!(
        adopt_vault(
            &f.storage,
            &uuid::Uuid::new_v4().to_string(),
            &f.source,
            &f.output
        )
        .await
        .is_err()
    );
    for file in [
        "config.json",
        "people.json",
        ".trash/old-note",
        "user-file",
        "sessions/empty/_meta.json",
    ] {
        let path = f.storage.join("vaults").join(&f.vault).join(file);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, b"").unwrap();
        assert!(f.run().await.is_err(), "accepted target {file}");
        assert!(!f.output.exists());
        fs::remove_file(path).unwrap();
        if file.starts_with(".trash/") {
            fs::remove_dir(f.storage.join("vaults").join(&f.vault).join(".trash")).unwrap();
        }
        if file.starts_with("sessions/empty/") {
            fs::remove_dir(
                f.storage
                    .join("vaults")
                    .join(&f.vault)
                    .join("sessions/empty"),
            )
            .unwrap();
        }
    }
    let agents = f.storage.join("vaults").join(&f.vault).join("AGENTS.md");
    fs::write(&agents, b"operator customization").unwrap();
    assert!(f.run().await.is_err());
}

#[tokio::test]
async fn any_runtime_work_or_unknown_state_refuses_adoption_and_known_locks_must_be_free() {
    let f = Fixture::new().await;
    let runtime = f.storage.join("runtime").join(&f.vault).join("creates");
    for file in [
        "jobs/request.json",
        "captures/capture/manifest.json",
        "member-media/user/admission.json",
        "provider-credentials/credential.json",
        "unknown/.lock",
    ] {
        let path = runtime.join(file);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, b"").unwrap();
        assert!(f.run().await.is_err(), "accepted runtime {file}");
        assert!(!f.output.exists());
        fs::remove_file(path).unwrap();
    }
    fs::create_dir_all(runtime.join("capture-processing")).unwrap();
    let path = runtime.join("capture-processing/.lock");
    fs::write(&path, b"").unwrap();
    let lock = File::options().read(true).write(true).open(&path).unwrap();
    lock.try_lock().unwrap();
    assert!(f.run().await.is_err());
    drop(lock);
    f.run().await.unwrap();
    assert_eq!(
        fs::read(
            f.output
                .join("runtime")
                .join(&f.vault)
                .join("creates/capture-processing/.lock")
        )
        .unwrap(),
        b""
    );
}

#[tokio::test]
async fn service_local_and_destination_vault_owners_block_copy_before_output() {
    let f = Fixture::new().await;
    let catalog = Catalog::open(&f.storage).await.unwrap();
    assert!(f.run().await.is_err());
    drop(catalog);
    let source = lease(&f.source).unwrap();
    assert!(f.run().await.is_err());
    drop(source);
    let target = lease(&f.storage.join("vaults").join(&f.vault)).unwrap();
    assert!(f.run().await.is_err());
    drop(target);
    assert!(!f.output.exists());
    assert!(
        adopt_vault(&f.storage, &f.vault, &f.source, &f.source.join("nested"))
            .await
            .is_err()
    );
    assert!(
        adopt_vault(&f.storage, &f.vault, &f.storage, &f.output)
            .await
            .is_err()
    );
}

#[tokio::test]
async fn changed_source_and_a_new_destination_never_publish_over_original_data() {
    let f = Fixture::new().await;
    let change = f.source.join("config.json");
    let result = adopt(
        &f.storage,
        &f.vault,
        &f.source,
        &f.output,
        Some(Box::new(move || {
            fs::write(change, b"changed after inventory").unwrap()
        })),
    )
    .await;
    assert!(result.is_err());
    assert!(!f.output.exists());
    let target = f.output.clone();
    let result = adopt(
        &f.storage,
        &f.vault,
        &f.source,
        &f.output,
        Some(Box::new(move || {
            fs::create_dir(&target).unwrap();
            fs::write(target.join("sentinel"), b"other owner").unwrap();
        })),
    )
    .await;
    assert!(result.is_err());
    assert_eq!(fs::read(f.output.join("sentinel")).unwrap(), b"other owner");
}

#[cfg(unix)]
#[tokio::test]
async fn source_links_and_same_bytes_directory_replacement_are_not_adopted() {
    use std::os::unix::fs::symlink;
    let f = Fixture::new().await;
    symlink(f.source.join("config.json"), f.source.join("alias")).unwrap();
    assert!(f.run().await.is_err());
    fs::remove_file(f.source.join("alias")).unwrap();
    fs::hard_link(f.source.join("config.json"), f.source.join("alias")).unwrap();
    assert!(f.run().await.is_err());
    fs::remove_file(f.source.join("alias")).unwrap();
    let source = f.source.clone();
    let moved = f._root.path().join("old-source");
    let result = adopt(
        &f.storage,
        &f.vault,
        &f.source,
        &f.output,
        Some(Box::new(move || {
            let entries = copy::inventory(&source).unwrap();
            fs::rename(&source, &moved).unwrap();
            copy::copy_inventory(&moved, &source, &entries, true).unwrap();
            copy::restore_directory_modes(&source, &entries).unwrap();
        })),
    )
    .await;
    assert!(result.is_err());
    assert!(!f.output.exists());
}

#[tokio::test]
async fn abandoned_caller_keeps_original_service_and_vault_owners_until_actual_copy_finishes() {
    let f = Fixture::new().await;
    let (entered, waiting) = tokio::sync::oneshot::channel();
    let (release, blocked) = std::sync::mpsc::channel();
    let (storage, vault, source, output) = (
        f.storage.clone(),
        f.vault.clone(),
        f.source.clone(),
        f.output.clone(),
    );
    let task = tokio::spawn(async move {
        adopt(
            &storage,
            &vault,
            &source,
            &output,
            Some(Box::new(move || {
                entered.send(()).unwrap();
                blocked.recv().unwrap();
            })),
        )
        .await
    });
    waiting.await.unwrap();
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    assert!(Catalog::open(&f.storage).await.is_err());
    assert!(matches!(
        VaultLease::try_acquire_existing(&f.source).unwrap(),
        LeaseAttempt::Busy
    ));
    release.send(()).unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            if f.output.exists() && Catalog::open(&f.storage).await.is_ok() {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert_eq!(
        canonical(&f.output.join("vaults").join(&f.vault)),
        canonical(&f.source)
    );
}
