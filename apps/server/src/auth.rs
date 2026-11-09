use std::{
    collections::HashSet,
    sync::Arc,
    time::{Duration, Instant},
};

use hypr_loofah_protocol::{ErrorCode, RemoteAuthConfig};
use jsonwebtoken::{Algorithm, DecodingKey, Validation, decode, decode_header, jwk::JwkSet};
use serde::{Deserialize, de::DeserializeOwned};
use tokio::sync::Mutex;

use crate::{
    config::{Config, OidcConfig, validate_url},
    error::{ApiError, Result},
};

#[derive(Clone)]
pub struct Authenticator {
    config: OidcConfig,
    discovery: Discovery,
    client: reqwest::Client,
    keys: Arc<Mutex<Keys>>,
}

struct Keys {
    set: JwkSet,
    refreshed: Instant,
    attempted: Instant,
}

#[derive(Clone, Deserialize)]
struct Discovery {
    issuer: String,
    jwks_uri: url::Url,
    authorization_endpoint: url::Url,
    token_endpoint: url::Url,
    code_challenge_methods_supported: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct Identity {
    pub issuer: String,
    pub subject: String,
    pub name: String,
    pub expires_at: u64,
}

#[derive(Clone, Deserialize)]
struct Claims {
    sub: String,
    iss: String,
    exp: u64,
    iat: Option<u64>,
    scope: Option<String>,
    scp: Option<String>,
    name: Option<String>,
}

impl Authenticator {
    pub async fn discover(config: &Config) -> Result<Self> {
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(10))
            .build()
            .map_err(ApiError::internal)?;
        let discovery_url = format!(
            "{}/.well-known/openid-configuration",
            config.oidc.issuer.trim_end_matches('/')
        );
        let discovery: Discovery = fetch_json(&client, &discovery_url).await?;
        if discovery.issuer != config.oidc.issuer
            || !discovery
                .code_challenge_methods_supported
                .iter()
                .any(|method| method == "S256")
        {
            return Err(ApiError::new(
                ErrorCode::Unavailable,
                "OIDC issuer must match configuration and support S256 PKCE",
            ));
        }
        for endpoint in [
            &discovery.jwks_uri,
            &discovery.authorization_endpoint,
            &discovery.token_endpoint,
        ] {
            validate_url(endpoint, config.allow_insecure_loopback).map_err(|_| {
                ApiError::new(ErrorCode::Unavailable, "OIDC endpoints require HTTPS")
            })?;
        }
        let set: JwkSet = fetch_json(&client, discovery.jwks_uri.as_str()).await?;
        validate_keys(&set)?;
        Ok(Self {
            config: config.oidc.clone(),
            discovery,
            client,
            keys: Arc::new(Mutex::new(Keys {
                set,
                refreshed: Instant::now(),
                attempted: Instant::now(),
            })),
        })
    }

    pub fn public_config(&self) -> RemoteAuthConfig {
        RemoteAuthConfig {
            issuer: self.config.issuer.clone(),
            authorization_endpoint: self.discovery.authorization_endpoint.to_string(),
            token_endpoint: self.discovery.token_endpoint.to_string(),
            client_id: self.config.client_id.clone(),
            audience: self.config.audience.clone(),
            scopes: if self.config.login_scopes.is_empty() {
                vec![
                    "openid".into(),
                    "profile".into(),
                    "offline_access".into(),
                    self.config.required_scope.clone(),
                ]
            } else {
                self.config.login_scopes.clone()
            },
        }
    }

    pub async fn authenticate(&self, token: &str) -> Result<Identity> {
        if token.len() > 16_384 {
            return Err(unauthenticated());
        }
        let header = decode_header(token).map_err(|_| unauthenticated())?;
        if !matches!(header.alg, Algorithm::RS256 | Algorithm::ES256)
            || !header
                .typ
                .as_ref()
                .is_some_and(|typ| self.config.access_token_types.contains(typ))
        {
            return Err(unauthenticated());
        }
        let kid = header
            .kid
            .as_deref()
            .filter(|kid| !kid.is_empty() && kid.len() <= 256)
            .ok_or_else(unauthenticated)?;
        let key = {
            let mut keys = self.keys.lock().await;
            let age = keys.refreshed.elapsed();
            let stale = age >= Duration::from_secs(300);
            if stale || keys.set.find(kid).is_none() {
                if keys.attempted.elapsed() < Duration::from_secs(10) {
                    return Err(if stale {
                        ApiError::new(
                            ErrorCode::Unavailable,
                            "Issuer keys cannot be refreshed yet",
                        )
                    } else {
                        unauthenticated()
                    });
                }
                keys.attempted = Instant::now();
                let set = fetch_json(&self.client, self.discovery.jwks_uri.as_str()).await?;
                validate_keys(&set)?;
                keys.set = set;
                keys.refreshed = Instant::now();
            }
            let jwk = keys.set.find(kid).ok_or_else(unauthenticated)?;
            // A key explicitly designated for encryption must never verify an access token.
            let value = serde_json::to_value(jwk).map_err(ApiError::internal)?;
            if value.get("use").is_some_and(|usage| usage != "sig")
                || value
                    .get("key_ops")
                    .and_then(|ops| ops.as_array())
                    .is_some_and(|ops| !ops.iter().any(|op| op == "verify"))
                || value.get("alg").is_some_and(|alg| {
                    alg != match header.alg {
                        Algorithm::RS256 => "RS256",
                        _ => "ES256",
                    }
                })
            {
                return Err(unauthenticated());
            }
            DecodingKey::from_jwk(jwk).map_err(|_| unauthenticated())?
        };
        let mut validation = Validation::new(header.alg);
        validation.set_issuer(&[self.config.issuer.as_str()]);
        validation.set_audience(&[self.config.audience.as_str()]);
        validation.set_required_spec_claims(&["exp", "iss", "aud", "sub"]);
        validation.leeway = 0;
        validation.validate_nbf = true;
        let claims = decode::<Claims>(token, &key, &validation)
            .map_err(|_| unauthenticated())?
            .claims;
        let scopes = claims
            .scope
            .as_deref()
            .or(claims.scp.as_deref())
            .unwrap_or_default();
        if claims.sub.is_empty()
            || claims.sub.len() > 512
            || claims.iss != self.config.issuer
            || claims.iat.is_some_and(|at| at > now())
            || !scopes
                .split_ascii_whitespace()
                .any(|scope| scope == self.config.required_scope)
        {
            return Err(unauthenticated());
        }
        Ok(Identity {
            issuer: claims.iss,
            name: claims
                .name
                .filter(|name| !name.trim().is_empty())
                .unwrap_or_else(|| "Company member".into())
                .chars()
                .take(128)
                .collect(),
            subject: claims.sub,
            expires_at: claims.exp,
        })
    }
}

fn validate_keys(keys: &JwkSet) -> Result<()> {
    let mut seen = HashSet::new();
    if keys.keys.is_empty() || keys.keys.len() > 100 {
        return Err(ApiError::new(
            ErrorCode::Unavailable,
            "Invalid issuer key set",
        ));
    }
    for key in &keys.keys {
        if let Some(id) = &key.common.key_id
            && !seen.insert(id)
        {
            return Err(ApiError::new(
                ErrorCode::Unavailable,
                "Issuer key IDs must be unique",
            ));
        }
    }
    Ok(())
}

async fn fetch_json<T: DeserializeOwned>(client: &reqwest::Client, url: &str) -> Result<T> {
    let mut response = client
        .get(url)
        .send()
        .await
        .and_then(|response| response.error_for_status())
        .map_err(|_| ApiError::new(ErrorCode::Unavailable, "Identity provider is unavailable"))?;
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| ApiError::new(ErrorCode::Unavailable, "Identity provider response failed"))?
    {
        if bytes.len() + chunk.len() > 1_048_576 {
            return Err(ApiError::new(
                ErrorCode::Unavailable,
                "Identity provider response is too large",
            ));
        }
        bytes.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&bytes).map_err(|_| {
        ApiError::new(
            ErrorCode::Unavailable,
            "Identity provider response is invalid",
        )
    })
}

pub fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
fn unauthenticated() -> ApiError {
    ApiError::new(
        ErrorCode::Unauthenticated,
        "A valid company access token is required",
    )
}
