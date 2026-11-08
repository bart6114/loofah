use std::{path::PathBuf, sync::Arc};

use hypr_loofah_client::{
    ConnectionProfile, ConnectionProfiles, CredentialStore, OsCredentials, ProfileCommitGuard,
    ProfileGeneration, ProfileLease, RemoteClient, RemoteOperation, RemoteReply, failure,
};
use hypr_loofah_protocol::{ErrorCode, Me};
use tokio_util::sync::CancellationToken;

use crate::{Error, Result, cli::ProfileCommand, output};

#[derive(Clone, PartialEq)]
struct Binding {
    id: String,
    server: String,
    member: String,
    vault: String,
}

#[derive(Clone)]
pub(crate) struct RemoteTarget {
    store: Arc<ConnectionProfiles>,
    name: String,
    binding: Binding,
}

pub(crate) fn store() -> Result<Arc<ConnectionProfiles>> {
    let root = match std::env::var_os("LOOFAH_CLI_PROFILE_DIR") {
        Some(value) => PathBuf::from(value),
        None => {
            let base = dirs::data_local_dir().ok_or_else(|| {
                Error::operation(
                    "locate server profiles",
                    "Local application-data directory is unavailable",
                )
            })?;
            let command = std::env::args_os()
                .next()
                .and_then(|value| {
                    std::path::Path::new(&value)
                        .file_name()
                        .map(|name| name.to_string_lossy().into_owned())
                })
                .unwrap_or_default();
            base.join("loofah-cli")
                .join(channel(&command))
                .join("profiles")
        }
    };
    Ok(ConnectionProfiles::new(root)?)
}
fn channel(command: &str) -> &'static str {
    match command.to_ascii_lowercase().trim_end_matches(".exe") {
        "loof-dev" => "dev",
        "loof-staging" => "staging",
        _ => "stable",
    }
}
fn credentials(profile: &ConnectionProfile) -> Arc<OsCredentials> {
    Arc::new(OsCredentials::new(format!(
        "io.loofah.cli.remote-auth.{}",
        profile.id
    )))
}
fn binding(profile: &ConnectionProfile) -> Result<Binding> {
    match (&profile.member_id, &profile.vault_id) {
        (Some(member), Some(vault)) if !profile.credential_accounts.is_empty() => Ok(Binding {
            id: profile.id.clone(),
            server: profile.server.clone(),
            member: member.clone(),
            vault: vault.clone(),
        }),
        _ => Err(failure(
            ErrorCode::Unauthenticated,
            &format!(
                "Sign in with 'loof profiles login {}' before using this server profile",
                profile.name
            ),
        )
        .into()),
    }
}

async fn connect(
    lease: &Arc<ProfileLease>,
    profile: &ConnectionProfile,
) -> Result<Arc<RemoteClient>> {
    let credentials = lease.credentials(credentials(profile));
    #[cfg(feature = "dev-fixtures")]
    if profile.name == "qa-remote"
        && let Some(path) = std::env::var_os("LOOFAH_CLI_QA_ROOT_CA")
    {
        let path = PathBuf::from(path);
        let metadata = std::fs::metadata(&path).map_err(|_| {
            Error::operation(
                "read QA certificate",
                "Cannot read the fixture root certificate",
            )
        })?;
        if !metadata.is_file() || metadata.len() > 131_072 {
            return Err(Error::operation(
                "read QA certificate",
                "Invalid fixture root certificate file",
            ));
        }
        let pem = std::fs::read(path).map_err(|_| {
            Error::operation(
                "read QA certificate",
                "Cannot read the fixture root certificate",
            )
        })?;
        return Ok(RemoteClient::discover_fixture(&profile.server, credentials, &pem).await?);
    }
    Ok(RemoteClient::discover(&profile.server, credentials, false).await?)
}
async fn request(client: &Arc<RemoteClient>, operation: RemoteOperation) -> Result<RemoteReply> {
    Ok(client
        .execute(
            operation,
            &uuid::Uuid::new_v4().to_string(),
            CancellationToken::new(),
        )
        .await?)
}

impl RemoteTarget {
    pub async fn select(name: &str) -> Result<Self> {
        Self::select_from(store()?, name).await
    }
    async fn select_from(store: Arc<ConnectionProfiles>, name: &str) -> Result<Self> {
        let lease = store.open(name).await?;
        let profile = lease.profile().await;
        let binding = binding(&profile)?;
        Ok(Self {
            store,
            name: name.into(),
            binding,
        })
    }
    pub async fn execute(
        &self,
        build: impl FnOnce(String) -> RemoteOperation,
    ) -> Result<RemoteReply> {
        let lease = self.store.open(&self.name).await?;
        let profile = lease.profile().await;
        if binding(&profile)? != self.binding {
            return Err(failure(ErrorCode::Conflict,"This server profile changed its account or vault. Select it again; restart an MCP connection before continuing.").into());
        }
        let client = connect(&lease, &profile).await?;
        if !profile
            .credential_accounts
            .iter()
            .any(|account| account == client.credential_account())
        {
            return Err(failure(
                ErrorCode::Unauthenticated,
                "The server's authentication settings changed. Sign in to this profile again.",
            )
            .into());
        }
        let RemoteReply::Me(me) = request(&client, RemoteOperation::Me).await? else {
            return Err(unexpected());
        };
        if me.member.id != self.binding.member {
            return Err(failure(ErrorCode::Unauthenticated,"Saved credentials belong to another member. Sign in to this profile again before accessing its vault.").into());
        }
        if !client
            .capabilities()
            .features
            .iter()
            .any(|feature| feature == "agent_read")
        {
            return Err(failure(ErrorCode::IncompatibleVersion,"This server does not provide the agent read operations required by the CLI and MCP bridge").into());
        }
        request(&client, build(self.binding.vault.clone())).await
    }
}

pub(crate) struct RemoteSession {
    pub(super) client: Arc<RemoteClient>,
    lease: Arc<ProfileLease>,
    generation: ProfileGeneration,
    member: String,
    pub(super) vault: String,
    pub(super) request_id: String,
}
impl RemoteTarget {
    pub async fn begin(&self) -> Result<RemoteSession> {
        let lease = self.store.open(&self.name).await?;
        let generation = lease.generation().await?;
        let profile = generation.profile();
        if binding(profile)? != self.binding {
            return Err(failure(
                ErrorCode::Conflict,
                "This server profile changed its account or vault; select it again",
            )
            .into());
        }
        let client = connect(&lease, profile).await?;
        if !profile
            .credential_accounts
            .iter()
            .any(|account| account == client.credential_account())
        {
            return Err(failure(
                ErrorCode::Unauthenticated,
                "The server authentication settings changed; sign in to this profile again",
            )
            .into());
        }
        let session = RemoteSession {
            client,
            lease,
            generation,
            member: self.binding.member.clone(),
            vault: self.binding.vault.clone(),
            request_id: uuid::Uuid::new_v4().to_string(),
        };
        drop(session.guard().await?);
        Ok(session)
    }
}
impl RemoteSession {
    pub(super) async fn guard(&self) -> Result<ProfileCommitGuard> {
        let guard = self.lease.guard_generation(&self.generation).await?;
        self.verify_member().await?;
        Ok(guard)
    }
    pub(super) async fn verify_member(&self) -> Result<()> {
        let RemoteReply::Me(me) = request(&self.client, RemoteOperation::Me).await? else {
            return Err(unexpected());
        };
        if me.member.id != self.member {
            return Err(failure(
                ErrorCode::Unauthenticated,
                "The original server member changed; this command was not retargeted",
            )
            .into());
        }
        Ok(())
    }
    pub async fn execute(
        &self,
        build: impl FnOnce(String) -> RemoteOperation,
    ) -> Result<RemoteReply> {
        let _guard = self.guard().await?;
        Ok(self
            .client
            .execute(
                build(self.vault.clone()),
                &self.request_id,
                CancellationToken::new(),
            )
            .await?)
    }
}
impl Drop for RemoteSession {
    fn drop(&mut self) {
        self.client.disconnect();
    }
}

pub(crate) fn unexpected() -> Error {
    failure(
        ErrorCode::IncompatibleVersion,
        "The server returned an unexpected agent response",
    )
    .into()
}

pub async fn run(command: ProfileCommand, json: bool) -> Result<()> {
    let store = store()?;
    match command {
        ProfileCommand::List => {
            let profiles = store.list().await?;
            let data = profiles.iter().map(summary).collect::<Vec<_>>();
            let text = if json {
                output::json("profiles.list", &data, None)?
            } else if profiles.is_empty() {
                "No saved server profiles.".into()
            } else {
                profiles
                    .iter()
                    .map(|profile| {
                        format!(
                            "{}\t{}\t{}",
                            profile.name,
                            profile.server,
                            profile.vault_id.as_deref().unwrap_or("not signed in")
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("\n")
            };
            output::emit(&text);
            Ok(())
        }
        ProfileCommand::Logout { name } => {
            let lease = store.open(&name).await?;
            let profile = lease.profile().await;
            let credentials = credentials(&profile);
            for account in &profile.credential_accounts {
                credentials.save(account, None).await?;
            }
            lease.remove().await?;
            output::emit(&if json {
                output::json(
                    "profiles.logout",
                    &serde_json::json!({"name":name,"signed_out":true}),
                    None,
                )?
            } else {
                format!("Signed out and removed CLI profile {name}.")
            });
            Ok(())
        }
        ProfileCommand::Login {
            name,
            server,
            vault,
            no_browser,
        } => {
            let lease = match store.open(&name).await {
                Ok(lease) => lease,
                Err(error) if error.code == ErrorCode::NotFound => store
                    .create(
                        &name,
                        server.as_deref().ok_or_else(|| {
                            Error::operation(
                                "create server profile",
                                "Pass --server with the server's HTTPS origin for a new profile",
                            )
                        })?,
                    )
                    .await?,
                Err(error) => return Err(error.into()),
            };
            let mut profile = lease.profile().await;
            if let Some(server) = server
                && url::Url::parse(&server)
                    .map(|url| url.to_string())
                    .ok()
                    .as_deref()
                    != Some(&profile.server)
            {
                return Err(Error::operation(
                    "sign in",
                    "An existing profile keeps its server. Use another profile name for another server.",
                ));
            }
            let client = connect(&lease, &profile).await?;
            if !profile
                .credential_accounts
                .iter()
                .any(|account| account == client.credential_account())
            {
                profile
                    .credential_accounts
                    .push(client.credential_account().into());
                lease.save(profile.clone()).await?;
            }
            let me = match request(&client, RemoteOperation::Provision).await {
                Ok(RemoteReply::Me(me)) => me,
                Ok(_) => return Err(unexpected()),
                Err(Error::Remote(error)) if error.code == ErrorCode::Unauthenticated => {
                    let login = client.begin_login().await?;
                    let url = login.authorization_url().to_string();
                    if no_browser || open::that_detached(&url).is_err() {
                        eprintln!("Open this URL on this computer to sign in:\n{url}");
                    }
                    login.complete().await?;
                    let RemoteReply::Me(me) = request(&client, RemoteOperation::Provision).await?
                    else {
                        return Err(unexpected());
                    };
                    me
                }
                Err(error) => return Err(error),
            };
            bind(&mut profile, &client, me, vault).await?;
            lease.save(profile.clone()).await?;
            output::emit(&if json {
                output::json("profiles.login", &summary(&profile), None)?
            } else {
                format!(
                    "Signed in to {} using profile {} (vault {}).",
                    profile.server,
                    profile.name,
                    profile.vault_id.as_deref().unwrap()
                )
            });
            Ok(())
        }
    }
}
async fn bind(
    profile: &mut ConnectionProfile,
    client: &Arc<RemoteClient>,
    me: Me,
    vault: Option<String>,
) -> Result<()> {
    if profile
        .member_id
        .as_ref()
        .is_some_and(|member| member != &me.member.id)
    {
        return Err(failure(ErrorCode::Unauthenticated,"This profile is bound to another member. Log out first, or use a different profile name.").into());
    }
    let vault = vault
        .or_else(|| profile.vault_id.clone())
        .unwrap_or(me.personal_vault_id);
    let RemoteReply::Vault(selected) =
        request(client, RemoteOperation::Vault { vault_id: vault }).await?
    else {
        return Err(unexpected());
    };
    profile.member_id = Some(me.member.id);
    profile.vault_id = Some(selected.id);
    Ok(())
}
fn summary(profile: &ConnectionProfile) -> serde_json::Value {
    serde_json::json!({"name":profile.name,"server":profile.server,"member_id":profile.member_id,"vault_id":profile.vault_id,"configured":profile.member_id.is_some() && profile.vault_id.is_some()})
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn channel_names_preserve_separate_native_cli_configuration() {
        assert_eq!(channel("loof-dev.EXE"), "dev");
        assert_eq!(channel("loof-staging.exe"), "staging");
        assert_eq!(channel("loof"), "stable");
    }
    #[tokio::test]
    async fn remote_filesystem_paths_fail_without_contacting_a_server() {
        use clap::Parser;
        let root = tempfile::tempdir().unwrap();
        let store = ConnectionProfiles::new(root.path().join("profiles")).unwrap();
        let lease = store
            .create("work", "https://company.invalid")
            .await
            .unwrap();
        let mut profile = lease.profile().await;
        profile.member_id = Some("alice".into());
        profile.vault_id = Some("private".into());
        profile.credential_accounts.push("a".repeat(64));
        lease.save(profile).await.unwrap();
        drop(lease);
        let reader = crate::agent_reader::AgentReader::Remote(
            RemoteTarget::select_from(store, "work").await.unwrap(),
        );
        for args in [vec!["loof", "sessions", "path", "id"]] {
            let crate::cli::Command::Sessions { command } =
                crate::Args::try_parse_from(args).unwrap().command
            else {
                unreachable!()
            };
            let error = tokio::time::timeout(
                std::time::Duration::from_secs(1),
                crate::commands::meetings::run_with_reader(&reader, command, true),
            )
            .await
            .unwrap()
            .unwrap_err();
            assert!(
                error.to_string().contains("No local vault was accessed"),
                "{error}"
            );
        }
        assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 1);
    }

    #[tokio::test]
    async fn a_changed_or_removed_profile_fails_before_any_network_or_local_vault_access() {
        let root = tempfile::tempdir().unwrap();
        let store = ConnectionProfiles::new(root.path().join("profiles")).unwrap();
        let lease = store
            .create("work", "https://company.example")
            .await
            .unwrap();
        let mut profile = lease.profile().await;
        profile.member_id = Some("alice".into());
        profile.vault_id = Some("private".into());
        profile.credential_accounts.push("a".repeat(64));
        lease.save(profile.clone()).await.unwrap();
        drop(lease);
        let target = RemoteTarget::select_from(store.clone(), "work")
            .await
            .unwrap();
        let lease = store.open("work").await.unwrap();
        profile.vault_id = Some("other".into());
        lease.save(profile).await.unwrap();
        drop(lease);
        assert_eq!(target.begin().await.err().unwrap().code(), "conflict");
        let error = target
            .execute(|_| panic!("changed profiles must not dispatch"))
            .await
            .unwrap_err();
        assert_eq!(error.code(), "conflict");
        let lease = store.open("work").await.unwrap();
        lease.remove().await.unwrap();
        drop(lease);
        assert_eq!(target.begin().await.err().unwrap().code(), "not_found");
        let error = target
            .execute(|_| panic!("removed profiles must not dispatch"))
            .await
            .unwrap_err();
        assert_eq!(error.code(), "not_found");
    }
}

#[cfg(test)]
#[path = "profiles/gateway_tests.rs"]
mod gateway_tests;
