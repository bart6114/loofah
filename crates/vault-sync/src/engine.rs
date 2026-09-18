use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::crypto::{EncryptedManifest, VaultKey};
use crate::remote::{Operation, RemoteClient, Revision};
use crate::replica::LocalReplica;
use crate::scheduler::Checkpoints;
use crate::scope::Entity;
use crate::snapshot::{FileDigest, Manifest, Snapshot, Stamp, inventory};
use crate::spool::UploadSpool;
use crate::{Error, Result};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Binding {
    pub account: String,
    pub vault: Uuid,
    pub generation: Uuid,
    pub local: PathBuf,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Baseline {
    pub revision: Uuid,
    pub content: EncryptedManifest,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Conflict {
    pub entity: Entity,
    pub local: Uuid,
    pub cloud: Uuid,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PendingApply {
    expected: Manifest,
    identity: String,
    revision: Uuid,
}

#[derive(Clone, Serialize, Deserialize)]
struct InitialSnapshot {
    id: Uuid,
    watermark: u64,
    generation: Uuid,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct State {
    version: u32,
    binding: Binding,
    initialized: bool,
    #[serde(default)]
    snapshot: Option<InitialSnapshot>,
    cursor: u64,
    baselines: BTreeMap<String, Baseline>,
    pending: Option<Revision>,
    #[serde(default)]
    deferred: Vec<Revision>,
    apply: Option<PendingApply>,
    conflicts: BTreeMap<String, Conflict>,
    last_success: Option<u64>,
}

pub struct Preview {
    pub title: Option<String>,
    pub device_id: Uuid,
    pub created_at: u64,
    pub manifest: Manifest,
    pub text: String,
    pub changed: Vec<String>,
    pub captured_at: Option<u64>,
    pub missing_references: Vec<String>,
}

pub struct Engine {
    directory: PathBuf,
    _lock: File,
    state: State,
    key: VaultKey,
    remote: RemoteClient,
    replica: LocalReplica,
    checkpoints: Checkpoints,
    observed: BTreeMap<Entity, BTreeMap<String, Stamp>>,
    reconciled: Option<Instant>,
    last_poll_succeeded: bool,
}

#[derive(Deserialize)]
struct StoredRevision {
    revision: StoredHead,
    objects: Vec<StoredObject>,
}

#[derive(Deserialize)]
struct StoredHead {
    device_id: Uuid,
    created_at: u64,
    entity: String,
    #[serde(rename = "manifest_object")]
    manifest: Uuid,
    operation: Operation,
    pinned: u8,
}

#[derive(Deserialize)]
struct StoredObject {
    id: Uuid,
    bytes: u64,
    digest: String,
}

#[derive(Deserialize)]
struct Change {
    sequence: u64,
    entity: String,
    revision: Uuid,
    operation: Operation,
}

impl Engine {
    pub async fn wait_idle(&self) -> Result<()> {
        self.replica.wait_idle().await
    }
    pub fn open(
        directory: &Path,
        mut binding: Binding,
        key: VaultKey,
        remote: RemoteClient,
    ) -> Result<Self> {
        binding.local = binding.local.canonicalize()?;
        if key.vault() != binding.vault {
            return Err(Error::Authentication);
        }
        fs::create_dir_all(directory)?;
        let directory = directory.canonicalize()?;
        if directory.starts_with(&binding.local) || binding.local.starts_with(&directory) {
            return Err(Error::Invalid(
                "sync state must be outside the vault".into(),
            ));
        }
        let lock = fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(directory.join("engine.lock"))?;
        lock.try_lock()
            .map_err(|_| Error::Invalid("another sync engine is active".into()))?;
        let state = match fs::read(directory.join("engine.json")) {
            Ok(bytes) => {
                let state: State =
                    serde_json::from_slice(&bytes).map_err(|_| Error::RecoveryRequired)?;
                if state.version != 1 || state.binding != binding {
                    return Err(Error::RecoveryRequired);
                }
                for (identity, baseline) in &state.baselines {
                    baseline.content.validate()?;
                    if baseline.content.vault != key.vault()
                        || key.entity_identity(&baseline.content.manifest.entity)? != *identity
                    {
                        return Err(Error::Authentication);
                    }
                }
                state
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => State {
                version: 1,
                binding,
                initialized: false,
                snapshot: None,
                cursor: 0,
                baselines: BTreeMap::new(),
                pending: None,
                deferred: vec![],
                apply: None,
                conflicts: BTreeMap::new(),
                last_success: None,
            },
            Err(error) => return Err(error.into()),
        };
        let replica = LocalReplica::open(&state.binding.local, &directory)?;
        replica.recover()?;
        let engine = Self {
            directory,
            _lock: lock,
            state,
            key,
            remote,
            replica,
            checkpoints: Checkpoints::default(),
            observed: BTreeMap::new(),
            reconciled: None,
            last_poll_succeeded: false,
        };
        engine.save()?;
        Ok(engine)
    }

    pub fn cursor(&self) -> u64 {
        self.state.cursor
    }

    pub fn conflicts(&self) -> Vec<Conflict> {
        self.state.conflicts.values().cloned().collect()
    }
    pub fn last_success(&self) -> Option<u64> {
        self.state.last_success
    }
    pub fn changed(&mut self, entity: Entity) {
        self.checkpoints.changed(entity, Instant::now(), false);
    }
    pub fn reconcile(&mut self) {
        self.reconciled = None;
    }

    fn save(&self) -> Result<()> {
        atomic_json(&self.directory.join("engine.json"), &self.state)
    }

    fn capture(&self, entity: Entity) -> Result<Snapshot> {
        blocking(|| Snapshot::capture(&self.state.binding.local, entity, &self.directory))
    }

    fn capture_current(&self, entity: Entity) -> Result<Snapshot> {
        match self.capture(entity.clone()) {
            Ok(snapshot) => Ok(snapshot),
            Err(error) => {
                let identity = self.key.entity_identity(&entity)?;
                let deleted = self
                    .state
                    .baselines
                    .get(&identity)
                    .is_some_and(|baseline| baseline.content.manifest.deleted);
                let explicit = match &entity {
                    Entity::Session(id) => {
                        hypr_vault_write::sync_deletions::read(&self.state.binding.local, id)?
                            .is_some()
                    }
                    _ => false,
                };
                if !deleted && !explicit {
                    return Err(error);
                }
                let absent = Snapshot::tombstone(entity, &self.directory)?;
                blocking(|| {
                    Snapshot::capture_expected(
                        &self.state.binding.local,
                        absent.manifest(),
                        &self.directory,
                    )
                })
            }
        }
    }

    fn prepare(
        &mut self,
        snapshot: &Snapshot,
        previous: Option<&EncryptedManifest>,
        expected: Option<Uuid>,
        operation: Operation,
        conflicts: Vec<Uuid>,
        apply_expected: Option<Manifest>,
    ) -> Result<UploadSpool> {
        if self.state.pending.is_some() || self.state.apply.is_some() {
            return Err(Error::RecoveryRequired);
        }
        let spool = blocking(|| {
            UploadSpool::persist(
                self.key
                    .prepare_version(snapshot, previous, &self.directory)?,
                &self.directory.join("uploads"),
            )
        })?;
        self.state.apply = apply_expected.map(|expected| PendingApply {
            expected,
            identity: self
                .key
                .entity_identity(&snapshot.manifest().entity)
                .expect("validated entity"),
            revision: spool.revision(),
        });
        self.state.pending = Some(Revision {
            id: spool.revision(),
            entity: self.key.entity_identity(&snapshot.manifest().entity)?,
            expected,
            manifest: spool.revision(),
            operation,
            objects: spool.manifest().membership().into_keys().collect(),
            conflicts,
        });
        self.save()?;
        Ok(spool)
    }

    async fn finish_pending(&mut self) -> Result<()> {
        let Some(mut revision) = self.state.pending.clone() else {
            return Ok(());
        };
        if matches!(revision.operation, Operation::Restore | Operation::Resolve)
            && self.state.apply.is_none()
        {
            return Err(Error::RecoveryRequired);
        }
        let spool = blocking(|| {
            UploadSpool::reopen(&self.directory.join("uploads"), revision.id, &self.key)
        })?;
        let control = matches!(
            revision.operation,
            Operation::Delete | Operation::Restore | Operation::Resolve
        );
        self.remote.upload(&spool, control).await?;
        match self.remote.commit(&revision).await {
            Ok(()) => {}
            Err(Error::Cloud { status: 409, code }) if code == "conflict" && !control => {
                revision.operation = Operation::Conflict;
                self.state.pending = Some(revision.clone());
                self.save()?;
                self.remote.commit(&revision).await?;
            }
            Err(Error::Cloud { status: 409, code }) if code == "conflict" && control => {
                self.state.pending = None;
                self.state.apply = None;
                self.save()?;
                return Err(Error::Conflict);
            }
            Err(error) => return Err(error),
        }
        if revision.operation == Operation::Conflict {
            if let Some(cloud) = revision.expected {
                self.state.conflicts.insert(
                    revision.entity.clone(),
                    Conflict {
                        entity: spool.manifest().manifest.entity.clone(),
                        local: revision.id,
                        cloud,
                    },
                );
            }
        } else if self.state.apply.is_none() {
            self.state.baselines.insert(
                revision.entity.clone(),
                Baseline {
                    revision: revision.id,
                    content: spool.manifest().clone(),
                },
            );
            if revision.operation == Operation::Resolve {
                self.state.conflicts.remove(&revision.entity);
            }
        }
        self.state.pending = None;
        self.save()?;
        // Keep the spool as a local checkpoint until a separately validated retention policy exists.
        Ok(())
    }

    async fn finish_or_defer(&mut self) -> Result<()> {
        match self.finish_pending().await {
            Err(Error::Cloud { status, code }) if code == "quota" && self.state.apply.is_none() => {
                if let Some(revision) = self.state.pending.take() {
                    self.state.deferred.push(revision);
                    self.save()?;
                }
                let _ = status;
                Ok(())
            }
            result => result,
        }
    }

    pub fn up_to_date(&self) -> bool {
        self.last_poll_succeeded
            && self.state.initialized
            && self.state.snapshot.is_none()
            && self.reconciled.is_some()
            && self.pending_work() == 0
            && self.state.conflicts.is_empty()
    }

    pub fn pending_work(&self) -> usize {
        self.checkpoints.len()
            + self.state.deferred.len()
            + usize::from(self.state.pending.is_some())
            + usize::from(self.state.apply.is_some())
    }

    async fn fetch_version(
        &self,
        revision: Uuid,
        identity: &str,
    ) -> Result<(EncryptedManifest, Snapshot)> {
        let stored: StoredRevision = self
            .remote
            .get(&format!("/sync/revisions/{revision}"))
            .await?;
        if stored.revision.entity != identity || stored.revision.manifest != revision {
            return Err(Error::Authentication);
        }
        let mut downloaded = BTreeMap::new();
        let mut membership = BTreeMap::new();
        for object in stored.objects {
            if downloaded.contains_key(&object.id) {
                return Err(Error::Authentication);
            }
            let digest = FileDigest {
                bytes: object.bytes,
                sha256: object.digest,
            };
            let file = self
                .remote
                .download(object.id, &digest, &self.directory)
                .await?;
            if object.id != revision {
                membership.insert(object.id, digest);
            }
            downloaded.insert(object.id, file);
        }
        blocking(|| {
            let ciphertext = fs::read(downloaded.get(&revision).ok_or(Error::Authentication)?)?;
            let encrypted = self.key.open_manifest(revision, &ciphertext)?;
            if self.key.entity_identity(&encrypted.manifest.entity)? != identity
                || (stored.revision.operation == Operation::Delete && !encrypted.manifest.deleted)
            {
                return Err(Error::Authentication);
            }
            let snapshot = self.key.restore_snapshot(
                revision,
                &ciphertext,
                &membership,
                &self.directory,
                |id| {
                    Ok(File::open(
                        downloaded.get(&id).ok_or(Error::Authentication)?,
                    )?)
                },
            )?;
            Ok((encrypted, snapshot))
        })
    }

    async fn receive(
        &mut self,
        store: &hypr_vault_write::SessionStore,
        identity: &str,
        revision: Uuid,
        operation: Operation,
    ) -> Result<()> {
        if operation == Operation::Conflict {
            let stored: StoredRevision = self
                .remote
                .get(&format!("/sync/revisions/{revision}"))
                .await?;
            if stored.revision.entity != identity {
                return Err(Error::Authentication);
            }
            if stored.revision.pinned != 1 || self.state.conflicts.contains_key(identity) {
                return Ok(());
            }
            #[derive(Deserialize)]
            struct Head {
                revision: Option<Uuid>,
            }
            let head: Head = self.remote.get(&format!("/sync/heads/{identity}")).await?;
            if let Some(cloud) = head.revision {
                let (content, _) = self.fetch_version(revision, identity).await?;
                self.state.conflicts.insert(
                    identity.into(),
                    Conflict {
                        entity: content.manifest.entity,
                        local: revision,
                        cloud,
                    },
                );
                self.save()?;
            }
            return Ok(());
        }
        if let Some(conflict) = self.state.conflicts.get(identity) {
            let stored: StoredRevision = self
                .remote
                .get(&format!("/sync/revisions/{}", conflict.local))
                .await?;
            if stored.revision.pinned == 0 {
                self.state.conflicts.remove(identity);
                self.save()?;
            }
        }
        if self
            .state
            .baselines
            .get(identity)
            .is_some_and(|baseline| baseline.revision == revision)
        {
            return Ok(());
        }
        let (encrypted, incoming) = self.fetch_version(revision, identity).await?;
        let entity = incoming.manifest().entity.clone();
        let local = self.capture_current(entity.clone());
        let baseline = self.state.baselines.get(identity).cloned();
        let expected = match local {
            Ok(local) if local.manifest() == incoming.manifest() => {
                self.state.baselines.insert(
                    identity.into(),
                    Baseline {
                        revision,
                        content: encrypted,
                    },
                );
                return self.save();
            }
            Ok(local)
                if baseline
                    .as_ref()
                    .is_some_and(|baseline| &baseline.content.manifest == local.manifest()) =>
            {
                local.manifest().clone()
            }
            Ok(local) => {
                if self
                    .state
                    .deferred
                    .iter()
                    .any(|pending| pending.entity == identity)
                {
                    return Err(Error::Cloud {
                        status: 507,
                        code: "quota".into(),
                    });
                }
                if !self.state.conflicts.contains_key(identity) {
                    self.prepare(
                        &local,
                        baseline.as_ref().map(|value| &value.content),
                        Some(revision),
                        Operation::Conflict,
                        vec![],
                        None,
                    )?;
                    self.finish_pending().await?;
                }
                if let Some(conflict) = self.state.conflicts.get_mut(identity) {
                    conflict.cloud = revision;
                }
                self.save()?;
                return Ok(());
            }
            Err(Error::Recording) => return Err(Error::Recording),
            Err(_) if baseline.is_none() => {
                let absent = Snapshot::tombstone(entity.clone(), &self.directory)?;
                blocking(|| {
                    Snapshot::capture_expected(
                        &self.state.binding.local,
                        absent.manifest(),
                        &self.directory,
                    )
                })?;
                absent.manifest().clone()
            }
            Err(error) => return Err(error),
        };
        self.replica
            .apply_to_store(store, expected, incoming)
            .await?;
        self.state.baselines.insert(
            identity.into(),
            Baseline {
                revision,
                content: encrypted,
            },
        );
        self.save()
    }

    async fn initialize(&mut self, store: &hypr_vault_write::SessionStore) -> Result<()> {
        #[derive(Deserialize)]
        struct Item {
            entity: String,
            revision: Uuid,
            operation: Operation,
        }
        #[derive(Deserialize)]
        struct Page {
            items: Vec<Item>,
            next: Option<String>,
        }
        let snapshot = match &self.state.snapshot {
            Some(snapshot) => snapshot.clone(),
            None => {
                let snapshot: InitialSnapshot = self
                    .remote
                    .post("/sync/snapshots", &serde_json::json!({}))
                    .await?;
                self.state.snapshot = Some(snapshot.clone());
                self.save()?;
                snapshot
            }
        };
        let minimum: u64 = match fs::read(self.directory.join("paired-checkpoint.json")) {
            Ok(bytes) => serde_json::from_slice(&bytes).map_err(|_| Error::RecoveryRequired)?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => 0,
            Err(error) => return Err(error.into()),
        };
        if snapshot.generation != self.state.binding.generation || snapshot.watermark < minimum {
            return Err(Error::RecoveryRequired);
        }
        let mut after = String::new();
        loop {
            let page: Page = match self
                .remote
                .get(&format!("/sync/snapshots/{}?after={after}", snapshot.id))
                .await
            {
                Ok(page) => page,
                Err(error @ Error::Cloud { status: 503, .. }) => {
                    self.state.snapshot = None;
                    self.save()?;
                    return Err(error);
                }
                Err(error) => return Err(error),
            };
            for item in page.items {
                self.receive(store, &item.entity, item.revision, item.operation)
                    .await?;
            }
            match page.next {
                Some(next)
                    if next > after
                        && next
                            .bytes()
                            .all(|c| c.is_ascii_hexdigit() || c == b':' || c == b'-') =>
                {
                    after = next
                }
                None => break,
                _ => return Err(Error::Authentication),
            }
        }
        self.state.cursor = snapshot.watermark;
        self.state.initialized = true;
        self.state.snapshot = None;
        self.save()?;
        let _ = self
            .remote
            .delete::<serde_json::Value>(&format!("/sync/snapshots/{}", snapshot.id))
            .await;
        Ok(())
    }

    pub async fn tick(&mut self, store: &hypr_vault_write::SessionStore) -> Result<()> {
        self.last_poll_succeeded = false;
        if store.vault_base().canonicalize()? != self.state.binding.local {
            return Err(Error::RecoveryRequired);
        }
        if self.state.pending.is_none() && !self.state.deferred.is_empty() {
            self.state.pending = Some(self.state.deferred.remove(0));
            self.save()?;
        }
        self.finish_or_defer().await?;
        self.finish_apply(store).await?;
        if !self.state.initialized {
            self.initialize(store).await?;
        }
        #[derive(Deserialize)]
        struct Changes {
            changes: Vec<Change>,
        }
        let changes: Changes = self
            .remote
            .get(&format!("/sync/changes?after={}", self.state.cursor))
            .await?;
        for change in changes.changes {
            if change.sequence <= self.state.cursor {
                return Err(Error::Authentication);
            }
            self.receive(store, &change.entity, change.revision, change.operation)
                .await?;
            self.state.cursor = change.sequence;
            self.save()?;
        }
        let now = Instant::now();
        if self
            .reconciled
            .is_none_or(|last| now.duration_since(last).as_secs() >= 300)
        {
            let mut entities = blocking(|| local_entities(&self.state.binding.local))?;
            entities.extend(
                self.state
                    .baselines
                    .values()
                    .map(|value| value.content.manifest.entity.clone()),
            );
            for entity in entities {
                let current = blocking(|| inventory(&self.state.binding.local, &entity));
                if let Ok(current) = current
                    && self.observed.get(&entity) != Some(&current)
                {
                    self.observed.insert(entity.clone(), current);
                    self.changed(entity);
                }
            }
            self.reconciled = Some(now);
        }
        for (entity, generation) in self.checkpoints.due(now) {
            let identity = self.key.entity_identity(&entity)?;
            if self.state.conflicts.contains_key(&identity) {
                continue;
            }
            let captured = match self.capture_current(entity.clone()) {
                Ok(value) => value,
                Err(Error::Recording) => continue,
                Err(error) => {
                    let Entity::Session(id) = &entity else {
                        return Err(error);
                    };
                    if hypr_vault_write::sync_deletions::read(&self.state.binding.local, id)?
                        .is_none()
                    {
                        return Err(error);
                    }
                    let absent = Snapshot::tombstone(entity.clone(), &self.directory)?;
                    blocking(|| {
                        Snapshot::capture_expected(
                            &self.state.binding.local,
                            absent.manifest(),
                            &self.directory,
                        )
                    })?;
                    absent
                }
            };
            if !captured.manifest().deleted
                && self
                    .state
                    .deferred
                    .iter()
                    .any(|revision| revision.entity == identity)
            {
                continue;
            }
            let baseline = self.state.baselines.get(&identity).cloned();
            if baseline
                .as_ref()
                .is_none_or(|baseline| baseline.content.manifest != *captured.manifest())
            {
                self.prepare(
                    &captured,
                    baseline.as_ref().map(|value| &value.content),
                    baseline.as_ref().map(|value| value.revision),
                    if captured.manifest().deleted {
                        Operation::Delete
                    } else if baseline.as_ref().is_some_and(|baseline| {
                        baseline
                            .content
                            .manifest
                            .files
                            .keys()
                            .any(|name| !captured.manifest().files.contains_key(name))
                    }) {
                        Operation::Conflict
                    } else {
                        Operation::Checkpoint
                    },
                    vec![],
                    None,
                )?;
                self.finish_or_defer().await?;
            }
            self.checkpoints
                .committed(&entity, generation, Instant::now());
        }
        if !self.state.deferred.is_empty() {
            return Err(Error::Cloud {
                status: 507,
                code: "quota".into(),
            });
        }
        self.state.last_success = Some(now_ms());
        self.save()?;
        self.last_poll_succeeded = true;
        Ok(())
    }

    async fn finish_apply(&mut self, store: &hypr_vault_write::SessionStore) -> Result<()> {
        let Some(apply) = self.state.apply.clone() else {
            return Ok(());
        };
        let (content, incoming) = self.fetch_version(apply.revision, &apply.identity).await?;
        let current = blocking(|| {
            Snapshot::capture_expected(
                &self.state.binding.local,
                incoming.manifest(),
                &self.directory,
            )
        })
        .or_else(|_| {
            blocking(|| {
                Snapshot::capture_expected(
                    &self.state.binding.local,
                    &apply.expected,
                    &self.directory,
                )
            })
        })?;
        if current.manifest() != incoming.manifest() {
            match self
                .replica
                .apply_to_store(store, apply.expected, incoming)
                .await
            {
                Err(Error::Conflict) => {
                    self.state.apply = None;
                    self.save()?;
                    return Err(Error::Conflict);
                }
                result => {
                    result?;
                }
            }
        } else {
            match &apply.expected.entity {
                Entity::Session(id) => store.refresh_session(id).await?,
                _ => {
                    store.rebuild_index().await?;
                }
            }
        }
        self.state.baselines.insert(
            apply.identity.clone(),
            Baseline {
                revision: apply.revision,
                content,
            },
        );
        self.state.conflicts.remove(&apply.identity);
        self.state.apply = None;
        self.save()
    }

    pub async fn history(&self, entity: &Entity) -> Result<serde_json::Value> {
        self.history_page(entity, None).await
    }

    pub async fn history_page(
        &self,
        entity: &Entity,
        before: Option<(u64, Uuid)>,
    ) -> Result<serde_json::Value> {
        let cursor = before
            .map(|(at, id)| format!("?before={at}&id={id}"))
            .unwrap_or_default();
        self.remote
            .get(&format!(
                "/sync/history/{}{cursor}",
                self.key.entity_identity(entity)?
            ))
            .await
    }

    pub async fn preview(&self, entity: &Entity, revision: Uuid) -> Result<Preview> {
        let identity = self.key.entity_identity(entity)?;
        let stored: StoredRevision = self
            .remote
            .get(&format!("/sync/revisions/{revision}"))
            .await?;
        if stored.revision.entity != identity || stored.revision.manifest != revision {
            return Err(Error::Authentication);
        }
        let object = stored
            .objects
            .iter()
            .find(|value| value.id == revision)
            .ok_or(Error::Authentication)?;
        let file = self
            .remote
            .download(
                revision,
                &FileDigest {
                    bytes: object.bytes,
                    sha256: object.digest.clone(),
                },
                &self.directory,
            )
            .await?;
        let encrypted = blocking(|| self.key.open_manifest(revision, &fs::read(file)?))?;
        if self.key.entity_identity(&encrypted.manifest.entity)? != identity {
            return Err(Error::Authentication);
        }
        let membership = stored
            .objects
            .iter()
            .filter(|object| object.id != revision)
            .map(|object| {
                (
                    object.id,
                    FileDigest {
                        bytes: object.bytes,
                        sha256: object.digest.clone(),
                    },
                )
            })
            .collect::<BTreeMap<_, _>>();
        if membership != encrypted.membership() || stored.objects.len() != membership.len() + 1 {
            return Err(Error::Authentication);
        }
        let name =
            entity
                .registry_name()
                .unwrap_or(if encrypted.objects.contains_key("notes.md") {
                    "notes.md"
                } else {
                    "_memo.md"
                });
        let text = match encrypted.objects.get(name) {
            Some(object) if object.plaintext.bytes > 1024 * 1024 => {
                "Preview is too large. Export this version to read it.".into()
            }
            Some(object) => {
                let bytes = self
                    .remote
                    .download(object.object, &object.ciphertext, &self.directory)
                    .await?;
                blocking(|| {
                    let plain =
                        self.key
                            .decrypt_file(&mut File::open(bytes)?, object, &self.directory)?;
                    Ok::<_, Error>(fs::read_to_string(plain.path())?)
                })?
            }
            None => String::new(),
        };
        let baseline = self
            .state
            .baselines
            .get(&identity)
            .map(|value| &value.content.manifest);
        let mut names = encrypted
            .manifest
            .files
            .keys()
            .cloned()
            .collect::<BTreeSet<_>>();
        if let Some(baseline) = baseline {
            names.extend(baseline.files.keys().cloned());
        }
        let changed = names
            .into_iter()
            .filter(|name| {
                baseline.and_then(|value| value.files.get(name))
                    != encrypted.manifest.files.get(name)
            })
            .collect();
        let mut title = None;
        let mut missing_references = BTreeSet::new();
        if matches!(entity, Entity::Session(_)) {
            let registries = blocking(|| -> Result<_> {
                let _guard = hypr_vault_read::transaction::VaultTransaction::shared(
                    &self.state.binding.local,
                )?;
                hypr_vault_read::transaction::VaultTransaction::ensure_ready(
                    &self.state.binding.local,
                )?;
                let people = hypr_vault_read::read_people(&self.state.binding.local)
                    .into_iter()
                    .map(|person| person.id)
                    .collect::<BTreeSet<_>>();
                let tags: serde_json::Value = fs::read(self.state.binding.local.join("tags.json"))
                    .ok()
                    .and_then(|bytes| serde_json::from_slice(&bytes).ok())
                    .unwrap_or_default();
                let tags = tags["tags"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|tag| tag["id"].as_str().map(str::to_owned))
                    .collect::<BTreeSet<_>>();
                Ok((people, tags))
            })?;
            for name in ["_meta.json", "transcript.json"] {
                let Some(object) = encrypted.objects.get(name) else {
                    continue;
                };
                if object.plaintext.bytes > 16 * 1024 * 1024 {
                    missing_references.insert(format!(
                        "Reference checking skipped for large {name}; export to inspect it."
                    ));
                    continue;
                }
                let bytes = self
                    .remote
                    .download(object.object, &object.ciphertext, &self.directory)
                    .await?;
                let value: serde_json::Value = blocking(|| {
                    let plain =
                        self.key
                            .decrypt_file(&mut File::open(bytes)?, object, &self.directory)?;
                    serde_json::from_reader(File::open(plain.path())?)
                        .map_err(|_| Error::Authentication)
                })?;
                if name == "_meta.json" {
                    title = value["title"].as_str().map(str::to_owned);
                    for tag in value["tags"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(|tag| tag.as_str())
                    {
                        if hypr_vault_read::normalize_tag_name(tag)
                            .is_some_and(|id| !registries.1.contains(&id))
                        {
                            missing_references.insert(format!("Tag: {tag}"));
                        }
                    }
                } else {
                    for transcript in value["transcripts"].as_array().into_iter().flatten() {
                        for hint in transcript["speaker_hints"].as_array().into_iter().flatten() {
                            if hint["type"] == "speaker_label"
                                && let Some(person) = hint["value"].as_str()
                                && !registries.0.contains(person)
                            {
                                missing_references
                                    .insert(format!("Person or legacy speaker label: {person}"));
                            }
                        }
                    }
                }
            }
        }
        Ok(Preview {
            title,
            device_id: stored.revision.device_id,
            created_at: stored.revision.created_at,
            manifest: encrypted.manifest,
            text,
            changed,
            captured_at: encrypted.captured_at,
            missing_references: missing_references.into_iter().collect(),
        })
    }

    pub async fn purge(&self, revisions: &[Uuid]) -> Result<()> {
        self.remote
            .post::<serde_json::Value>("/sync/history/purge", &revisions)
            .await?;
        Ok(())
    }

    pub async fn export(&self, entity: &Entity, revision: Uuid, destination: &Path) -> Result<()> {
        let (_, snapshot) = self
            .fetch_version(revision, &self.key.entity_identity(entity)?)
            .await?;
        blocking(|| {
            fs::create_dir(destination)?;
            for name in snapshot.manifest().files.keys() {
                let target = destination.join(name);
                fs::create_dir_all(target.parent().unwrap())?;
                let mut output = File::create_new(target)?;
                std::io::copy(
                    &mut File::open(snapshot.directory().join(name))?,
                    &mut output,
                )?;
                output.sync_all()?;
            }
            Ok(())
        })
    }

    pub async fn restore(
        &mut self,
        store: &hypr_vault_write::SessionStore,
        entity: Entity,
        selected: Uuid,
    ) -> Result<()> {
        self.finish_or_defer().await?;
        let identity = self.key.entity_identity(&entity)?;
        let baseline = self.state.baselines.get(&identity).cloned();
        let expected = self
            .state
            .conflicts
            .get(&identity)
            .map(|conflict| conflict.cloud)
            .or_else(|| baseline.as_ref().map(|value| value.revision))
            .ok_or(Error::RecoveryRequired)?;
        let local = self.capture_current(entity.clone())?;
        let previous_conflict = self.state.conflicts.get(&identity).map(|value| value.local);
        let already_preserved = if let Some(revision) = previous_conflict {
            self.fetch_version(revision, &identity).await?.1.manifest() == local.manifest()
        } else {
            false
        };
        if baseline
            .as_ref()
            .is_none_or(|baseline| *local.manifest() != baseline.content.manifest)
            && !already_preserved
        {
            self.prepare(
                &local,
                baseline.as_ref().map(|baseline| &baseline.content),
                Some(expected),
                Operation::Conflict,
                vec![],
                None,
            )?;
            self.finish_pending().await?;
        }
        let (content, incoming) = self.fetch_version(selected, &identity).await?;
        let mut conflicts = self
            .state
            .conflicts
            .get(&identity)
            .map(|value| vec![value.local])
            .unwrap_or_default();
        if let Some(previous) = previous_conflict
            && !conflicts.contains(&previous)
        {
            conflicts.push(previous);
        }
        let operation = if conflicts.is_empty() {
            Operation::Restore
        } else {
            Operation::Resolve
        };
        self.prepare(
            &incoming,
            Some(&content),
            Some(expected),
            operation,
            conflicts,
            Some(local.manifest().clone()),
        )?;
        self.finish_pending().await?;
        self.finish_apply(store).await
    }
}

pub fn atomic_json(path: &Path, value: &impl Serialize) -> Result<()> {
    let parent = path.parent().ok_or(Error::RecoveryRequired)?;
    fs::create_dir_all(parent)?;
    let mut file = tempfile::NamedTempFile::new_in(parent)?;
    serde_json::to_writer(&mut file, value).map_err(|_| Error::RecoveryRequired)?;
    file.flush()?;
    file.as_file().sync_all()?;
    file.persist(path).map_err(|error| error.error)?;
    File::open(parent)?.sync_all()?;
    Ok(())
}

pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn blocking<T>(work: impl FnOnce() -> T) -> T {
    tokio::task::block_in_place(work)
}

fn local_entities(vault: &Path) -> Result<BTreeSet<Entity>> {
    let mut entities = BTreeSet::new();
    for entity in [Entity::People, Entity::Tags, Entity::Tasks] {
        if vault.join(entity.registry_name().unwrap()).try_exists()? {
            entities.insert(entity);
        }
    }
    match fs::read_dir(vault.join("sessions")) {
        Ok(entries) => {
            for entry in entries {
                let entry = entry?;
                let name = entry.file_name().to_string_lossy().into_owned();
                if name.starts_with('.') || !entry.file_type()?.is_dir() {
                    continue;
                }
                let entity = Entity::Session(name);
                if entity.root().is_ok() && entry.path().join("_meta.json").is_file() {
                    entities.insert(entity);
                }
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    Ok(entities)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::remote::Environment;

    #[test]
    fn persistent_state_rejects_another_account_vault_generation_and_corruption() {
        let vault = tempfile::tempdir().unwrap();
        let state = tempfile::tempdir().unwrap();
        let id = Uuid::new_v4();
        let key = VaultKey::generate(id).unwrap();
        let kit = key.recovery_kit();
        let binding = Binding {
            account: "one".into(),
            vault: id,
            generation: Uuid::new_v4(),
            local: vault.path().canonicalize().unwrap(),
        };
        let remote =
            || RemoteClient::new(Environment::Staging, "test", Some(binding.generation)).unwrap();
        let engine = Engine::open(state.path(), binding.clone(), key, remote()).unwrap();
        assert!(engine.state.baselines.is_empty());
        assert!(!engine.state.initialized);
        assert!(
            Engine::open(
                state.path(),
                binding.clone(),
                VaultKey::from_recovery_kit(&kit, id).unwrap(),
                remote()
            )
            .is_err()
        );
        drop(engine);
        let original = fs::read(state.path().join("engine.json")).unwrap();
        for changed in [
            Binding {
                account: "two".into(),
                ..binding.clone()
            },
            Binding {
                generation: Uuid::new_v4(),
                ..binding.clone()
            },
        ] {
            assert!(matches!(
                Engine::open(
                    state.path(),
                    changed,
                    VaultKey::from_recovery_kit(&kit, id).unwrap(),
                    remote()
                ),
                Err(Error::RecoveryRequired)
            ));
            assert_eq!(
                fs::read(state.path().join("engine.json")).unwrap(),
                original
            );
        }
        fs::write(state.path().join("engine.json"), b"broken").unwrap();
        assert!(matches!(
            Engine::open(
                state.path(),
                binding.clone(),
                VaultKey::from_recovery_kit(&kit, id).unwrap(),
                remote()
            ),
            Err(Error::RecoveryRequired)
        ));
        assert_eq!(
            fs::read(state.path().join("engine.json")).unwrap(),
            b"broken"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn pending_ciphertext_and_expected_revision_survive_restart() {
        let vault = tempfile::tempdir().unwrap();
        let state = tempfile::tempdir().unwrap();
        fs::write(
            vault.path().join("people.json"),
            br#"{"people":[],"future":true}"#,
        )
        .unwrap();
        let id = Uuid::new_v4();
        let key = VaultKey::generate(id).unwrap();
        let kit = key.recovery_kit();
        let binding = Binding {
            account: "one".into(),
            vault: id,
            generation: Uuid::new_v4(),
            local: vault.path().canonicalize().unwrap(),
        };
        let remote =
            || RemoteClient::new(Environment::Staging, "test", Some(binding.generation)).unwrap();
        let mut engine = Engine::open(state.path(), binding.clone(), key, remote()).unwrap();
        let snapshot = engine.capture(Entity::People).unwrap();
        let expected = Some(Uuid::new_v4());
        let upload = engine
            .prepare(
                &snapshot,
                None,
                expected,
                Operation::Checkpoint,
                vec![],
                None,
            )
            .unwrap();
        let revision = upload.revision();
        let ciphertext = upload.encrypted_manifest().to_vec();
        let snapshot_id = Uuid::new_v4();
        engine.state.snapshot = Some(InitialSnapshot {
            id: snapshot_id,
            watermark: 17,
            generation: binding.generation,
        });
        engine.save().unwrap();
        drop(engine);
        let reopened = Engine::open(
            state.path(),
            binding.clone(),
            VaultKey::from_recovery_kit(&kit, id).unwrap(),
            remote(),
        )
        .unwrap();
        assert_eq!(reopened.state.snapshot.as_ref().unwrap().id, snapshot_id);
        assert_eq!(reopened.state.pending.as_ref().unwrap().id, revision);
        assert_eq!(reopened.state.pending.as_ref().unwrap().expected, expected);
        let spool =
            UploadSpool::reopen(&state.path().join("uploads"), revision, &reopened.key).unwrap();
        assert_eq!(spool.encrypted_manifest(), ciphertext);
        assert_eq!(
            fs::read(vault.path().join("people.json")).unwrap(),
            br#"{"people":[],"future":true}"#
        );
    }
    #[tokio::test(flavor = "multi_thread")]
    async fn up_to_date_requires_this_run_to_reconcile_and_all_work_to_finish() {
        let vault = tempfile::tempdir().unwrap();
        let state = tempfile::tempdir().unwrap();
        let id = Uuid::new_v4();
        let key = VaultKey::generate(id).unwrap();
        let kit = key.recovery_kit();
        let binding = Binding {
            account: "one".into(),
            vault: id,
            generation: Uuid::new_v4(),
            local: vault.path().canonicalize().unwrap(),
        };
        let remote =
            || RemoteClient::new(Environment::Staging, "test", Some(binding.generation)).unwrap();
        let mut engine = Engine::open(state.path(), binding.clone(), key, remote()).unwrap();
        assert!(!engine.up_to_date());
        engine.state.initialized = true;
        engine.state.last_success = Some(now_ms());
        assert!(!engine.up_to_date());
        engine.reconciled = Some(Instant::now());
        assert!(!engine.up_to_date());
        engine.last_poll_succeeded = true;
        assert!(engine.up_to_date());
        engine.changed(Entity::People);
        assert!(!engine.up_to_date());
        engine.checkpoints = Checkpoints::default();
        engine.state.conflicts.insert(
            "conflict".into(),
            Conflict {
                entity: Entity::People,
                local: Uuid::new_v4(),
                cloud: Uuid::new_v4(),
            },
        );
        assert!(!engine.up_to_date());
        engine.state.conflicts.clear();
        assert!(engine.up_to_date());
        engine.save().unwrap();
        drop(engine);
        let reopened = Engine::open(
            state.path(),
            binding.clone(),
            VaultKey::from_recovery_kit(&kit, id).unwrap(),
            remote(),
        )
        .unwrap();
        assert!(!reopened.up_to_date());
    }
}
