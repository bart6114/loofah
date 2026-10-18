use std::collections::BTreeMap;

use hypr_vault_access::{Error, Result, Role, User, Vault};
use serde::{Deserialize, Serialize};

use crate::Server;

#[derive(Clone, Serialize, Deserialize)]
pub struct Session {
    pub user_id: String,
    pub access_hash: String,
    pub refresh_hash: String,
    pub access_expires: i64,
    pub expires: i64,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Registry {
    pub version: u32,
    pub server_id: String,
    pub users: BTreeMap<String, User>,
    pub vaults: BTreeMap<String, Vault>,
    pub sessions: BTreeMap<String, Session>,
}

impl Default for Registry {
    fn default() -> Self {
        Self {
            version: 1,
            server_id: uuid::Uuid::new_v4().to_string(),
            users: BTreeMap::new(),
            vaults: BTreeMap::new(),
            sessions: BTreeMap::new(),
        }
    }
}

impl Server {
    pub async fn provision(
        &self,
        issuer: &str,
        subject: &str,
        name: &str,
        email: &str,
    ) -> Result<User> {
        let mut registry = self.registry.write().await;
        let mut next = registry.clone();
        let existing = next
            .users
            .values()
            .find(|user| user.issuer == issuer && user.subject == subject)
            .map(|user| user.id.clone());
        let id = existing.unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
        let user = User {
            id: id.clone(),
            issuer: issuer.into(),
            subject: subject.into(),
            name: name.into(),
            email: email.into(),
            disabled: next.users.get(&id).is_some_and(|user| user.disabled),
        };
        if user.disabled {
            return Err(Error::new("forbidden", "user is disabled"));
        }
        next.users.insert(id.clone(), user.clone());
        if !next
            .vaults
            .values()
            .any(|vault| vault.personal && vault.members.contains_key(&id))
        {
            let vault_id = uuid::Uuid::new_v4().to_string();
            hypr_vault_access::filesystem::create_dir_all(
                &self.config.data_dir.join("vaults").join(&vault_id),
            )?;
            next.vaults.insert(
                vault_id.clone(),
                Vault {
                    id: vault_id,
                    name: "Personal".into(),
                    personal: true,
                    members: BTreeMap::from([(id, Role::Owner)]),
                },
            );
        }
        self.save_registry(&next).await?;
        *registry = next;
        Ok(user)
    }

    pub async fn create_vault(&self, user: &User, name: &str) -> Result<Vault> {
        let name = name.trim();
        if name.is_empty() || name.len() > 255 {
            return Err(Error::new(
                "invalid_request",
                "vault name must contain 1–255 bytes",
            ));
        }
        let mut registry = self.registry.write().await;
        if registry
            .users
            .get(&user.id)
            .is_none_or(|user| user.disabled)
        {
            return Err(Error::new("unauthenticated", "user is unavailable"));
        }
        let mut next = registry.clone();
        let id = uuid::Uuid::new_v4().to_string();
        hypr_vault_access::filesystem::create_dir_all(
            &self.config.data_dir.join("vaults").join(&id),
        )?;
        let vault = Vault {
            id: id.clone(),
            name: name.into(),
            personal: false,
            members: BTreeMap::from([(user.id.clone(), Role::Owner)]),
        };
        next.vaults.insert(id.clone(), vault.clone());
        self.save_registry(&next).await?;
        *registry = next;
        drop(registry);
        self.notify(Some(id), "vaults", vec![]);
        Ok(vault)
    }

    pub async fn update_member(
        &self,
        user: &User,
        vault_id: &str,
        member_id: &str,
        role: Option<Role>,
    ) -> Result<Vault> {
        let runtime = self.runtime(vault_id).await?;
        let _gate = runtime.gate.write().await;
        let vault = self
            .authorize(
                user,
                vault_id,
                false,
                member_id != user.id || role.is_some(),
            )
            .await?;
        if vault.personal {
            return Err(Error::new(
                "forbidden",
                "personal vault membership is fixed",
            ));
        }
        let mut registry = self.registry.write().await;
        let mut next = registry.clone();
        if role.is_some() && next.users.get(member_id).is_none_or(|user| user.disabled) {
            return Err(Error::new("not_found", "user not found"));
        }
        let vault = next
            .vaults
            .get_mut(vault_id)
            .ok_or_else(|| Error::new("not_found", "vault not found"))?;
        if let Some(role) = role {
            vault.members.insert(member_id.into(), role);
        } else {
            vault.members.remove(member_id);
        }
        if !vault.members.values().any(|role| *role == Role::Owner) {
            return Err(Error::new(
                "conflict",
                "a shared vault must retain an owner",
            ));
        }
        let vault = vault.clone();
        self.save_registry(&next).await?;
        *registry = next;
        drop(registry);
        self.notify(Some(vault_id.into()), "members", vec![member_id.into()]);
        Ok(vault)
    }
}
