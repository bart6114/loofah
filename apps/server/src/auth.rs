use std::sync::Arc;

use axum::{
    Json,
    extract::{Query, State},
    http::HeaderMap,
    response::Redirect,
};
use base64::Engine;
use hypr_vault_access::{Credentials, Error, Result, User, digest};
use openidconnect::{
    AuthorizationCode, ClientId, ClientSecret, CsrfToken, IssuerUrl, Nonce, PkceCodeChallenge,
    PkceCodeVerifier, RedirectUrl, Scope, TokenResponse,
    core::{CoreAuthenticationFlow, CoreClient, CoreProviderMetadata},
};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::{Server, http::ApiError, registry::Session};

pub struct Login {
    created: i64,
    metadata: CoreProviderMetadata,
    verifier: String,
    nonce: String,
    native_challenge: String,
    callback: String,
    native_state: String,
    grant: Option<String>,
    user: Option<User>,
}

fn secret() -> String {
    format!(
        "{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    )
}

pub async fn authenticate(server: &Server, headers: &HeaderMap) -> Result<User> {
    let token = headers
        .get("authorization")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .ok_or_else(|| Error::new("unauthenticated", "sign in to the company server"))?;
    authenticate_token(server, token).await
}

pub async fn authenticate_token(server: &Server, token: &str) -> Result<User> {
    let hash = digest(token.as_bytes());
    let registry = server.registry.read().await;
    let now = chrono::Utc::now().timestamp();
    let session = registry
        .sessions
        .values()
        .find(|session| {
            session.access_hash == hash && session.access_expires > now && session.expires > now
        })
        .ok_or_else(|| Error::new("unauthenticated", "session expired"))?;
    let user = registry
        .users
        .get(&session.user_id)
        .filter(|user| !user.disabled)
        .ok_or_else(|| Error::new("unauthenticated", "user is unavailable"))?;
    Ok(user.clone())
}

async fn credentials(
    server: &Server,
    user: User,
    expires: i64,
    previous: Option<&str>,
) -> Result<Credentials> {
    let now = chrono::Utc::now().timestamp();
    let access_token = secret();
    let refresh_token = secret();
    let access_expires = (now + 15 * 60).min(expires);
    let mut registry = server.registry.write().await;
    if registry
        .users
        .get(&user.id)
        .is_none_or(|user| user.disabled)
    {
        return Err(Error::new("unauthenticated", "user is unavailable"));
    }
    let mut next = registry.clone();
    if let Some(previous) = previous {
        let session = next
            .sessions
            .remove(previous)
            .ok_or_else(|| Error::new("unauthenticated", "refresh token was already used"))?;
        if session.expires <= now {
            return Err(Error::new("unauthenticated", "session expired"));
        }
    }
    next.sessions.retain(|_, session| session.expires > now);
    next.sessions.insert(
        digest(refresh_token.as_bytes()),
        Session {
            user_id: user.id.clone(),
            access_hash: digest(access_token.as_bytes()),
            refresh_hash: digest(refresh_token.as_bytes()),
            access_expires,
            expires,
        },
    );
    server.save_registry(&next).await?;
    *registry = next;
    Ok(Credentials {
        access_token,
        refresh_token,
        expires_at: access_expires,
        user,
    })
}

#[derive(Deserialize, utoipa::ToSchema)]
pub struct StartLogin {
    pub callback: String,
    pub challenge: String,
    pub state: String,
}

#[utoipa::path(post,path="/v1/auth/start",request_body=StartLogin,responses((status=200,body=Value)))]
pub async fn start(
    State(server): State<Arc<Server>>,
    Json(request): Json<StartLogin>,
) -> std::result::Result<Json<Value>, ApiError> {
    let callback = url::Url::parse(&request.callback)
        .map_err(|_| Error::new("invalid_request", "invalid native callback"))?;
    if callback.scheme() != "http"
        || callback.host_str() != Some("127.0.0.1")
        || callback.port().is_none()
        || !callback.username().is_empty()
        || callback.password().is_some()
        || callback.query().is_some()
        || callback.fragment().is_some()
    {
        return Err(Error::new(
            "invalid_request",
            "native callback must be an IPv4 loopback URL with an explicit port",
        )
        .into());
    }
    if request.challenge.len() != 43 || request.state.len() < 32 || request.state.len() > 256 {
        return Err(Error::new("invalid_request", "invalid native login proof").into());
    }
    let http = openidconnect::reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .redirect(openidconnect::reqwest::redirect::Policy::none())
        .build()
        .map_err(|error| Error::new("unavailable", error.to_string()))?;
    let metadata = CoreProviderMetadata::discover_async(
        IssuerUrl::new(server.config.issuer.clone())
            .map_err(|_| Error::new("invalid_request", "invalid issuer"))?,
        &http,
    )
    .await
    .map_err(|error| Error::new("unavailable", error.to_string()))?;
    let client = CoreClient::from_provider_metadata(
        metadata.clone(),
        ClientId::new(server.config.client_id.clone()),
        server.config.client_secret.clone().map(ClientSecret::new),
    )
    .set_redirect_uri(
        RedirectUrl::new(format!(
            "{}/v1/auth/callback",
            server.config.public_url.trim_end_matches('/')
        ))
        .map_err(|_| Error::new("invalid_request", "invalid public URL"))?,
    );
    let (challenge, verifier) = PkceCodeChallenge::new_random_sha256();
    let (authorization_url, csrf, nonce) = client
        .authorize_url(
            CoreAuthenticationFlow::AuthorizationCode,
            CsrfToken::new_random,
            Nonce::new_random,
        )
        .add_scope(Scope::new("profile".into()))
        .add_scope(Scope::new("email".into()))
        .set_pkce_challenge(challenge)
        .url();
    let now = chrono::Utc::now().timestamp();
    let mut logins = server.logins.lock().await;
    logins.retain(|_, login| login.created + 300 > now);
    if logins.len() >= 100 {
        return Err(Error::new("unavailable", "too many pending sign-ins").into());
    }
    logins.insert(
        csrf.secret().clone(),
        Login {
            created: now,
            metadata,
            verifier: verifier.secret().clone(),
            nonce: nonce.secret().clone(),
            native_challenge: request.challenge,
            callback: request.callback,
            native_state: request.state,
            grant: None,
            user: None,
        },
    );
    Ok(Json(
        json!({"authorization_url": authorization_url.to_string()}),
    ))
}

#[derive(Deserialize, utoipa::ToSchema)]
pub struct Callback {
    code: Option<String>,
    state: String,
    error: Option<String>,
}

#[utoipa::path(get,path="/v1/auth/callback",responses((status=303)))]
pub async fn callback(
    State(server): State<Arc<Server>>,
    Query(request): Query<Callback>,
) -> std::result::Result<Redirect, ApiError> {
    let mut login = server
        .logins
        .lock()
        .await
        .remove(&request.state)
        .ok_or_else(|| Error::new("unauthenticated", "login state is invalid or already used"))?;
    if login.created + 300 <= chrono::Utc::now().timestamp() || request.error.is_some() {
        return Err(
            Error::new("unauthenticated", "company sign-in expired or was declined").into(),
        );
    }
    let code = request
        .code
        .ok_or_else(|| Error::new("unauthenticated", "missing authorization code"))?;
    let client = CoreClient::from_provider_metadata(
        login.metadata.clone(),
        ClientId::new(server.config.client_id.clone()),
        server.config.client_secret.clone().map(ClientSecret::new),
    )
    .set_redirect_uri(
        RedirectUrl::new(format!(
            "{}/v1/auth/callback",
            server.config.public_url.trim_end_matches('/')
        ))
        .map_err(|_| Error::new("invalid_request", "invalid public URL"))?,
    );
    let http = openidconnect::reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .redirect(openidconnect::reqwest::redirect::Policy::none())
        .build()
        .map_err(|error| Error::new("unavailable", error.to_string()))?;
    let tokens = client
        .exchange_code(AuthorizationCode::new(code))
        .map_err(|error| Error::new("unauthenticated", error.to_string()))?
        .set_pkce_verifier(PkceCodeVerifier::new(login.verifier.clone()))
        .request_async(&http)
        .await
        .map_err(|_| Error::new("unauthenticated", "company token exchange failed"))?;
    let claims = tokens
        .id_token()
        .ok_or_else(|| Error::new("unauthenticated", "missing identity token"))?
        .claims(
            &client.id_token_verifier(),
            &Nonce::new(login.nonce.clone()),
        )
        .map_err(|_| Error::new("unauthenticated", "company identity could not be verified"))?;
    let email = claims
        .email()
        .map(|email| email.to_string())
        .unwrap_or_default();
    let name = claims
        .name()
        .and_then(|name| name.get(None))
        .map(|name| name.to_string())
        .unwrap_or_else(|| email.clone());
    let user = server
        .provision(
            &server.config.issuer,
            claims.subject().as_str(),
            &name,
            &email,
        )
        .await?;
    let grant = secret();
    let mut callback = url::Url::parse(&login.callback)
        .map_err(|_| Error::new("invalid_request", "invalid callback"))?;
    callback
        .query_pairs_mut()
        .append_pair("code", &grant)
        .append_pair("state", &login.native_state);
    login.grant = Some(grant.clone());
    login.user = Some(user);
    server.logins.lock().await.insert(grant, login);
    Ok(Redirect::to(callback.as_str()))
}

#[derive(Deserialize, utoipa::ToSchema)]
pub struct Exchange {
    pub code: String,
    pub verifier: String,
}

#[utoipa::path(post,path="/v1/auth/exchange",request_body=Exchange,responses((status=200,body=Credentials)))]
pub async fn exchange(
    State(server): State<Arc<Server>>,
    Json(request): Json<Exchange>,
) -> std::result::Result<Json<Credentials>, ApiError> {
    let login = server
        .logins
        .lock()
        .await
        .remove(&request.code)
        .ok_or_else(|| Error::new("unauthenticated", "login grant expired or was already used"))?;
    use sha2::{Digest, Sha256};
    let proof = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .encode(Sha256::digest(request.verifier.as_bytes()));
    if login.created + 300 <= chrono::Utc::now().timestamp()
        || proof != login.native_challenge
        || login.grant.as_deref() != Some(&request.code)
    {
        return Err(Error::new("unauthenticated", "invalid login proof").into());
    }
    let user = login
        .user
        .ok_or_else(|| Error::new("unauthenticated", "login was not completed"))?;
    Ok(Json(
        credentials(
            &server,
            user,
            chrono::Utc::now().timestamp() + 12 * 3600,
            None,
        )
        .await?,
    ))
}

#[derive(Deserialize, utoipa::ToSchema)]
pub struct Refresh {
    pub refresh_token: String,
}

#[utoipa::path(post,path="/v1/auth/refresh",request_body=Refresh,responses((status=200,body=Credentials)))]
pub async fn refresh(
    State(server): State<Arc<Server>>,
    Json(request): Json<Refresh>,
) -> std::result::Result<Json<Credentials>, ApiError> {
    let hash = digest(request.refresh_token.as_bytes());
    let registry = server.registry.read().await;
    let session = registry
        .sessions
        .get(&hash)
        .filter(|session| session.expires > chrono::Utc::now().timestamp())
        .ok_or_else(|| Error::new("unauthenticated", "sign in again"))?
        .clone();
    let user = registry
        .users
        .get(&session.user_id)
        .cloned()
        .ok_or_else(|| Error::new("unauthenticated", "user is unavailable"))?;
    drop(registry);
    Ok(Json(
        credentials(&server, user, session.expires, Some(&hash)).await?,
    ))
}

#[utoipa::path(post,path="/v1/auth/logout",responses((status=200,body=Value)))]
pub async fn logout(
    State(server): State<Arc<Server>>,
    headers: HeaderMap,
) -> std::result::Result<Json<Value>, ApiError> {
    authenticate(&server, &headers).await?;
    let hash = digest(
        headers["authorization"]
            .to_str()
            .unwrap()
            .trim_start_matches("Bearer ")
            .as_bytes(),
    );
    let mut registry = server.registry.write().await;
    let mut next = registry.clone();
    next.sessions
        .retain(|_, session| session.access_hash != hash);
    server.save_registry(&next).await?;
    *registry = next;
    Ok(Json(Value::Null))
}
