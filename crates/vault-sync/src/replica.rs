use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};

use hypr_vault_read::transaction::VaultTransaction;
use serde::{Deserialize, Serialize};

use crate::snapshot::{Manifest, Snapshot, checked_path, copy_digest, validate_content};
use crate::{Error, Result};

const PENDING: &str = ".loofah-sync-pending";
const STAGING: &str = ".loofah-sync-stage";

#[derive(Clone)]
pub struct LocalReplica {
    vault: PathBuf,
    state: PathBuf,
    applying: std::sync::Arc<tokio::sync::Semaphore>,
}

struct PreparedStage {
    path: PathBuf,
    journaled: bool,
}

impl Drop for PreparedStage {
    fn drop(&mut self) {
        if !self.journaled {
            let _ = fs::remove_dir_all(&self.path);
        }
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Journal {
    version: u32,
    operation: String,
    vault: PathBuf,
    before: Manifest,
    after: Manifest,
    applied: bool,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Recovery {
    Clean,
    Applied(String),
    RolledBack(String),
}

impl LocalReplica {
    pub async fn wait_idle(&self) -> Result<()> {
        let _permit = self
            .applying
            .acquire()
            .await
            .map_err(|_| Error::RecoveryRequired)?;
        Ok(())
    }
    pub async fn apply_to_store(
        &self,
        store: &hypr_vault_write::SessionStore,
        expected: Manifest,
        incoming: Snapshot,
    ) -> Result<String> {
        if store.vault_base().canonicalize()? != self.vault {
            return Err(Error::Invalid("store belongs to another vault".into()));
        }
        store.flush_all().await?;
        let permit = self
            .applying
            .clone()
            .acquire_owned()
            .await
            .map_err(|_| Error::RecoveryRequired)?;
        let entity = expected.entity.clone();
        let replica = self.clone();
        let store = store.clone();
        // Cancelling a caller cannot cancel blocking installation; its index refresh
        // must therefore finish independently of the caller too.
        tokio::spawn(async move {
            let _permit = permit;
            let operation =
                tokio::task::spawn_blocking(move || replica.apply(&expected, &incoming))
                    .await
                    .map_err(|_| Error::RecoveryRequired)??;
            match entity {
                crate::scope::Entity::Session(id) => store.refresh_session(&id).await?,
                _ => {
                    store.rebuild_index().await?;
                }
            }
            Ok(operation)
        })
        .await
        .map_err(|_| Error::RecoveryRequired)?
    }

    pub fn open(vault: &Path, state: &Path) -> Result<Self> {
        let vault = vault.canonicalize()?;
        fs::create_dir_all(state)?;
        let state = state.canonicalize()?;
        if state.starts_with(&vault) || vault.starts_with(&state) {
            return Err(Error::Invalid(
                "sync state must be outside the vault".into(),
            ));
        }
        Ok(Self {
            vault,
            state,
            applying: std::sync::Arc::new(tokio::sync::Semaphore::new(1)),
        })
    }

    /// The caller advances its remote cursor only after this returns successfully.
    /// Previous content remains in same-volume staging for explicit reconciliation.
    pub fn apply(&self, expected: &Manifest, incoming: &Snapshot) -> Result<String> {
        self.apply_with(expected, incoming, |_| Ok(()))
    }

    fn apply_with(
        &self,
        expected: &Manifest,
        incoming: &Snapshot,
        mut checkpoint: impl FnMut(usize) -> Result<()>,
    ) -> Result<String> {
        expected.validate()?;
        incoming.manifest().validate()?;
        if expected.entity != incoming.manifest().entity {
            return Err(Error::Invalid("apply identity mismatch".into()));
        }
        let before = Snapshot::capture_expected(&self.vault, expected, &self.state)?;
        if before.manifest() != expected {
            return Err(Error::Conflict);
        }
        let changed = incoming.manifest().changed_paths(expected)?;
        validate_content(incoming.directory(), incoming.manifest())?;

        let operation = uuid::Uuid::new_v4().to_string();
        let stage = checked_path(&self.vault, &Path::new(STAGING).join(&operation))?;
        fs::create_dir_all(stage.parent().unwrap())?;
        fs::create_dir(&stage)?;
        let mut prepared = PreparedStage {
            path: stage.clone(),
            journaled: false,
        };
        let incoming_dir = stage.join("incoming");
        let total = changed.iter().try_fold(0u64, |sum, name| {
            sum.checked_add(
                incoming
                    .manifest()
                    .files
                    .get(name)
                    .map_or(0, |file| file.bytes),
            )
            .ok_or(Error::DiskFull)
        })?;
        if fs2::available_space(&self.vault)? < total.saturating_add(16 * 1024 * 1024) {
            return Err(Error::DiskFull);
        }
        fs::create_dir_all(&incoming_dir)?;
        for name in &changed {
            let Some(expected_digest) = incoming.manifest().files.get(name) else {
                continue;
            };
            let source = checked_path(incoming.directory(), Path::new(name))?;
            let destination = incoming_dir.join(name);
            fs::create_dir_all(destination.parent().unwrap())?;
            let mut output = File::create_new(&destination)?;
            let actual = copy_digest(&mut File::open(source)?, &mut output)?;
            output.sync_all()?;
            if &actual != expected_digest {
                return Err(Error::Invalid("staged content digest mismatch".into()));
            }
        }
        sync_tree_directories(&incoming_dir)?;
        sync_dir(&stage)?;
        sync_dir(stage.parent().unwrap())?;
        let mut journal = Journal {
            version: 1,
            operation: operation.clone(),
            vault: self.vault.clone(),
            before: expected.clone(),
            after: incoming.manifest().clone(),
            applied: false,
        };
        let _transaction = VaultTransaction::exclusive(&self.vault)?;
        VaultTransaction::ensure_ready(&self.vault)?;
        if !before.still_current(&self.vault)? {
            return Err(Error::Conflict);
        }
        self.write_journal(&journal)?;
        let marker = serde_json::to_vec(&(self.state.clone(), operation.clone()))
            .map_err(|e| Error::Invalid(e.to_string()))?;
        let mut pending = tempfile::NamedTempFile::new_in(&self.vault)?;
        pending.write_all(&marker)?;
        pending.as_file().sync_all()?;
        prepared.journaled = true;
        pending
            .persist_noclobber(self.vault.join(PENDING))
            .map_err(|e| e.error)?;
        sync_dir(&self.vault)?;
        checkpoint(0)?;
        let root = self.vault.join(expected.entity.root()?);
        for (index, name) in changed.iter().enumerate() {
            let destination = checked_path(&root, Path::new(name))?;
            if expected.files.contains_key(name) {
                let saved = stage.join("previous").join(name);
                create_dirs_durable(saved.parent().unwrap(), &stage)?;
                hypr_storage::fs::rename_no_replace(&destination, &saved)?;
                crate::snapshot::open_capture(&saved)?.sync_all()?;
                sync_dir(saved.parent().unwrap())?;
                sync_dir(destination.parent().unwrap())?;
                if !before.matches_saved(name, &saved)? {
                    return Err(Error::Conflict);
                }
            }
            checkpoint(index * 2 + 1)?;
            if incoming.manifest().files.contains_key(name) {
                create_dirs_durable(destination.parent().unwrap(), &self.vault)?;
                hypr_storage::fs::rename_no_replace(&incoming_dir.join(name), &destination)?;
                sync_dir(destination.parent().unwrap())?;
                sync_dir(incoming_dir.join(name).parent().unwrap())?;
            }
            checkpoint(index * 2 + 2)?;
        }
        journal.applied = true;
        self.write_journal(&journal)?;
        checkpoint(usize::MAX)?;
        fs::remove_file(self.vault.join(PENDING))?;
        sync_dir(&self.vault)?;
        Ok(operation)
    }

    /// An interrupted apply rolls back. Bytes written after the crash are moved to
    /// `competing/`, never overwritten. A corrupt/missing journal blocks normal use.
    pub fn recover(&self) -> Result<Recovery> {
        let _transaction = VaultTransaction::exclusive(&self.vault)?;
        let marker = match fs::read(self.vault.join(PENDING)) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Recovery::Clean);
            }
            Err(error) => return Err(error.into()),
        };
        let (state, operation): (PathBuf, String) =
            serde_json::from_slice(&marker).map_err(|_| Error::RecoveryRequired)?;
        if state != self.state || uuid::Uuid::parse_str(&operation).is_err() {
            return Err(Error::RecoveryRequired);
        }
        let journal: Journal = serde_json::from_slice(&fs::read(self.journal_path(&operation))?)
            .map_err(|_| Error::RecoveryRequired)?;
        journal.before.validate()?;
        journal.after.validate()?;
        if journal.version != 1
            || journal.vault != self.vault
            || journal.operation != operation
            || journal.before.entity != journal.after.entity
        {
            return Err(Error::RecoveryRequired);
        }
        if !journal.applied {
            self.rollback(&journal)?;
        }
        fs::remove_file(self.vault.join(PENDING))?;
        sync_dir(&self.vault)?;
        Ok(if journal.applied {
            Recovery::Applied(operation)
        } else {
            Recovery::RolledBack(operation)
        })
    }

    fn rollback(&self, journal: &Journal) -> Result<()> {
        let stage = checked_path(&self.vault, &Path::new(STAGING).join(&journal.operation))?;
        let root = checked_path(&self.vault, &journal.before.entity.root()?)?;
        let paths: std::collections::BTreeSet<_> = journal
            .before
            .files
            .keys()
            .chain(journal.after.files.keys())
            .collect();
        for name in paths {
            let previous = checked_path(&stage, &Path::new("previous").join(name))?;
            let incoming = checked_path(&stage, &Path::new("incoming").join(name))?;
            let destination = checked_path(&root, Path::new(name))?;
            let rolled_back = stage.join("restored").join(name);
            if rolled_back.exists() {
                continue;
            }
            let saved = previous.try_exists()?;
            let new_installed =
                !journal.before.files.contains_key(name) && !incoming.try_exists()?;
            if saved || new_installed {
                if destination.try_exists()? {
                    let competing = stage
                        .join("competing")
                        .join(uuid::Uuid::new_v4().to_string())
                        .join(name);
                    create_dirs_durable(competing.parent().unwrap(), &stage)?;
                    hypr_storage::fs::rename_no_replace(&destination, &competing)?;
                    sync_dir(competing.parent().unwrap())?;
                    sync_dir(destination.parent().unwrap())?;
                }
                if saved {
                    // Never hard-link the backup into editable content: an external
                    // in-place edit would otherwise change the preserved bytes too.
                    create_dirs_durable(destination.parent().unwrap(), &self.vault)?;
                    let temp = tempfile::NamedTempFile::new_in(destination.parent().unwrap())?;
                    fs::copy(&previous, temp.path())?;
                    temp.as_file().sync_all()?;
                    temp.persist_noclobber(&destination).map_err(|e| e.error)?;
                    sync_dir(destination.parent().unwrap())?;
                }
                create_dirs_durable(rolled_back.parent().unwrap(), &stage)?;
                File::create_new(&rolled_back)?.sync_all()?;
                sync_dir(rolled_back.parent().unwrap())?;
            }
        }
        Ok(())
    }

    fn journal_path(&self, operation: &str) -> PathBuf {
        self.state.join(format!("apply-{operation}.json"))
    }

    fn write_journal(&self, journal: &Journal) -> Result<()> {
        let bytes = serde_json::to_vec(journal).map_err(|e| Error::Invalid(e.to_string()))?;
        let mut temp = tempfile::NamedTempFile::new_in(&self.state)?;
        temp.write_all(&bytes)?;
        temp.as_file().sync_all()?;
        temp.persist(self.journal_path(&journal.operation))
            .map_err(|e| e.error)?;
        sync_dir(&self.state)?;
        Ok(())
    }
}

fn sync_dir(path: &Path) -> Result<()> {
    File::open(path)?.sync_all()?;
    Ok(())
}

fn create_dirs_durable(path: &Path, root: &Path) -> Result<()> {
    if !path.starts_with(root) {
        return Err(Error::Invalid("directory outside staging".into()));
    }
    fs::create_dir_all(path)?;
    for ancestor in path.ancestors() {
        sync_dir(ancestor)?;
        if ancestor == root {
            break;
        }
    }
    Ok(())
}

fn sync_tree_directories(path: &Path) -> Result<()> {
    for entry in fs::read_dir(path)? {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            sync_tree_directories(&entry.path())?;
        }
    }
    sync_dir(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scope::Entity;

    fn fixture(vault: &Path, note: &[u8]) {
        let dir = vault.join("sessions/one");
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("_meta.json"),
            br#"{"id":"one","title":"test","created_at":"2026-09-16","tags":[]}"#,
        )
        .unwrap();
        fs::write(dir.join("notes.md"), note).unwrap();
        fs::write(dir.join("user.pdf"), b"leave alone").unwrap();
        fs::write(vault.join("people.json"), b"{\"future\":true}").unwrap();
    }

    #[test]
    fn apply_preserves_previous_and_excluded_files_and_checks_baseline() {
        let first = tempfile::tempdir().unwrap();
        let second = tempfile::tempdir().unwrap();
        let state = tempfile::tempdir().unwrap();
        fixture(first.path(), b"before");
        fixture(second.path(), b"after");
        let entity = Entity::Session("one".into());
        let before = Snapshot::capture(first.path(), entity.clone(), state.path()).unwrap();
        let incoming = Snapshot::capture(second.path(), entity, state.path()).unwrap();
        let replica = LocalReplica::open(first.path(), state.path()).unwrap();
        let operation = replica.apply(before.manifest(), &incoming).unwrap();
        assert_eq!(
            fs::read(first.path().join("sessions/one/notes.md")).unwrap(),
            b"after"
        );
        assert_eq!(
            fs::read(
                first
                    .path()
                    .join(STAGING)
                    .join(operation)
                    .join("previous/notes.md")
            )
            .unwrap(),
            b"before"
        );
        assert_eq!(
            fs::read(first.path().join("sessions/one/user.pdf")).unwrap(),
            b"leave alone"
        );
        assert_eq!(
            fs::read(first.path().join("people.json")).unwrap(),
            b"{\"future\":true}"
        );
        assert!(matches!(
            replica.apply(before.manifest(), &incoming),
            Err(Error::Conflict)
        ));
    }

    #[test]
    fn every_apply_crash_boundary_recovers_idempotently() {
        for fail_at in [0, 1, 2, 3, 4, 5, 6, usize::MAX] {
            let first = tempfile::tempdir().unwrap();
            let second = tempfile::tempdir().unwrap();
            let state = tempfile::tempdir().unwrap();
            fixture(first.path(), b"before");
            fixture(second.path(), b"after");
            fs::write(
                first.path().join("sessions/one/audio.mp3"),
                b"removed recording",
            )
            .unwrap();
            fs::create_dir(second.path().join("sessions/one/attachments")).unwrap();
            fs::write(
                second.path().join("sessions/one/attachments/new.txt"),
                b"new attachment",
            )
            .unwrap();
            let entity = Entity::Session("one".into());
            let before = Snapshot::capture(first.path(), entity.clone(), state.path()).unwrap();
            let incoming = Snapshot::capture(second.path(), entity, state.path()).unwrap();
            let replica = LocalReplica::open(first.path(), state.path()).unwrap();
            assert!(
                replica
                    .apply_with(before.manifest(), &incoming, |step| {
                        if step == fail_at {
                            Err(Error::RecoveryRequired)
                        } else {
                            Ok(())
                        }
                    })
                    .is_err()
            );
            assert!(VaultTransaction::ensure_ready(first.path()).is_err());
            assert!(!matches!(replica.recover().unwrap(), Recovery::Clean));
            assert_eq!(replica.recover().unwrap(), Recovery::Clean);
            let expected: &[u8] = if fail_at == usize::MAX {
                b"after"
            } else {
                b"before"
            };
            assert_eq!(
                fs::read(first.path().join("sessions/one/notes.md")).unwrap(),
                expected,
                "crash at {fail_at}"
            );
            let current =
                Snapshot::capture(first.path(), Entity::Session("one".into()), state.path())
                    .unwrap();
            assert_eq!(
                current.manifest(),
                if fail_at == usize::MAX {
                    incoming.manifest()
                } else {
                    before.manifest()
                }
            );
        }
    }

    #[test]
    fn corrupt_or_missing_recovery_metadata_never_allows_normal_operation() {
        let vault = tempfile::tempdir().unwrap();
        let state = tempfile::tempdir().unwrap();
        fixture(vault.path(), b"before");
        let replica = LocalReplica::open(vault.path(), state.path()).unwrap();
        fs::write(vault.path().join(PENDING), b"broken").unwrap();
        assert!(matches!(replica.recover(), Err(Error::RecoveryRequired)));
        assert!(VaultTransaction::ensure_ready(vault.path()).is_err());
    }

    #[test]
    fn recovery_preserves_an_external_edit_made_after_the_crash() {
        let first = tempfile::tempdir().unwrap();
        let second = tempfile::tempdir().unwrap();
        let state = tempfile::tempdir().unwrap();
        fixture(first.path(), b"before");
        fixture(second.path(), b"after");
        let entity = Entity::Session("one".into());
        let baseline = Snapshot::capture(first.path(), entity.clone(), state.path()).unwrap();
        let incoming = Snapshot::capture(second.path(), entity, state.path()).unwrap();
        let replica = LocalReplica::open(first.path(), state.path()).unwrap();
        assert!(
            replica
                .apply_with(baseline.manifest(), &incoming, |step| {
                    if step == 2 {
                        Err(Error::RecoveryRequired)
                    } else {
                        Ok(())
                    }
                })
                .is_err()
        );
        fs::write(
            first.path().join("sessions/one/notes.md"),
            b"external after crash",
        )
        .unwrap();
        let Recovery::RolledBack(operation) = replica.recover().unwrap() else {
            panic!("expected rollback")
        };
        assert_eq!(
            fs::read(first.path().join("sessions/one/notes.md")).unwrap(),
            b"before"
        );
        let competing = first.path().join(STAGING).join(operation).join("competing");
        assert!(fs::read_dir(competing).unwrap().any(|entry| {
            fs::read(entry.unwrap().path().join("notes.md"))
                .is_ok_and(|bytes| bytes == b"external after crash")
        }));
    }
}
