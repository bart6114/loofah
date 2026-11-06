#![forbid(unsafe_code)]

mod auth;
mod drafts;
mod events;
#[cfg(feature = "os-credentials")]
mod os_credentials;
mod profiles;
mod requests;
mod sse;

pub use auth::LoginAttempt;
pub use drafts::{
    DraftCache, DraftText, EditorTaskRecovery, NoteDraftRecovery, RecoveryEntry, RecoveryScope,
};
pub use events::{ConnectionState, RemoteEvent, Subscription};
#[cfg(feature = "os-credentials")]
pub use os_credentials::OsCredentials;
pub use profiles::{ConnectionProfile, ConnectionProfiles, ProfileLease};
pub use requests::{RemoteOperation, RemoteReply};

use std::{future::Future, pin::Pin, sync::Arc, time::Duration};

use hypr_loofah_protocol::{
    API_MAJOR, ApplicationError, Capabilities, ErrorCode, RemoteAuthConfig,
};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use sha2::{Digest, Sha256};
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;
use url::Url;

pub type Result<T> = std::result::Result<T, ApplicationError>;
pub type CredentialFuture<'a, T> = Pin<Box<dyn Future<Output = Result<T>> + Send + 'a>>;

/// Implemented by the native host. Refresh credentials never cross the renderer boundary.
pub trait CredentialStore: Send + Sync {
    fn load<'a>(&'a self, account: &'a str) -> CredentialFuture<'a, Option<String>>;
    fn save<'a>(&'a self, account: &'a str, value: Option<&'a str>) -> CredentialFuture<'a, ()>;
}

pub struct RemoteClient {
    address: Url,
    http: reqwest::Client,
    auth: RemoteAuthConfig,
    capabilities: Capabilities,
    account: String,
    credentials: Arc<dyn CredentialStore>,
    tokens: Mutex<auth::TokenState>,
    stop: CancellationToken,
}

impl RemoteClient {
    #[cfg(feature = "dev-fixtures")]
    pub async fn discover_fixture(
        address: &str,
        credentials: Arc<dyn CredentialStore>,
        root_pem: &[u8],
    ) -> Result<Arc<Self>> {
        let url = Url::parse(address).map_err(|_| invalid("Invalid fixture address"))?;
        let loopback = match url.host() {
            Some(url::Host::Domain("localhost")) => true,
            Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
            Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
            _ => false,
        };
        if url.scheme() != "https" || !loopback {
            return Err(invalid("Fixtures require a loopback HTTPS origin"));
        }
        let root = reqwest::Certificate::from_pem(root_pem)
            .map_err(|_| invalid("Invalid fixture root certificate"))?;
        let http = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(10))
            .tls_certs_only([root])
            .build()
            .map_err(|_| failure(ErrorCode::Internal, "Cannot initialize fixture HTTPS"))?;
        Self::discover_http(address, credentials, true, http).await
    }

    pub async fn discover(
        address: &str,
        credentials: Arc<dyn CredentialStore>,
        allow_insecure_loopback: bool,
    ) -> Result<Arc<Self>> {
        let http = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(10))
            .build()
            .map_err(|_| failure(ErrorCode::Internal, "Cannot initialize the HTTPS client"))?;
        Self::discover_http(address, credentials, allow_insecure_loopback, http).await
    }

    async fn discover_http(
        address: &str,
        credentials: Arc<dyn CredentialStore>,
        allow_insecure_loopback: bool,
        http: reqwest::Client,
    ) -> Result<Arc<Self>> {
        let address = Url::parse(address).map_err(|_| invalid("Invalid server address"))?;
        validate_endpoint(&address, allow_insecure_loopback)?;
        if address.path() != "/" || address.query().is_some() {
            return Err(invalid(
                "Use the server's HTTPS origin without a path or query",
            ));
        }
        let capabilities: Capabilities =
            public_json(&http, address.join("api/v1/capabilities").unwrap()).await?;
        if capabilities.api_major != API_MAJOR {
            return Err(failure(
                ErrorCode::IncompatibleVersion,
                "This server requires a different Loofah API version",
            ));
        }
        let auth: RemoteAuthConfig =
            public_json(&http, address.join("api/v1/auth/config").unwrap()).await?;
        auth::validate_config(&http, &auth, allow_insecure_loopback).await?;
        let coordinate = serde_json::to_vec(&(
            address.as_str(),
            &auth.issuer,
            &auth.client_id,
            &auth.audience,
            &auth.scopes,
        ))
        .map_err(|_| failure(ErrorCode::Internal, "Cannot identify the connection"))?;
        let account = Sha256::digest(coordinate)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        Ok(Arc::new(Self {
            address,
            http,
            auth,
            capabilities,
            account,
            credentials,
            tokens: Mutex::new(auth::TokenState::default()),
            stop: CancellationToken::new(),
        }))
    }

    pub fn address(&self) -> &str {
        self.address.as_str()
    }
    pub fn capabilities(&self) -> &Capabilities {
        &self.capabilities
    }
    pub fn credential_account(&self) -> &str {
        &self.account
    }

    pub async fn logout(&self) -> Result<()> {
        self.stop.cancel();
        let mut state = self.tokens.lock().await;
        *state = auth::TokenState {
            loaded: true,
            token: None,
        };
        self.credentials.save(&self.account, None).await
    }

    pub fn is_closed(&self) -> bool {
        self.stop.is_cancelled()
    }

    pub fn disconnect(&self) {
        self.stop.cancel();
    }

    fn endpoint(&self, segments: &[&str]) -> Result<Url> {
        if self.stop.is_cancelled() {
            return Err(cancelled());
        }
        let mut url = self.address.clone();
        let mut path = url
            .path_segments_mut()
            .map_err(|_| invalid("Invalid server address"))?;
        path.clear().extend(["api", "v1"]);
        for segment in segments {
            if segment.is_empty()
                || segment.len() > 512
                || matches!(*segment, "." | "..")
                || segment.contains(['/', '\\', '\0'])
            {
                return Err(invalid("Invalid resource identifier"));
            }
            path.push(segment);
        }
        drop(path);
        Ok(url)
    }
}

pub(crate) fn validate_endpoint(url: &Url, allow_insecure_loopback: bool) -> Result<()> {
    let loopback = url.host_str().is_some_and(|host| {
        host.trim_matches(['[', ']'])
            .parse::<std::net::IpAddr>()
            .is_ok_and(|ip| ip.is_loopback())
    });
    if !(url.scheme() == "https" || (allow_insecure_loopback && url.scheme() == "http" && loopback))
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
    {
        return Err(invalid("Server and identity endpoints must use HTTPS"));
    }
    Ok(())
}

async fn public_json<T: DeserializeOwned>(http: &reqwest::Client, url: Url) -> Result<T> {
    let response = http
        .get(url)
        .timeout(Duration::from_secs(30))
        .send()
        .await
        .map_err(|_| unavailable())?;
    if !response.status().is_success() {
        return Err(unavailable());
    }
    json_body(response, 1024 * 1024).await
}

async fn json_body<T: DeserializeOwned>(
    mut response: reqwest::Response,
    limit: usize,
) -> Result<T> {
    if response
        .content_length()
        .is_some_and(|length| length > limit as u64)
    {
        return Err(failure(
            ErrorCode::ResourceLimit,
            "The server response exceeds the size limit",
        ));
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|_| unavailable())? {
        if bytes.len().saturating_add(chunk.len()) > limit {
            return Err(failure(
                ErrorCode::ResourceLimit,
                "The server response exceeds the size limit",
            ));
        }
        bytes.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&bytes).map_err(|_| {
        failure(
            ErrorCode::IncompatibleVersion,
            "The server returned an invalid response",
        )
    })
}

pub fn failure(code: ErrorCode, message: &str) -> ApplicationError {
    ApplicationError {
        code,
        message: message.into(),
        request_id: None,
        retryable: false,
        current_revision: None,
        mutation_outcome: None,
    }
}
fn invalid(message: &str) -> ApplicationError {
    failure(ErrorCode::Validation, message)
}
fn cancelled() -> ApplicationError {
    failure(ErrorCode::Cancelled, "The connection was closed")
}
fn unavailable() -> ApplicationError {
    ApplicationError {
        retryable: true,
        ..failure(
            ErrorCode::Unavailable,
            "Cannot reach the server. Check your connection and try again.",
        )
    }
}

#[derive(Serialize, Deserialize)]
struct RefreshCredential {
    schema: u32,
    refresh_token: String,
}

#[cfg(test)]
mod tests;
