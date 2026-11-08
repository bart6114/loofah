use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::Arc,
};

use hypr_loofah_protocol::{
    ErrorCode, Me, MemberInfo, Membership, VaultInfo, VaultKind, VaultMutation, VaultRole,
};
use serde::{Deserialize, Serialize};
use tokio::sync::{Mutex, OwnedMutexGuard, OwnedRwLockReadGuard, RwLock, watch};

use crate::{
    auth::Identity,
    error::{ApiError, Result},
};

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Member {
    info: MemberInfo,
    issuer: String,
    subject: String,
    personal_vault_id: String,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Vault {
    id: String,
    name: String,
    kind: VaultKind,
    revision: String,
    deleted: bool,
    members: BTreeMap<String, VaultRole>,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Receipt {
    name: String,
    vault_id: String,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Stored {
    schema_version: u32,
    members: BTreeMap<String, Member>,
    vaults: BTreeMap<String, Vault>,
    receipts: BTreeMap<String, Receipt>,
}

struct State {
    healthy: bool,
    stored: Stored,
    gates: BTreeMap<String, Arc<RwLock<()>>>,
}

#[derive(Clone)]
pub struct Catalog {
    #[cfg(test)]
    pub(crate) pause:
        Arc<std::sync::Mutex<Option<(std::sync::mpsc::Sender<()>, Arc<std::sync::Barrier>)>>>,
    _lease: Arc<std::fs::File>,
    path: PathBuf,
    state: Arc<Mutex<State>>,
    changes: watch::Sender<u64>,
}

pub struct VaultAccess {
    _catalog_lease: Arc<std::fs::File>,
    pub info: VaultInfo,
    pub member_id: String,
    _guard: OwnedRwLockReadGuard<()>,
}

impl Catalog {
    pub async fn open(root: &Path) -> Result<Self> {
        let root = root.to_path_buf();
        let lock_root = root.clone();
        let lease = tokio::task::spawn_blocking(move || -> Result<std::fs::File> {
            std::fs::create_dir_all(&lock_root)?;
            let lock_path = lock_root.join(".loofah-server.lock");
            if std::fs::symlink_metadata(&lock_path).is_ok_and(|meta| meta.file_type().is_symlink())
            {
                return Err(ApiError::new(
                    ErrorCode::Unavailable,
                    "Service lock must not be a symbolic link",
                ));
            }
            let lease = std::fs::File::options()
                .read(true)
                .write(true)
                .create(true)
                .truncate(false)
                .open(lock_path)?;
            lease.try_lock().map_err(|_| {
                ApiError::new(
                    ErrorCode::Unavailable,
                    "Service storage is already owned or cannot be locked",
                )
            })?;
            Ok(lease)
        })
        .await
        .map_err(ApiError::internal)??;
        let path = root.join("catalog.json");
        let file = path.clone();
        let stored = tokio::task::spawn_blocking(move || -> Result<Stored> {
            if std::fs::symlink_metadata(&file)
                .is_ok_and(|metadata| metadata.file_type().is_symlink())
            {
                return Err(ApiError::new(
                    ErrorCode::Unavailable,
                    "Service catalog must not be a symbolic link",
                ));
            }
            match std::fs::read(file) {
                Ok(bytes) => Ok(serde_json::from_slice(&bytes)?),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Stored {
                    schema_version: 1,
                    members: BTreeMap::new(),
                    vaults: BTreeMap::new(),
                    receipts: BTreeMap::new(),
                }),
                Err(error) => Err(error.into()),
            }
        })
        .await
        .map_err(ApiError::internal)??;
        validate_stored(&stored)?;
        let gates = stored
            .vaults
            .keys()
            .map(|id| (id.clone(), Arc::new(RwLock::new(()))))
            .collect();
        Ok(Self {
            #[cfg(test)]
            pause: Default::default(),
            _lease: Arc::new(lease),
            path,
            state: Arc::new(Mutex::new(State {
                healthy: true,
                stored,
                gates,
            })),
            changes: watch::channel(0).0,
        })
    }

    pub async fn ready(&self) -> bool {
        self.state.lock().await.healthy
    }

    pub fn subscribe(&self) -> watch::Receiver<u64> {
        self.changes.subscribe()
    }

    pub async fn provision(&self, identity: &Identity) -> Result<Me> {
        let state = self.state.clone().lock_owned().await;
        check_health(&state)?;
        if let Some(member) = find_member(&state.stored, identity) {
            return Ok(me(member));
        }
        let mut next = state.stored.clone();
        let member_id = new_id();
        let vault_id = new_id();
        let member = Member {
            info: MemberInfo {
                id: member_id.clone(),
                name: identity.name.clone(),
            },
            issuer: identity.issuer.clone(),
            subject: identity.subject.clone(),
            personal_vault_id: vault_id.clone(),
        };
        next.vaults.insert(
            vault_id.clone(),
            Vault {
                id: vault_id.clone(),
                name: "Personal".into(),
                kind: VaultKind::Personal,
                revision: new_id(),
                deleted: false,
                members: BTreeMap::from([(member_id.clone(), VaultRole::Owner)]),
            },
        );
        next.members.insert(member_id, member.clone());
        self.commit(state, next).await?;
        Ok(me(&member))
    }

    pub async fn me(&self, identity: &Identity) -> Result<Me> {
        let state = self.state.lock().await;
        check_health(&state)?;
        find_member(&state.stored, identity)
            .map(me)
            .ok_or_else(not_provisioned)
    }

    pub async fn list(&self, identity: &Identity) -> Result<Vec<VaultInfo>> {
        let state = self.state.lock().await;
        check_health(&state)?;
        let member = find_member(&state.stored, identity).ok_or_else(not_provisioned)?;
        Ok(state
            .stored
            .vaults
            .values()
            .filter_map(|vault| info(vault, &member.info.id))
            .collect())
    }

    pub async fn directory(&self, identity: &Identity) -> Result<Vec<MemberInfo>> {
        let state = self.state.lock().await;
        check_health(&state)?;
        find_member(&state.stored, identity).ok_or_else(not_provisioned)?;
        Ok(state
            .stored
            .members
            .values()
            .filter(|member| member.issuer == identity.issuer)
            .map(|member| member.info.clone())
            .collect())
    }

    pub async fn access(&self, identity: &Identity, id: &str, edit: bool) -> Result<VaultAccess> {
        let member_id = self.me(identity).await?.member.id;
        self.access_member(&member_id, id, edit).await
    }

    pub(crate) async fn access_member(
        &self,
        member_id: &str,
        id: &str,
        edit: bool,
    ) -> Result<VaultAccess> {
        let gate = {
            let state = self.state.lock().await;
            check_health(&state)?;
            if !state.stored.members.contains_key(member_id) {
                return Err(not_provisioned());
            }
            state
                .stored
                .vaults
                .get(id)
                .and_then(|vault| info(vault, member_id))
                .ok_or_else(not_found)?;
            state.gates[id].clone()
        };
        let guard = gate.read_owned().await;
        let state = self.state.lock().await;
        check_health(&state)?;
        if !state.stored.members.contains_key(member_id) {
            return Err(not_provisioned());
        }
        let info = state
            .stored
            .vaults
            .get(id)
            .and_then(|vault| info(vault, member_id))
            .ok_or_else(not_found)?;
        if edit && !info.role.can_edit() {
            return Err(ApiError::new(
                ErrorCode::Forbidden,
                "This vault is read-only",
            ));
        }
        Ok(VaultAccess {
            _catalog_lease: self._lease.clone(),
            info,
            member_id: member_id.into(),
            _guard: guard,
        })
    }

    pub async fn create_shared(
        &self,
        identity: &Identity,
        request_id: &str,
        name: &str,
    ) -> Result<VaultInfo> {
        validate_name(name)?;
        let request_id = uuid::Uuid::parse_str(request_id)
            .map_err(|_| ApiError::new(ErrorCode::Validation, "A UUID request ID is required"))?;
        let state = self.state.clone().lock_owned().await;
        check_health(&state)?;
        let member = find_member(&state.stored, identity)
            .ok_or_else(not_provisioned)?
            .info
            .id
            .clone();
        let key = format!("{member}:{request_id}");
        if let Some(receipt) = state.stored.receipts.get(&key) {
            if receipt.name != name {
                return Err(ApiError::new(
                    ErrorCode::Conflict,
                    "Request ID was already used with different input",
                ));
            }
            return state
                .stored
                .vaults
                .get(&receipt.vault_id)
                .and_then(|vault| info(vault, &member))
                .ok_or_else(not_found);
        }
        if state
            .stored
            .vaults
            .values()
            .filter(|vault| !vault.deleted && vault.members.get(&member) == Some(&VaultRole::Owner))
            .count()
            >= 100
        {
            return Err(ApiError::new(
                ErrorCode::ResourceLimit,
                "Vault ownership limit reached",
            ));
        }
        let mut next = state.stored.clone();
        let id = new_id();
        let vault = Vault {
            id: id.clone(),
            name: name.into(),
            kind: VaultKind::Shared,
            revision: new_id(),
            deleted: false,
            members: BTreeMap::from([(member.clone(), VaultRole::Owner)]),
        };
        next.receipts.insert(
            key,
            Receipt {
                name: name.into(),
                vault_id: id.clone(),
            },
        );
        next.vaults.insert(id.clone(), vault.clone());
        self.commit(state, next).await?;
        Ok(info(&vault, &member).unwrap())
    }

    pub async fn memberships(&self, identity: &Identity, id: &str) -> Result<Vec<Membership>> {
        let access = self.access(identity, id, false).await?;
        let state = self.state.lock().await;
        check_health(&state)?;
        let vault = state.stored.vaults.get(&access.info.id).unwrap();
        Ok(vault
            .members
            .iter()
            .map(|(id, role)| Membership {
                member: state.stored.members[id].info.clone(),
                role: *role,
            })
            .collect())
    }

    pub async fn set_membership(
        &self,
        identity: &Identity,
        id: &str,
        target: &str,
        role: Option<VaultRole>,
        revision: &str,
    ) -> Result<VaultMutation> {
        self.mutate(identity, id, revision, |stored, vault| {
            if stored
                .members
                .get(target)
                .is_none_or(|member| member.issuer != identity.issuer)
            {
                return Err(ApiError::new(
                    ErrorCode::NotFound,
                    "Member must have signed into this company first",
                ));
            }
            if let Some(role) = role {
                vault.members.insert(target.into(), role);
            } else {
                vault.members.remove(target);
            }
            if !vault.members.values().any(|role| *role == VaultRole::Owner) {
                return Err(ApiError::new(
                    ErrorCode::Conflict,
                    "A shared vault must retain an owner",
                ));
            }
            Ok(())
        })
        .await
    }

    pub async fn rename(
        &self,
        identity: &Identity,
        id: &str,
        name: &str,
        revision: &str,
    ) -> Result<VaultMutation> {
        validate_name(name)?;
        self.mutate(identity, id, revision, |_, vault| {
            vault.name = name.into();
            Ok(())
        })
        .await
    }

    pub async fn delete(
        &self,
        identity: &Identity,
        id: &str,
        revision: &str,
    ) -> Result<VaultMutation> {
        self.mutate(identity, id, revision, |_, vault| {
            vault.deleted = true;
            Ok(())
        })
        .await
    }

    async fn mutate(
        &self,
        identity: &Identity,
        id: &str,
        revision: &str,
        update: impl FnOnce(&Stored, &mut Vault) -> Result<()>,
    ) -> Result<VaultMutation> {
        let gate = self.gate(identity, id).await?;
        // Writers hold a read lease through their commit; revocation holds this exclusive lease.
        let _guard = gate.write_owned().await;
        let state = self.state.clone().lock_owned().await;
        check_health(&state)?;
        let member = find_member(&state.stored, identity)
            .ok_or_else(not_provisioned)?
            .info
            .id
            .clone();
        let current = state
            .stored
            .vaults
            .get(id)
            .filter(|vault| !vault.deleted && vault.members.contains_key(&member))
            .ok_or_else(not_found)?;
        if current.kind != VaultKind::Shared
            || current.members.get(&member) != Some(&VaultRole::Owner)
        {
            return Err(ApiError::new(
                ErrorCode::Forbidden,
                "Only shared-vault owners can manage access",
            ));
        }
        if current.revision != revision {
            let mut error = ApiError::new(
                ErrorCode::Conflict,
                "Vault configuration changed; reload it before retrying",
            );
            error.0.current_revision = Some(current.revision.clone());
            return Err(error);
        }
        let mut vault = current.clone();
        update(&state.stored, &mut vault)?;
        vault.revision = new_id();
        let response = VaultMutation {
            vault_id: vault.id.clone(),
            revision: vault.revision.clone(),
        };
        let mut next = state.stored.clone();
        next.vaults.insert(id.into(), vault);
        self.commit(state, next).await?;
        Ok(response)
    }

    async fn gate(&self, identity: &Identity, id: &str) -> Result<Arc<RwLock<()>>> {
        let state = self.state.lock().await;
        check_health(&state)?;
        let member = find_member(&state.stored, identity).ok_or_else(not_provisioned)?;
        state
            .stored
            .vaults
            .get(id)
            .and_then(|vault| info(vault, &member.info.id))
            .ok_or_else(not_found)?;
        Ok(state.gates[id].clone())
    }

    async fn commit(&self, mut state: OwnedMutexGuard<State>, stored: Stored) -> Result<()> {
        let content = serde_json::to_string_pretty(&stored)?;
        let path = self.path.clone();
        let changes = self.changes.clone();
        let lease = self._lease.clone();
        #[cfg(test)]
        let pause = self.pause.lock().unwrap().take();
        // The worker owns the state lock: cancelling an HTTP request cannot leave memory behind disk.
        tokio::task::spawn_blocking(move || -> Result<()> {
            let _lease = lease;
            #[cfg(test)]
            if let Some((entered, resume)) = pause {
                let _ = entered.send(());
                resume.wait();
            }
            hypr_storage::fs::atomic_write(&path, &content)?;
            for id in stored.vaults.keys() {
                state
                    .gates
                    .entry(id.clone())
                    .or_insert_with(|| Arc::new(RwLock::new(())));
            }
            state.stored = stored;
            #[cfg(unix)]
            if let Err(cause) =
                std::fs::File::open(path.parent().unwrap()).and_then(|parent| parent.sync_all())
            {
                state.healthy = false;
                changes.send_modify(|revision| *revision = revision.wrapping_add(1));
                let mut error = ApiError::internal(cause);
                error.0.mutation_outcome = Some(hypr_loofah_protocol::MutationOutcome::Unknown);
                return Err(error);
            }
            changes.send_modify(|revision| *revision = revision.wrapping_add(1));
            Ok(())
        })
        .await
        .map_err(ApiError::internal)?
    }
}

fn find_member<'a>(stored: &'a Stored, identity: &Identity) -> Option<&'a Member> {
    stored
        .members
        .values()
        .find(|member| member.issuer == identity.issuer && member.subject == identity.subject)
}
fn me(member: &Member) -> Me {
    Me {
        member: member.info.clone(),
        personal_vault_id: member.personal_vault_id.clone(),
    }
}
fn new_id() -> String {
    uuid::Uuid::new_v4().to_string()
}
fn info(vault: &Vault, member: &str) -> Option<VaultInfo> {
    if vault.deleted {
        return None;
    }
    vault.members.get(member).map(|role| VaultInfo {
        id: vault.id.clone(),
        name: vault.name.clone(),
        kind: vault.kind,
        role: *role,
        revision: vault.revision.clone(),
    })
}
fn not_found() -> ApiError {
    ApiError::new(ErrorCode::NotFound, "Vault not found")
}
fn not_provisioned() -> ApiError {
    ApiError::new(
        ErrorCode::Forbidden,
        "Sign into Loofah before accessing company vaults",
    )
}
fn validate_name(name: &str) -> Result<()> {
    if name.trim().is_empty() || name.len() > 256 || name.chars().any(char::is_control) {
        return Err(ApiError::new(
            ErrorCode::Validation,
            "Vault name must contain 1–256 bytes without control characters",
        ));
    }
    Ok(())
}

fn validate_stored(stored: &Stored) -> Result<()> {
    let invalid = || {
        ApiError::new(
            ErrorCode::Unavailable,
            "Service catalog is invalid or uses an unsupported version",
        )
    };
    if stored.schema_version != 1 {
        return Err(invalid());
    }
    let mut identities = std::collections::HashSet::new();
    for (id, member) in &stored.members {
        if uuid::Uuid::parse_str(id).is_err()
            || id != &member.info.id
            || !identities.insert((&member.issuer, &member.subject))
        {
            return Err(invalid());
        }
        let personal = stored
            .vaults
            .get(&member.personal_vault_id)
            .ok_or_else(invalid)?;
        if personal.kind != VaultKind::Personal
            || personal.deleted
            || personal.members.len() != 1
            || personal.members.get(id) != Some(&VaultRole::Owner)
        {
            return Err(invalid());
        }
    }
    for (id, vault) in &stored.vaults {
        if uuid::Uuid::parse_str(id).is_err()
            || id != &vault.id
            || uuid::Uuid::parse_str(&vault.revision).is_err()
            || !vault.members.values().any(|role| *role == VaultRole::Owner)
            || vault
                .members
                .keys()
                .any(|member| !stored.members.contains_key(member))
        {
            return Err(invalid());
        }
    }
    Ok(())
}

fn check_health(state: &State) -> Result<()> {
    if !state.healthy {
        return Err(ApiError::new(
            ErrorCode::Unavailable,
            "Catalog persistence failed; operator recovery is required",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn identity(subject: &str) -> Identity {
        Identity {
            issuer: "https://company.example".into(),
            subject: subject.into(),
            name: subject.into(),
            expires_at: crate::auth::now() + 3600,
        }
    }

    #[tokio::test]
    async fn concurrent_login_provisions_one_private_vault_and_survives_restart() {
        let root = tempfile::tempdir().unwrap();
        let catalog = Catalog::open(root.path()).await.unwrap();
        let mut tasks = Vec::new();
        for _ in 0..20 {
            let catalog = catalog.clone();
            tasks.push(tokio::spawn(async move {
                catalog.provision(&identity("alice")).await.unwrap()
            }));
        }
        let mut results = Vec::new();
        for task in tasks {
            results.push(task.await.unwrap());
        }
        assert!(results.iter().all(|result| result == &results[0]));
        assert!(Catalog::open(root.path()).await.is_err());
        let personal = catalog.list(&identity("alice")).await.unwrap();
        assert_eq!(personal.len(), 1);
        assert_eq!(personal[0].kind, VaultKind::Personal);
        drop(catalog);
        let reopened = Catalog::open(root.path()).await.unwrap();
        assert_eq!(reopened.me(&identity("alice")).await.unwrap(), results[0]);
        assert_eq!(reopened.list(&identity("alice")).await.unwrap(), personal);
    }

    #[tokio::test]
    async fn private_vaults_roles_existing_members_and_last_owner_are_enforced() {
        let root = tempfile::tempdir().unwrap();
        let catalog = Catalog::open(root.path()).await.unwrap();
        let alice = catalog.provision(&identity("alice")).await.unwrap();
        let bob = catalog.provision(&identity("bob")).await.unwrap();
        assert_eq!(
            catalog
                .access(&identity("bob"), &alice.personal_vault_id, false)
                .await
                .err()
                .unwrap()
                .0
                .code,
            ErrorCode::NotFound
        );
        let personal = catalog.list(&identity("alice")).await.unwrap().remove(0);
        assert_eq!(
            catalog
                .set_membership(
                    &identity("alice"),
                    &personal.id,
                    &bob.member.id,
                    Some(VaultRole::Reader),
                    &personal.revision
                )
                .await
                .unwrap_err()
                .0
                .code,
            ErrorCode::Forbidden
        );
        let request_id = new_id();
        let shared = catalog
            .create_shared(&identity("alice"), &request_id, "Team")
            .await
            .unwrap();
        assert_eq!(
            catalog
                .create_shared(&identity("alice"), &request_id, "Team")
                .await
                .unwrap(),
            shared
        );
        assert_eq!(
            catalog
                .create_shared(&identity("alice"), &request_id, "Other")
                .await
                .unwrap_err()
                .0
                .code,
            ErrorCode::Conflict
        );
        assert_eq!(
            catalog
                .set_membership(
                    &identity("alice"),
                    &shared.id,
                    &new_id(),
                    Some(VaultRole::Editor),
                    &shared.revision
                )
                .await
                .unwrap_err()
                .0
                .code,
            ErrorCode::NotFound
        );
        assert_eq!(
            catalog
                .set_membership(
                    &identity("alice"),
                    &shared.id,
                    &alice.member.id,
                    None,
                    &shared.revision
                )
                .await
                .unwrap_err()
                .0
                .code,
            ErrorCode::Conflict
        );
        let changed = catalog
            .set_membership(
                &identity("alice"),
                &shared.id,
                &bob.member.id,
                Some(VaultRole::Reader),
                &shared.revision,
            )
            .await
            .unwrap();
        assert_eq!(
            catalog
                .access(&identity("bob"), &shared.id, true)
                .await
                .err()
                .unwrap()
                .0
                .code,
            ErrorCode::Forbidden
        );
        assert_eq!(
            catalog
                .rename(&identity("bob"), &shared.id, "Changed", &changed.revision)
                .await
                .unwrap_err()
                .0
                .code,
            ErrorCode::Forbidden
        );
        assert_eq!(
            catalog
                .rename(&identity("alice"), &shared.id, "Changed", &shared.revision)
                .await
                .unwrap_err()
                .0
                .code,
            ErrorCode::Conflict
        );
        assert_eq!(catalog.list(&identity("bob")).await.unwrap().len(), 2);
        assert_eq!(
            catalog.directory(&identity("alice")).await.unwrap().len(),
            2
        );
        catalog
            .delete(&identity("alice"), &shared.id, &changed.revision)
            .await
            .unwrap();
        assert_eq!(
            catalog
                .access(&identity("bob"), &shared.id, false)
                .await
                .err()
                .unwrap()
                .0
                .code,
            ErrorCode::NotFound
        );
    }

    #[tokio::test]
    async fn revocation_waits_for_a_committing_writer_then_denies_new_access() {
        let root = tempfile::tempdir().unwrap();
        let catalog = Catalog::open(root.path()).await.unwrap();
        catalog.provision(&identity("alice")).await.unwrap();
        let bob = catalog.provision(&identity("bob")).await.unwrap();
        let shared = catalog
            .create_shared(&identity("alice"), &new_id(), "Team")
            .await
            .unwrap();
        let changed = catalog
            .set_membership(
                &identity("alice"),
                &shared.id,
                &bob.member.id,
                Some(VaultRole::Editor),
                &shared.revision,
            )
            .await
            .unwrap();
        let writer = catalog
            .access(&identity("bob"), &shared.id, true)
            .await
            .unwrap();
        let mut task = {
            let catalog = catalog.clone();
            let shared = shared.clone();
            tokio::spawn(async move {
                catalog
                    .set_membership(
                        &identity("alice"),
                        &shared.id,
                        &bob.member.id,
                        None,
                        &changed.revision,
                    )
                    .await
            })
        };
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(20), &mut task)
                .await
                .is_err()
        );
        drop(writer);
        tokio::time::timeout(std::time::Duration::from_secs(2), task)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(
            catalog
                .access(&identity("bob"), &shared.id, true)
                .await
                .err()
                .unwrap()
                .0
                .code,
            ErrorCode::NotFound
        );
    }

    #[tokio::test]
    async fn cancelled_provision_does_not_leave_memory_behind_a_committed_catalog() {
        let root = tempfile::tempdir().unwrap();
        let catalog = Catalog::open(root.path()).await.unwrap();
        let (entered, receiver) = std::sync::mpsc::channel();
        let resume = Arc::new(std::sync::Barrier::new(2));
        *catalog.pause.lock().unwrap() = Some((entered, resume.clone()));
        let task = {
            let catalog = catalog.clone();
            tokio::spawn(async move { catalog.provision(&identity("alice")).await })
        };
        tokio::task::spawn_blocking(move || {
            receiver.recv_timeout(std::time::Duration::from_secs(2))
        })
        .await
        .unwrap()
        .unwrap();
        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
        tokio::task::spawn_blocking(move || resume.wait())
            .await
            .unwrap();
        let me = catalog.me(&identity("alice")).await.unwrap();
        assert_eq!(catalog.list(&identity("alice")).await.unwrap().len(), 1);
        drop(catalog);
        let reopened = Catalog::open(root.path()).await.unwrap();
        assert_eq!(reopened.me(&identity("alice")).await.unwrap(), me);
    }
}
