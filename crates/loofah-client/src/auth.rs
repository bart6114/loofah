use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use axum::{
    Router,
    extract::{Query, State},
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
    routing::get,
};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use hypr_loofah_protocol::{ErrorCode, RemoteAuthConfig};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use tokio::sync::{Mutex, oneshot};
use tokio_util::sync::CancellationToken;
use url::Url;

use crate::{
    RefreshCredential, RemoteClient, Result, cancelled, failure, invalid, json_body, public_json,
    unavailable, validate_endpoint,
};

#[derive(Default)]
pub(crate) struct TokenState {
    pub loaded: bool,
    pub token: Option<Token>,
    pub generation: u64,
}

impl TokenState {
    pub(crate) fn access_for_commit(&self) -> Option<&str> {
        self.current_for_commit()
            .then(|| self.token.as_ref().unwrap().access.as_str())
    }
    pub(crate) fn current_for_commit(&self) -> bool {
        self.loaded
            && self
                .token
                .as_ref()
                .is_some_and(|token| token.expires > Instant::now())
    }
}

pub(crate) struct Token {
    access: String,
    expires: Instant,
    refresh: Option<String>,
}

pub struct LoginAttempt {
    client: Arc<RemoteClient>,
    listener: tokio::net::TcpListener,
    authorization_url: Url,
    redirect_uri: String,
    verifier: String,
    state: String,
    cancel: CancellationToken,
}

#[derive(Deserialize)]
struct Discovery {
    issuer: String,
    authorization_endpoint: String,
    token_endpoint: String,
    code_challenge_methods_supported: Vec<String>,
}

pub(crate) async fn validate_config(
    http: &reqwest::Client,
    config: &RemoteAuthConfig,
    insecure: bool,
) -> Result<()> {
    for endpoint in [
        &config.issuer,
        &config.authorization_endpoint,
        &config.token_endpoint,
    ] {
        let url = Url::parse(endpoint).map_err(|_| invalid("Invalid identity endpoint"))?;
        validate_endpoint(&url, insecure)?;
        if url.query().is_some() {
            return Err(invalid(
                "Identity endpoints must not contain query parameters",
            ));
        }
    }
    if config.client_id.is_empty()
        || config.audience.is_empty()
        || config.client_id == config.audience
        || config.scopes.is_empty()
        || config.scopes.len() > 64
        || config.scopes.iter().any(|scope| {
            scope.is_empty() || scope.len() > 512 || scope.chars().any(char::is_whitespace)
        })
    {
        return Err(invalid("Invalid server identity configuration"));
    }
    let discovery: Discovery = public_json(
        http,
        format!(
            "{}/.well-known/openid-configuration",
            config.issuer.trim_end_matches('/')
        )
        .parse()
        .unwrap(),
    )
    .await?;
    if discovery.issuer != config.issuer
        || discovery.authorization_endpoint != config.authorization_endpoint
        || discovery.token_endpoint != config.token_endpoint
        || !discovery
            .code_challenge_methods_supported
            .iter()
            .any(|method| method == "S256")
    {
        return Err(invalid(
            "The server identity configuration does not match the issuer",
        ));
    }
    Ok(())
}

impl RemoteClient {
    pub async fn begin_login(self: &Arc<Self>) -> Result<LoginAttempt> {
        if self.stop.is_cancelled() {
            return Err(cancelled());
        }
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .map_err(|_| {
                failure(
                    ErrorCode::Unavailable,
                    "Cannot open the browser login callback",
                )
            })?;
        let port = listener.local_addr().map_err(|_| unavailable())?.port();
        let redirect_uri = format!("http://127.0.0.1:{port}/oauth/callback");
        let state = random_secret(2);
        let verifier = random_secret(3);
        let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
        let mut authorization_url = Url::parse(&self.auth.authorization_endpoint).unwrap();
        authorization_url.query_pairs_mut().extend_pairs([
            ("response_type", "code"),
            ("client_id", self.auth.client_id.as_str()),
            ("redirect_uri", &redirect_uri),
            ("scope", &self.auth.scopes.join(" ")),
            ("audience", &self.auth.audience),
            ("state", &state),
            ("code_challenge", &challenge),
            ("code_challenge_method", "S256"),
        ]);
        Ok(LoginAttempt {
            client: self.clone(),
            listener,
            authorization_url,
            redirect_uri,
            verifier,
            state,
            cancel: self.stop.child_token(),
        })
    }

    pub(crate) async fn access_token(self: &Arc<Self>) -> Result<String> {
        let client = self.clone();
        // Rotation and credential persistence continue if an individual request is cancelled.
        tokio::spawn(async move {
            let mut state = client.tokens.lock().await;
            if client.stop.is_cancelled() {
                return Err(cancelled());
            }
            if !state.loaded {
                if let Some(stored) = client.credentials.load(&client.account).await? {
                    let saved: RefreshCredential =
                        serde_json::from_str(&stored).map_err(|_| login_required())?;
                    if saved.schema != 1 || !valid_secret(&saved.refresh_token, 65_536) {
                        return Err(login_required());
                    }
                    state.token = Some(Token {
                        access: String::new(),
                        expires: Instant::now(),
                        refresh: Some(saved.refresh_token),
                    });
                }
                state.loaded = true;
            }
            let token = state.token.as_ref().ok_or_else(login_required)?;
            if token.expires.saturating_duration_since(Instant::now()) > Duration::from_secs(30) {
                return Ok(token.access.clone());
            }
            let refresh = token.refresh.clone().ok_or_else(login_required)?;
            let result = client
                .token_request(&[
                    ("grant_type", "refresh_token"),
                    ("refresh_token", &refresh),
                    ("client_id", &client.auth.client_id),
                ])
                .await;
            match result {
                Ok(reply) => {
                    let token = client.persist_token(reply, Some(refresh)).await?;
                    let access = token.access.clone();
                    state.token = Some(token);
                    Ok(access)
                }
                Err(error) => {
                    if error.code == ErrorCode::Unauthenticated {
                        state.generation =
                            state.generation.checked_add(1).ok_or_else(login_required)?;
                        state.token = None;
                        client.credentials.save(&client.account, None).await?;
                    }
                    Err(error)
                }
            }
        })
        .await
        .map_err(|_| failure(ErrorCode::Internal, "Authentication worker stopped"))?
    }

    pub(crate) async fn expire_access(&self, access: &str) {
        let mut state = self.tokens.lock().await;
        if let Some(token) = &mut state.token
            && token.access == access
        {
            token.expires = Instant::now();
        }
    }

    async fn token_request(&self, form: &[(&str, &str)]) -> Result<TokenReply> {
        let response = self
            .http
            .post(&self.auth.token_endpoint)
            .form(form)
            .timeout(Duration::from_secs(30))
            .send()
            .await
            .map_err(|_| unavailable())?;
        if matches!(response.status().as_u16(), 400 | 401 | 403) {
            return Err(login_required());
        }
        if !response.status().is_success() {
            return Err(unavailable());
        }
        let reply: TokenReply = json_body(response, 131_072).await?;
        if !reply.token_type.eq_ignore_ascii_case("bearer")
            || !valid_secret(&reply.access_token, 16_384)
            || !(1..=604_800).contains(&reply.expires_in)
            || reply
                .refresh_token
                .as_ref()
                .is_some_and(|token| !valid_secret(token, 65_536))
        {
            return Err(failure(
                ErrorCode::Unauthenticated,
                "The issuer returned invalid credentials",
            ));
        }
        Ok(reply)
    }

    async fn persist_token(&self, reply: TokenReply, fallback: Option<String>) -> Result<Token> {
        if self.stop.is_cancelled() {
            return Err(cancelled());
        }
        let refresh = reply.refresh_token.or(fallback);
        let saved = refresh.as_ref().map(|refresh| {
            serde_json::to_string(&RefreshCredential {
                schema: 1,
                refresh_token: refresh.clone(),
            })
            .unwrap()
        });
        self.credentials
            .save(&self.account, saved.as_deref())
            .await?;
        Ok(Token {
            access: reply.access_token,
            expires: Instant::now() + Duration::from_secs(reply.expires_in),
            refresh,
        })
    }
}

impl LoginAttempt {
    pub fn authorization_url(&self) -> &str {
        self.authorization_url.as_str()
    }
    pub fn cancellation(&self) -> CancellationToken {
        self.cancel.clone()
    }

    pub async fn complete(self) -> Result<()> {
        tokio::spawn(self.run())
            .await
            .map_err(|_| failure(ErrorCode::Internal, "Browser login worker stopped"))?
    }

    async fn run(self) -> Result<()> {
        let (sender, receiver) = oneshot::channel();
        let callback = Arc::new(CallbackState {
            expected_host: self
                .listener
                .local_addr()
                .map_err(|_| unavailable())?
                .to_string(),
            expected_state: self.state,
            expected_issuer: self.client.auth.issuer.clone(),
            sender: Mutex::new(Some(sender)),
        });
        let app = Router::new()
            .route("/oauth/callback", get(receive_callback))
            .with_state(callback);
        let server_stop = CancellationToken::new();
        let stop = server_stop.clone();
        let mut listener_task = tokio::spawn(async move {
            axum::serve(self.listener, app)
                .with_graceful_shutdown(stop.cancelled_owned())
                .await
        });
        let code = tokio::select! {
            _ = self.cancel.cancelled() => Err(cancelled()),
            _ = tokio::time::sleep(Duration::from_secs(180)) => Err(failure(ErrorCode::Cancelled, "Browser login timed out. Try again.")),
            result = receiver => result.map_err(|_| cancelled()).and_then(|result| result),
        };
        server_stop.cancel();
        if tokio::time::timeout(Duration::from_secs(1), &mut listener_task)
            .await
            .is_err()
        {
            listener_task.abort();
        }
        let code = code?;
        let mut state = self.client.tokens.lock().await;
        if self.cancel.is_cancelled() {
            return Err(cancelled());
        }
        state.generation = state.generation.checked_add(1).ok_or_else(login_required)?;
        let reply = self
            .client
            .token_request(&[
                ("grant_type", "authorization_code"),
                ("code", &code),
                ("client_id", &self.client.auth.client_id),
                ("redirect_uri", &self.redirect_uri),
                ("code_verifier", &self.verifier),
            ])
            .await?;
        if self.cancel.is_cancelled() {
            return Err(cancelled());
        }
        state.token = Some(self.client.persist_token(reply, None).await?);
        state.loaded = true;
        Ok(())
    }
}

#[derive(Deserialize)]
struct TokenReply {
    access_token: String,
    token_type: String,
    expires_in: u64,
    refresh_token: Option<String>,
}

struct CallbackState {
    expected_host: String,
    expected_state: String,
    expected_issuer: String,
    sender: Mutex<Option<oneshot::Sender<Result<String>>>>,
}

#[derive(Deserialize)]
struct Callback {
    state: String,
    code: Option<String>,
    error: Option<String>,
    iss: Option<String>,
}

async fn receive_callback(
    State(state): State<Arc<CallbackState>>,
    headers: HeaderMap,
    Query(query): Query<Callback>,
) -> impl IntoResponse {
    let host_ok =
        headers.get("host").and_then(|host| host.to_str().ok()) == Some(&state.expected_host);
    let valid = host_ok
        && query.state == state.expected_state
        && query
            .iss
            .as_ref()
            .is_none_or(|issuer| issuer == &state.expected_issuer)
        && (query.error.is_some() != query.code.is_some());
    let result = if valid {
        if query.error.is_some() {
            Err(failure(
                ErrorCode::Unauthenticated,
                "Browser login was declined",
            ))
        } else {
            query
                .code
                .filter(|code| valid_secret(code, 4096))
                .ok_or_else(|| invalid("Invalid authorization code"))
        }
    } else {
        Err(invalid("Invalid browser login callback"))
    };
    let status = if valid {
        if let Some(sender) = state.sender.lock().await.take() {
            let _ = sender.send(result);
            StatusCode::OK
        } else {
            StatusCode::GONE
        }
    } else {
        StatusCode::BAD_REQUEST
    };
    (
        status,
        [
            ("cache-control", "no-store"),
            ("content-security-policy", "default-src 'none'"),
            ("referrer-policy", "no-referrer"),
        ],
        if status == StatusCode::OK {
            "Login received. You can return to Loofah."
        } else {
            "This login callback is invalid or has expired."
        },
    )
}

fn random_secret(count: usize) -> String {
    (0..count)
        .map(|_| uuid::Uuid::new_v4().simple().to_string())
        .collect()
}
fn valid_secret(value: &str, max: usize) -> bool {
    !value.is_empty() && value.len() <= max && !value.chars().any(char::is_control)
}
pub(crate) fn login_required() -> hypr_loofah_protocol::ApplicationError {
    failure(
        ErrorCode::Unauthenticated,
        "Sign in to this server to continue",
    )
}
