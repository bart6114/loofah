use std::convert::Infallible;
use std::sync::Arc;

use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, Path, State},
    http::{HeaderMap, StatusCode},
    response::{
        IntoResponse, Response, Sse,
        sse::{Event, KeepAlive},
    },
    routing::{get, post, put},
};
use hypr_vault_access::{Call, Error, Role, ServerInfo, User};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::{Server, auth, content, transfers};

pub struct ApiError(pub Error);
impl From<Error> for ApiError {
    fn from(error: Error) -> Self {
        Self(error)
    }
}
impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let status = match self.0.code.as_str() {
            "unauthenticated" => StatusCode::UNAUTHORIZED,
            "forbidden" => StatusCode::FORBIDDEN,
            "not_found" => StatusCode::NOT_FOUND,
            "conflict" => StatusCode::PRECONDITION_FAILED,
            "precondition_required" => StatusCode::PRECONDITION_REQUIRED,
            "invalid_request" | "invalid_data" => StatusCode::BAD_REQUEST,
            "too_large" => StatusCode::PAYLOAD_TOO_LARGE,
            "range" => StatusCode::RANGE_NOT_SATISFIABLE,
            _ => StatusCode::SERVICE_UNAVAILABLE,
        };
        if status.is_server_error() {
            tracing::error!(code = %self.0.code, message = %self.0.message, "storage request failed");
        }
        (status, Json(self.0)).into_response()
    }
}

#[derive(utoipa::OpenApi)]
#[openapi(
    paths(
        info,
        me,
        users,
        vaults,
        create_vault,
        rename_vault,
        member,
        remove_member,
        invoke,
        operation,
        events,
        auth::start,
        auth::callback,
        auth::exchange,
        auth::refresh,
        auth::logout,
        transfers::begin,
        transfers::status,
        transfers::chunk,
        transfers::finish,
        transfers::cancel,
        transfers::list,
        transfers::ticket,
        transfers::media
    ),
    components(schemas(
        hypr_vault_access::Call,
        hypr_vault_access::Reply,
        hypr_vault_access::Error,
        hypr_vault_access::User,
        hypr_vault_access::Vault,
        hypr_vault_access::Role,
        hypr_vault_access::Upload,
        hypr_vault_access::Credentials
    ))
)]
struct ApiDoc;

async fn openapi() -> Json<utoipa::openapi::OpenApi> {
    use utoipa::OpenApi;
    Json(ApiDoc::openapi())
}

pub fn router(server: Arc<Server>) -> Router {
    Router::new()
        .route("/health", get(|| async { "ok" }))
        .route("/ready", get(ready))
        .route("/v1/server", get(info))
        .route("/v1/openapi.json", get(openapi))
        .route("/v1/auth/start", post(auth::start))
        .route("/v1/auth/callback", get(auth::callback))
        .route("/v1/auth/exchange", post(auth::exchange))
        .route("/v1/auth/refresh", post(auth::refresh))
        .route("/v1/auth/logout", post(auth::logout))
        .route("/v1/me", get(me))
        .route("/v1/users", get(users))
        .route("/v1/vaults", get(vaults).post(create_vault))
        .route("/v1/vaults/{vault}", put(rename_vault))
        .route(
            "/v1/vaults/{vault}/members/{member}",
            put(member).delete(remove_member),
        )
        .route("/v1/vaults/{vault}/invoke", post(invoke))
        .route("/v1/vaults/{vault}/operations/{operation}", get(operation))
        .route("/v1/vaults/{vault}/uploads", post(transfers::begin))
        .route(
            "/v1/vaults/{vault}/uploads/{upload}",
            get(transfers::status)
                .put(transfers::chunk)
                .delete(transfers::cancel),
        )
        .route(
            "/v1/vaults/{vault}/uploads/{upload}/finish",
            post(transfers::finish),
        )
        .route(
            "/v1/vaults/{vault}/sessions/{session}/artifacts",
            get(transfers::list),
        )
        .route(
            "/v1/vaults/{vault}/sessions/{session}/media",
            post(transfers::ticket),
        )
        .route("/v1/media/{ticket}", get(transfers::media))
        .route("/v1/events", get(events))
        .layer(DefaultBodyLimit::max(16 * 1024 * 1024))
        .layer(
            tower_http::set_header::SetResponseHeaderLayer::if_not_present(
                axum::http::header::CACHE_CONTROL,
                axum::http::HeaderValue::from_static("no-store"),
            ),
        )
        .layer(
            tower_http::set_header::SetResponseHeaderLayer::if_not_present(
                axum::http::header::REFERRER_POLICY,
                axum::http::HeaderValue::from_static("no-referrer"),
            ),
        )
        .with_state(server)
}

#[utoipa::path(get,path="/v1/server",responses((status=200,body=hypr_vault_access::ServerInfo)))]
async fn info(State(server): State<Arc<Server>>) -> Json<ServerInfo> {
    Json(ServerInfo {
        id: server.registry.read().await.server_id.clone(),
        api_version: 1,
        authentication: "oidc".into(),
    })
}

async fn ready(State(server): State<Arc<Server>>) -> std::result::Result<Json<Value>, ApiError> {
    let path = server.config.data_dir.join("service");
    let writable = tokio::task::spawn_blocking(move || tempfile::tempfile_in(path))
        .await
        .map_err(|error| Error::new("storage", error.to_string()))?
        .map_err(Error::from)?;
    drop(writable);
    Ok(Json(json!({"ready":true})))
}

#[utoipa::path(get,path="/v1/me",responses((status=200,body=hypr_vault_access::User)))]
async fn me(
    State(server): State<Arc<Server>>,
    headers: HeaderMap,
) -> std::result::Result<Json<User>, ApiError> {
    Ok(Json(auth::authenticate(&server, &headers).await?))
}

#[utoipa::path(get,path="/v1/users",responses((status=200,body=Value)))]
async fn users(
    State(server): State<Arc<Server>>,
    headers: HeaderMap,
) -> std::result::Result<Json<Value>, ApiError> {
    auth::authenticate(&server, &headers).await?;
    let registry = server.registry.read().await;
    Ok(Json(json!(
        registry
            .users
            .values()
            .filter(|user| !user.disabled)
            .map(|user| json!({"id": user.id,"name":user.name,"email":user.email}))
            .collect::<Vec<_>>()
    )))
}

#[utoipa::path(get,path="/v1/vaults",responses((status=200,body=Vec<hypr_vault_access::Vault>)))]
async fn vaults(
    State(server): State<Arc<Server>>,
    headers: HeaderMap,
) -> std::result::Result<Json<Value>, ApiError> {
    let user = auth::authenticate(&server, &headers).await?;
    Ok(Json(json!(
        server
            .registry
            .read()
            .await
            .vaults
            .values()
            .filter(|vault| vault.members.contains_key(&user.id))
            .collect::<Vec<_>>()
    )))
}

#[derive(Deserialize)]
struct Name {
    name: String,
}

#[utoipa::path(post,path="/v1/vaults",request_body=Value,responses((status=200,body=hypr_vault_access::Vault)))]
async fn create_vault(
    State(server): State<Arc<Server>>,
    headers: HeaderMap,
    Json(request): Json<Name>,
) -> std::result::Result<Json<Value>, ApiError> {
    let user = auth::authenticate(&server, &headers).await?;
    Ok(Json(json!(
        server.create_vault(&user, &request.name).await?
    )))
}

#[utoipa::path(put,path="/v1/vaults/{vault}",request_body=Value,params(("vault"=String,Path)),responses((status=200,body=hypr_vault_access::Vault)))]
async fn rename_vault(
    State(server): State<Arc<Server>>,
    Path(vault_id): Path<String>,
    headers: HeaderMap,
    Json(request): Json<Name>,
) -> std::result::Result<Json<Value>, ApiError> {
    let user = auth::authenticate(&server, &headers).await?;
    let runtime = server.runtime(&vault_id).await?;
    let _gate = runtime.gate.write().await;
    server.authorize(&user, &vault_id, false, true).await?;
    if request.name.trim().is_empty() || request.name.len() > 255 {
        return Err(Error::new("invalid_request", "invalid vault name").into());
    }
    let mut registry = server.registry.write().await;
    let mut next = registry.clone();
    next.vaults.get_mut(&vault_id).unwrap().name = request.name.trim().into();
    server.save_registry(&next).await?;
    *registry = next;
    let vault = registry.vaults[&vault_id].clone();
    drop(registry);
    server.notify(Some(vault_id), "vaults", vec![]);
    Ok(Json(json!(vault)))
}

#[derive(Deserialize)]
struct Membership {
    role: Role,
}

#[utoipa::path(put,path="/v1/vaults/{vault}/members/{member}",request_body=Value,params(("vault"=String,Path),("member"=String,Path)),responses((status=200,body=hypr_vault_access::Vault)))]
async fn member(
    State(server): State<Arc<Server>>,
    Path((vault, member)): Path<(String, String)>,
    headers: HeaderMap,
    Json(request): Json<Membership>,
) -> std::result::Result<Json<Value>, ApiError> {
    let user = auth::authenticate(&server, &headers).await?;
    Ok(Json(json!(
        server
            .update_member(&user, &vault, &member, Some(request.role))
            .await?
    )))
}

#[utoipa::path(delete,path="/v1/vaults/{vault}/members/{member}",params(("vault"=String,Path),("member"=String,Path)),responses((status=200,body=hypr_vault_access::Vault)))]
async fn remove_member(
    State(server): State<Arc<Server>>,
    Path((vault, member)): Path<(String, String)>,
    headers: HeaderMap,
) -> std::result::Result<Json<Value>, ApiError> {
    let user = auth::authenticate(&server, &headers).await?;
    Ok(Json(json!(
        server.update_member(&user, &vault, &member, None).await?
    )))
}

#[utoipa::path(post,path="/v1/vaults/{vault}/invoke",request_body=hypr_vault_access::Call,params(("vault"=String,Path)),responses((status=200,body=hypr_vault_access::Reply),(status=412,body=hypr_vault_access::Error)))]
async fn invoke(
    State(server): State<Arc<Server>>,
    Path(vault): Path<String>,
    headers: HeaderMap,
    Json(call): Json<Call>,
) -> std::result::Result<Json<hypr_vault_access::Reply>, ApiError> {
    let user = auth::authenticate(&server, &headers).await?;
    Ok(Json(content::invoke(server, &user, &vault, call).await?))
}

#[utoipa::path(get,path="/v1/vaults/{vault}/operations/{operation}",params(("vault"=String,Path),("operation"=String,Path)),responses((status=200,body=Value)))]
async fn operation(
    State(server): State<Arc<Server>>,
    Path((vault, operation)): Path<(String, String)>,
    headers: HeaderMap,
) -> std::result::Result<Json<Value>, ApiError> {
    let user = auth::authenticate(&server, &headers).await?;
    server.authorize(&user, &vault, false, false).await?;
    hypr_vault_access::validate_id(&operation)?;
    let runtime = server.runtime(&vault).await?;
    let _gate = runtime.gate.write().await;
    server.authorize(&user, &vault, false, false).await?;
    content::recover(&runtime).await?;
    Ok(Json(json!(
        runtime
            .storage
            .operation(&format!("{}_{operation}", user.id), None)?
    )))
}

#[utoipa::path(get,path="/v1/events",responses((status=200,content_type="text/event-stream",body=String)))]
async fn events(
    State(server): State<Arc<Server>>,
    headers: HeaderMap,
) -> std::result::Result<
    Sse<impl futures_util::Stream<Item = std::result::Result<Event, Infallible>>>,
    ApiError,
> {
    let user = auth::authenticate(&server, &headers).await?;
    let headers = headers.clone();
    let mut receiver = server.events.subscribe();
    let mut stopping = server.stopping.subscribe();
    let stream = async_stream::stream! {
        yield Ok(Event::default().event("reset").data("{}"));
        loop {
            let message = tokio::select! {
                _=stopping.changed()=>break,
                message = receiver.recv() => message,
                _ = tokio::time::sleep(std::time::Duration::from_secs(15)) => {
                    if auth::authenticate(&server, &headers).await.is_err() { yield Ok(Event::default().event("unauthenticated").data("{}")); break; }
                    continue;
                }
            };
            if auth::authenticate(&server, &headers).await.is_err() { yield Ok(Event::default().event("unauthenticated").data("{}")); break; }
            match message {
                Ok(event) => {
                    let permitted = match &event.vault_id { Some(vault) => server.authorize(&user, vault, false, false).await.is_ok(), None => true };
                    if permitted { yield Ok(Event::default().id(&event.cursor).event("change").data(serde_json::to_string(&event).unwrap())); }
                    else if event.entity == "members" && event.ids.contains(&user.id) { yield Ok(Event::default().event("revoked").data("{}")); }
                },
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => { yield Ok(Event::default().event("reset").data("{}")); },
                Err(_) => break,
            }
        }
    };
    Ok(Sse::new(stream).keep_alive(KeepAlive::default()))
}
