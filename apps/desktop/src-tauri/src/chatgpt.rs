use std::{
    collections::HashMap,
    path::PathBuf,
    process::Stdio,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::Duration,
};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tauri::{Manager, ipc::Channel};
use tauri_plugin_opener::OpenerExt;
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    process::{Child, ChildStdin, Command},
    sync::{Mutex as AsyncMutex, broadcast, oneshot, watch},
};

#[derive(Clone, Debug, Serialize, specta::Type)]
pub struct ChatgptError {
    pub code: String,
    pub message: String,
    pub retryable: bool,
}

fn failure(code: &str, message: &str) -> ChatgptError {
    ChatgptError {
        code: code.into(),
        message: message.into(),
        retryable: false,
    }
}

fn runtime_error() -> ChatgptError {
    failure(
        "runtime",
        "The ChatGPT runtime stopped responding. Try again, or restart Loofah.",
    )
}

fn provider_error(value: &Value) -> ChatgptError {
    let text = value.to_string().to_lowercase();
    if text.contains("keyring") || text.contains("keychain") {
        failure(
            "keychain",
            "macOS couldn't access your login Keychain. Unlock it and reconnect ChatGPT.",
        )
    } else if text.contains("401")
        || text.contains("unauthorized")
        || text.contains("auth")
        || text.contains("token_expired")
    {
        failure(
            "authentication",
            "Your ChatGPT session has expired. Reconnect in Intelligence settings.",
        )
    } else if text.contains("429")
        || text.contains("usage_limit")
        || text.contains("ratelimit")
        || text.contains("quota")
        || text.contains("rate limit")
    {
        failure(
            "quota",
            "Your account's Codex allowance is exhausted. Wait for it to reset, or check your ChatGPT plan.",
        )
    } else if text.contains("403") || text.contains("workspace") {
        failure(
            "access",
            "Your ChatGPT workspace does not allow this request. Check its Codex access settings.",
        )
    } else if text.contains("model") {
        failure(
            "model",
            "This model is unavailable for your account. Select another model in Intelligence settings.",
        )
    } else if text.contains("context") || text.contains("too large") {
        failure(
            "input",
            "This meeting exceeds the model's input limit. Select a model with a larger context window.",
        )
    } else {
        failure(
            "generation",
            "ChatGPT couldn't complete the request. Check your connection and try again.",
        )
    }
}

#[derive(Clone, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct ChatgptAccount {
    pub email: String,
    pub plan_type: String,
}

#[derive(Clone, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct ChatgptModel {
    pub model: String,
    pub display_name: String,
    pub is_default: bool,
    #[serde(default)]
    pub input_modalities: Vec<String>,
}

#[derive(Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct ChatgptGeneration {
    pub request_id: String,
    pub model: String,
    pub system: String,
    pub prompt: String,
    pub images: Vec<String>,
    pub max_output_tokens: Option<u32>,
}

#[derive(Clone, Serialize, specta::Type)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ChatgptEvent {
    Text {
        delta: String,
    },
    Complete {
        input_tokens: Option<u32>,
        output_tokens: Option<u32>,
    },
    Error {
        error: ChatgptError,
    },
}

#[derive(Default)]
pub struct ChatgptState {
    server: AsyncMutex<Option<Arc<Client>>>,
    login_lock: AsyncMutex<()>,
    login_cancel: Mutex<Option<watch::Sender<bool>>>,
    jobs: Mutex<HashMap<String, watch::Sender<bool>>>,
}

impl ChatgptState {
    async fn client<R: tauri::Runtime>(
        &self,
        app: &tauri::AppHandle<R>,
    ) -> Result<Arc<Client>, ChatgptError> {
        let mut slot = self.server.lock().await;
        if let Some(client) = slot.as_ref().filter(|c| c.alive.load(Ordering::SeqCst)) {
            return Ok(client.clone());
        }
        let resources = app.path().resource_dir().map_err(|_| runtime_error())?;
        let packaged = std::env::current_exe()
            .map_err(|_| runtime_error())?
            .with_file_name("loofah-codex");
        let development = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let (binary, catalog) = if packaged.is_file() {
            (packaged, resources.join("codex/models.json"))
        } else if cfg!(debug_assertions) {
            (
                development.join("binaries/loofah-codex-aarch64-apple-darwin"),
                development.join("resources/codex/models.json"),
            )
        } else {
            return Err(failure(
                "runtime_missing",
                "The ChatGPT runtime is missing. Reinstall Loofah.",
            ));
        };
        if !binary.is_file() || !catalog.is_file() {
            return Err(failure(
                "runtime_missing",
                "The ChatGPT runtime is missing from this build of Loofah.",
            ));
        }
        let home = app
            .path()
            .app_local_data_dir()
            .map_err(|_| runtime_error())?
            .join("chatgpt");
        let client = Client::start(binary, catalog, home).await?;
        *slot = Some(client.clone());
        Ok(client)
    }

    fn cancel_jobs(&self) {
        for cancel in self.jobs.lock().unwrap().values() {
            let _ = cancel.send(true);
        }
    }

    pub fn shutdown(&self) {
        self.cancel_jobs();
        if let Some(cancel) = self.login_cancel.lock().unwrap().as_ref() {
            let _ = cancel.send(true);
        }
        if let Ok(slot) = self.server.try_lock() {
            if let Some(client) = slot.as_ref() {
                client.stop();
            }
        }
    }
}

type Pending = Arc<Mutex<HashMap<u64, oneshot::Sender<Result<Value, ChatgptError>>>>>;

struct Client {
    child: Mutex<Child>,
    stdin: AsyncMutex<ChildStdin>,
    pending: Pending,
    notifications: broadcast::Sender<Value>,
    sequence: AtomicU64,
    alive: Arc<AtomicBool>,
    cwd: PathBuf,
    allowed_models: Vec<String>,
}

fn runtime_config(catalog: &std::path::Path) -> Vec<(String, Value)> {
    let mut config = vec![
        ("cli_auth_credentials_store".into(), json!("keyring")),
        ("forced_login_method".into(), json!("chatgpt")),
        ("model_provider".into(), json!("openai")),
        ("model_catalog_json".into(), json!(catalog)),
        ("mcp_servers".into(), json!({})),
        ("web_search".into(), json!("disabled")),
        ("history.persistence".into(), json!("none")),
        ("project_doc_max_bytes".into(), json!(0)),
        ("tools.update_plan.enabled".into(), json!(false)),
        (
            "tools.experimental_request_user_input.enabled".into(),
            json!(false),
        ),
        ("analytics.enabled".into(), json!(false)),
        ("feedback.enabled".into(), json!(false)),
        ("check_for_update_on_startup".into(), json!(false)),
    ];
    for feature in [
        "shell_tool",
        "unified_exec",
        "view_image",
        "apps",
        "plugins",
        "hooks",
        "codex_hooks",
        "plugin_hooks",
        "multi_agent",
        "multi_agent_v2",
        "code_mode",
        "code_mode_only",
        "code_mode_host",
        "browser",
        "browser_use",
        "computer_use",
        "image_generation",
        "imagegenext",
        "memories",
        "memory_tool",
        "shell_snapshot",
        "shell_snapshot_v2",
        "request_permissions_tool",
        "search_tool",
        "tool_search",
        "tool_suggest",
        "token_budget",
        "sleep_tool",
        "goals",
        "remote_control",
        "remote_models",
        "remote_plugin",
        "secret_auth_storage",
        "js_repl",
        "context_management",
    ] {
        config.push((format!("features.{feature}"), json!(false)));
    }
    config.push(("features.skip_host_skill_discovery".into(), json!(true)));
    config
}

impl Client {
    async fn start(
        binary: PathBuf,
        catalog: PathBuf,
        home: PathBuf,
    ) -> Result<Arc<Self>, ChatgptError> {
        let catalog_value: Value =
            serde_json::from_slice(&std::fs::read(&catalog).map_err(|_| runtime_error())?)
                .map_err(|_| runtime_error())?;
        let models = catalog_value["models"]
            .as_array()
            .ok_or_else(runtime_error)?;
        let mut allowed_models = Vec::new();
        for model in models {
            if !model["apply_patch_tool_type"].is_null()
                || model["shell_type"] != "disabled"
                || model["experimental_supported_tools"] != json!([])
                || model["tool_mode"] != "traditional"
            {
                return Err(failure(
                    "configuration",
                    "The bundled ChatGPT model configuration is invalid. Reinstall Loofah.",
                ));
            }
            allowed_models.push(
                model["slug"]
                    .as_str()
                    .ok_or_else(runtime_error)?
                    .to_string(),
            );
        }
        let cwd = home.join("work");
        std::fs::create_dir_all(&cwd).map_err(|_| runtime_error())?;
        let mut command = Command::new(binary);
        command.arg("app-server").arg("--stdio").env_clear();
        for key in [
            "HOME",
            "USER",
            "LOGNAME",
            "TMPDIR",
            "SSL_CERT_FILE",
            "CODEX_CA_CERTIFICATE",
            "HTTPS_PROXY",
            "HTTP_PROXY",
            "NO_PROXY",
        ] {
            if let Some(value) = std::env::var_os(key) {
                command.env(key, value);
            }
        }
        command
            .env("CODEX_HOME", &home)
            .env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin")
            .env("RUST_LOG", "off");
        for (key, value) in runtime_config(&catalog) {
            command.arg("-c").arg(format!("{key}={value}"));
        }
        let child = command
            .current_dir(&cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .map_err(|_| runtime_error())?;
        let client = Self::attach(child, cwd, allowed_models)?;
        client.request("initialize", json!({"clientInfo":{"name":"loofah","title":"Loofah","version":env!("CARGO_PKG_VERSION")}})).await?;
        client.write(json!({"method":"initialized"})).await?;
        let effective = client
            .request("config/read", json!({"includeLayers":false}))
            .await?;
        // Codex merges maps, so an empty override alone cannot remove inherited MCP servers.
        for (key, expected) in runtime_config(&catalog) {
            if key.starts_with("tools.") {
                continue;
            }
            let pointer = format!("/config/{}", key.replace('.', "/"));
            if effective.pointer(&pointer) != Some(&expected) {
                client.stop();
                return Err(failure(
                    "configuration",
                    "Your device configuration prevents isolated ChatGPT summaries.",
                ));
            }
        }
        Ok(client)
    }

    fn attach(
        mut child: Child,
        cwd: PathBuf,
        allowed_models: Vec<String>,
    ) -> Result<Arc<Self>, ChatgptError> {
        let stdin = child.stdin.take().ok_or_else(runtime_error)?;
        let stdout = child.stdout.take().ok_or_else(runtime_error)?;
        let pending: Pending = Arc::new(Mutex::new(HashMap::new()));
        let alive = Arc::new(AtomicBool::new(true));
        let (notifications, _) = broadcast::channel(1024);
        let client = Arc::new(Self {
            child: Mutex::new(child),
            stdin: AsyncMutex::new(stdin),
            pending: pending.clone(),
            notifications: notifications.clone(),
            sequence: AtomicU64::new(1),
            alive: alive.clone(),
            cwd,
            allowed_models,
        });
        let weak = Arc::downgrade(&client);
        tokio::spawn(async move {
            let mut lines = BufReader::new(stdout).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                let Ok(value) = serde_json::from_str::<Value>(&line) else {
                    break;
                };
                if value.get("method").is_some() && value.get("id").is_some() {
                    // No server-initiated tool or approval request is valid for summaries.
                    break;
                }
                if let Some(id) = value["id"].as_u64() {
                    if let Some(reply) = pending.lock().unwrap().remove(&id) {
                        let result = if let Some(error) = value.get("error") {
                            Err(provider_error(error))
                        } else {
                            Ok(value["result"].clone())
                        };
                        let _ = reply.send(result);
                    }
                } else {
                    let _ = notifications.send(value);
                }
            }
            alive.store(false, Ordering::SeqCst);
            for (_, reply) in pending.lock().unwrap().drain() {
                let _ = reply.send(Err(runtime_error()));
            }
            let _ = notifications.send(json!({"method":"loofah/runtime/stopped"}));
            if let Some(client) = weak.upgrade() {
                client.stop();
            }
        });
        Ok(client)
    }

    fn stop(&self) {
        self.alive.store(false, Ordering::SeqCst);
        let _ = self.child.lock().unwrap().start_kill();
    }

    async fn write(&self, value: Value) -> Result<(), ChatgptError> {
        let mut stdin = self.stdin.lock().await;
        let line = format!("{value}\n");
        stdin
            .write_all(line.as_bytes())
            .await
            .map_err(|_| runtime_error())?;
        stdin.flush().await.map_err(|_| runtime_error())
    }

    async fn request(&self, method: &str, params: Value) -> Result<Value, ChatgptError> {
        if !self.alive.load(Ordering::SeqCst) {
            return Err(runtime_error());
        }
        let id = self.sequence.fetch_add(1, Ordering::SeqCst);
        let (tx, rx) = oneshot::channel();
        self.pending.lock().unwrap().insert(id, tx);
        let operation = async {
            self.write(json!({"id":id,"method":method,"params":params}))
                .await?;
            rx.await.map_err(|_| runtime_error())?
        };
        let result = tokio::time::timeout(Duration::from_secs(30), operation).await;
        self.pending.lock().unwrap().remove(&id);
        match result {
            Ok(value) => value,
            Err(_) => {
                self.stop();
                Err(runtime_error())
            }
        }
    }

    async fn account(&self) -> Result<Option<ChatgptAccount>, ChatgptError> {
        let result = self
            .request("account/read", json!({"refreshToken":false}))
            .await?;
        if result["account"].is_null() {
            return Ok(None);
        }
        if result["account"]["type"] != "chatgpt" {
            return Err(failure(
                "authentication",
                "Connect a ChatGPT account in Intelligence settings.",
            ));
        }
        serde_json::from_value(result["account"].clone())
            .map(Some)
            .map_err(|_| runtime_error())
    }
}

#[tauri::command]
#[specta::specta]
pub async fn chatgpt_account<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
) -> Result<Option<ChatgptAccount>, ChatgptError> {
    app.state::<ChatgptState>()
        .client(&app)
        .await?
        .account()
        .await
}

#[tauri::command]
#[specta::specta]
pub async fn chatgpt_login<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
) -> Result<(), ChatgptError> {
    let state = app.state::<ChatgptState>();
    let _guard = state
        .login_lock
        .try_lock()
        .map_err(|_| failure("login_pending", "A ChatGPT sign-in is already in progress."))?;
    let (tx, mut cancel) = watch::channel(false);
    *state.login_cancel.lock().unwrap() = Some(tx);
    let result = async {
        let client = state.client(&app).await?;
        let mut events = client.notifications.subscribe();
        let login = client.request("account/login/start", json!({"type":"chatgpt"})).await?;
        let id = login["loginId"].as_str().ok_or_else(runtime_error)?;
        let result = async {
            if *cancel.borrow() { return Err(failure("cancelled", "Sign-in cancelled.")); }
            let url = login["authUrl"].as_str().ok_or_else(runtime_error)?;
            if !url.starts_with("https://auth.openai.com/") && !url.starts_with("https://chatgpt.com/") { return Err(runtime_error()); }
            app.opener().open_url(url, None::<&str>).map_err(|_| failure("browser", "Couldn't open your browser. Try signing in again."))?;
            loop {
                tokio::select! {
                    _ = cancel.changed() => return Err(failure("cancelled", "Sign-in cancelled.")),
                    event = events.recv() => {
                        let event = event.map_err(|_| runtime_error())?;
                        if event["method"] == "loofah/runtime/stopped" { return Err(runtime_error()); }
                        if event["method"] == "account/login/completed" && event["params"]["loginId"] == id {
                            return if event["params"]["success"] == true { Ok(()) } else { Err(provider_error(&event["params"]["error"])) };
                        }
                    }
                }
            }
        };
        let result = tokio::time::timeout(Duration::from_secs(300), result).await.unwrap_or_else(|_| Err(failure("login_timeout", "Sign-in timed out. Try again.")));
        if result.is_err() || *cancel.borrow() {
            let _ = client.request("account/login/cancel", json!({"loginId":id})).await;
            if *cancel.borrow() {
                client.request("account/logout", json!({})).await?;
                return Err(failure("cancelled", "Sign-in cancelled."));
            }
        }
        result
    }.await;
    state.login_cancel.lock().unwrap().take();
    result
}

#[tauri::command]
#[specta::specta]
pub fn chatgpt_cancel_login<R: tauri::Runtime>(app: tauri::AppHandle<R>) {
    if let Some(cancel) = app
        .state::<ChatgptState>()
        .login_cancel
        .lock()
        .unwrap()
        .as_ref()
    {
        let _ = cancel.send(true);
    }
}

#[tauri::command]
#[specta::specta]
pub async fn chatgpt_logout<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
) -> Result<(), ChatgptError> {
    chatgpt_cancel_login(app.clone());
    let state = app.state::<ChatgptState>();
    let _guard = state.login_lock.lock().await;
    state.cancel_jobs();
    let client = state.client(&app).await?;
    client.request("account/logout", json!({})).await?;
    client.stop();
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub async fn chatgpt_models<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
) -> Result<Vec<ChatgptModel>, ChatgptError> {
    let client = app.state::<ChatgptState>().client(&app).await?;
    if client.account().await?.is_none() {
        return Ok(vec![]);
    }
    let mut models = Vec::new();
    let mut cursor = Value::Null;
    loop {
        let result = client
            .request("model/list", json!({"cursor":cursor,"includeHidden":false}))
            .await?;
        let page: Vec<ChatgptModel> =
            serde_json::from_value(result["data"].clone()).map_err(|_| runtime_error())?;
        models.extend(
            page.into_iter()
                .filter(|m| client.allowed_models.contains(&m.model)),
        );
        cursor = result["nextCursor"].clone();
        if cursor.is_null() {
            break;
        }
    }
    models.sort_by_key(|m| !m.is_default);
    Ok(models)
}

#[tauri::command]
#[specta::specta]
pub async fn chatgpt_generate<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    request: ChatgptGeneration,
    events: Channel<ChatgptEvent>,
) -> Result<(), ChatgptError> {
    let state = app.state::<ChatgptState>();
    let client = state.client(&app).await?;
    if !client.allowed_models.contains(&request.model) {
        return Err(failure(
            "model",
            "Select an available ChatGPT model in Intelligence settings.",
        ));
    }
    if client.account().await?.is_none() {
        return Err(failure(
            "authentication",
            "Sign in to ChatGPT in Intelligence settings.",
        ));
    }
    if request.images.len() > 10
        || request
            .images
            .iter()
            .any(|image| !image.starts_with("data:image/") || image.len() > 2 * 1024 * 1024)
    {
        return Err(failure(
            "input",
            "ChatGPT received unsupported image input.",
        ));
    }
    let (tx, mut cancel) = watch::channel(false);
    {
        let mut jobs = state.jobs.lock().unwrap();
        if jobs.contains_key(&request.request_id) {
            return Err(failure("busy", "This request is already running."));
        }
        jobs.insert(request.request_id.clone(), tx);
    }
    tokio::spawn(async move {
        let result = run_generation(&client, &request, &events, &mut cancel).await;
        if let Err(error) = result {
            let _ = events.send(ChatgptEvent::Error { error });
        }
        app.state::<ChatgptState>()
            .jobs
            .lock()
            .unwrap()
            .remove(&request.request_id);
    });
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub fn chatgpt_cancel_generation<R: tauri::Runtime>(app: tauri::AppHandle<R>, request_id: String) {
    if let Some(cancel) = app
        .state::<ChatgptState>()
        .jobs
        .lock()
        .unwrap()
        .get(&request_id)
    {
        let _ = cancel.send(true);
    }
}

async fn run_generation(
    client: &Client,
    request: &ChatgptGeneration,
    events: &Channel<ChatgptEvent>,
    cancel: &mut watch::Receiver<bool>,
) -> Result<(), ChatgptError> {
    let mut notifications = client.notifications.subscribe();
    let instructions = format!(
        "{}\n\nReturn only the requested final text. Do not use tools or include progress commentary.{}",
        request.system,
        request
            .max_output_tokens
            .map(|n| format!(" Keep the response within approximately {n} tokens."))
            .unwrap_or_default()
    );
    let thread = client.request("thread/start", json!({"model":request.model,"cwd":client.cwd,"sandbox":"read-only","approvalPolicy":"never","ephemeral":true,"baseInstructions":instructions})).await?;
    let thread_id = thread["thread"]["id"].as_str().ok_or_else(runtime_error)?;
    let mut turn_id = None;
    let generation = async {
        if *cancel.borrow() {
            return Err(failure("cancelled", "Generation cancelled."));
        }
        let mut input = vec![json!({"type":"text","text":request.prompt,"text_elements":[]})];
        input.extend(
            request
                .images
                .iter()
                .map(|url| json!({"type":"image","url":url})),
        );
        let turn = client
            .request("turn/start", json!({"threadId":thread_id,"input":input}))
            .await?;
        turn_id = Some(
            turn["turn"]["id"]
                .as_str()
                .ok_or_else(runtime_error)?
                .to_string(),
        );
        let mut ignored_items = std::collections::HashSet::new();
        let mut has_text = false;
        let mut input_tokens = None;
        let mut output_tokens = None;
        loop {
            if *cancel.borrow() {
                return Err(failure("cancelled", "Generation cancelled."));
            }
            tokio::select! {
                _ = cancel.changed() => return Err(failure("cancelled", "Generation cancelled.")),
                event = notifications.recv() => {
                    let event = event.map_err(|_| runtime_error())?;
                    if event["method"] == "loofah/runtime/stopped" { return Err(runtime_error()); }
                    let params = &event["params"];
                    if params["threadId"] != thread_id { continue; }
                    match event["method"].as_str() {
                        Some("item/started") if params["item"]["type"] == "agentMessage" && params["item"]["phase"] == "commentary" => {
                            if let Some(id) = params["item"]["id"].as_str() { ignored_items.insert(id.to_string()); }
                        }
                        Some("item/agentMessage/delta") => {
                            if params["itemId"].as_str().is_some_and(|id| ignored_items.contains(id)) { continue; }
                            let delta = params["delta"].as_str().ok_or_else(runtime_error)?.to_string();
                            has_text |= !delta.trim().is_empty();
                            events.send(ChatgptEvent::Text { delta }).map_err(|_| failure("cancelled", "Generation cancelled."))?;
                        }
                        Some("thread/tokenUsage/updated") => {
                            input_tokens = params["tokenUsage"]["total"]["inputTokens"].as_u64().and_then(|n| n.try_into().ok());
                            output_tokens = params["tokenUsage"]["total"]["outputTokens"].as_u64().and_then(|n| n.try_into().ok());
                        }
                        Some("turn/completed") => {
                            if params["turn"]["status"] != "completed" { return Err(provider_error(&params["turn"]["error"])); }
                            if !has_text { return Err(failure("generation", "ChatGPT returned an empty response. Try again.")); }
                            events.send(ChatgptEvent::Complete { input_tokens, output_tokens }).map_err(|_| runtime_error())?;
                            return Ok(());
                        }
                        Some("error") if params["willRetry"] != true => return Err(provider_error(&params["error"])),
                        _ => {}
                    }
                }
            }
        }
    };
    let result = tokio::time::timeout(Duration::from_secs(600), generation)
        .await
        .unwrap_or_else(|_| {
            Err(failure(
                "timeout",
                "ChatGPT took too long to respond. Try again.",
            ))
        });
    if result.is_err() {
        if let Some(turn_id) = turn_id {
            let _ = client
                .request(
                    "turn/interrupt",
                    json!({"threadId":thread_id,"turnId":turn_id}),
                )
                .await;
        }
    }
    let _ = client
        .request("thread/unsubscribe", json!({"threadId":thread_id}))
        .await;
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(script: &str) -> Arc<Client> {
        let child = Command::new("/bin/sh")
            .arg("-c")
            .arg(script)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        Client::attach(child, PathBuf::from("/tmp"), vec![]).unwrap()
    }

    #[tokio::test]
    async fn correlates_out_of_order_responses() {
        let client = fixture(
            r#"read -r first
read -r second
printf '%s\n' '{"id":2,"result":"second"}' '{"id":1,"result":"first"}'
read -r hold"#,
        );
        let (first, second) = tokio::join!(
            client.request("first", json!({})),
            client.request("second", json!({}))
        );
        assert_eq!(first.unwrap(), "first");
        assert_eq!(second.unwrap(), "second");
        client.stop();
    }

    #[tokio::test]
    async fn unexpected_tool_request_stops_runtime_and_fails_pending_work() {
        let client = fixture(
            r#"read -r request
printf '%s\n' '{"id":99,"method":"item/commandExecution/requestApproval","params":{}}'
read -r hold"#,
        );
        let result = tokio::time::timeout(
            Duration::from_secs(2),
            client.request("turn/start", json!({})),
        )
        .await
        .unwrap();
        assert_eq!(result.unwrap_err().code, "runtime");
        assert!(!client.alive.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn process_exit_fails_request_without_waiting_for_timeout() {
        let client = fixture("read -r request; exit 0");
        let result = tokio::time::timeout(
            Duration::from_secs(2),
            client.request("account/read", json!({})),
        )
        .await
        .unwrap();
        assert_eq!(result.unwrap_err().code, "runtime");
    }

    fn generation() -> ChatgptGeneration {
        ChatgptGeneration {
            request_id: "request".into(),
            model: "model".into(),
            system: "Summarize the meeting.".into(),
            prompt: "Alice will send the checklist.".into(),
            images: vec![],
            max_output_tokens: None,
        }
    }

    #[tokio::test]
    async fn streams_final_text_and_usage_without_commentary() {
        let client = fixture(
            r#"read -r thread
printf '%s\n' '{"id":1,"result":{"thread":{"id":"thread"}}}'
read -r turn
printf '%s\n' '{"id":2,"result":{"turn":{"id":"turn"}}}' \
'{"method":"item/started","params":{"threadId":"thread","item":{"type":"agentMessage","phase":"commentary","id":"progress"}}}' \
'{"method":"item/agentMessage/delta","params":{"threadId":"thread","itemId":"progress","delta":"Working..."}}' \
'{"method":"item/agentMessage/delta","params":{"threadId":"thread","itemId":"final","delta":"Alice will send the checklist."}}' \
'{"method":"thread/tokenUsage/updated","params":{"threadId":"thread","tokenUsage":{"total":{"inputTokens":12,"outputTokens":7}}}}' \
'{"method":"turn/completed","params":{"threadId":"thread","turn":{"status":"completed"}}}'
read -r unsubscribe
printf '%s\n' '{"id":3,"result":{}}'
read -r hold"#,
        );
        let received = Arc::new(Mutex::new(Vec::<Value>::new()));
        let output = received.clone();
        let channel = Channel::new(move |body| {
            if let tauri::ipc::InvokeResponseBody::Json(body) = body {
                output
                    .lock()
                    .unwrap()
                    .push(serde_json::from_str(&body).unwrap());
            }
            Ok(())
        });
        let (_tx, mut cancel) = watch::channel(false);
        run_generation(&client, &generation(), &channel, &mut cancel)
            .await
            .unwrap();
        assert_eq!(
            *received.lock().unwrap(),
            vec![
                json!({"type":"text","delta":"Alice will send the checklist."}),
                json!({"type":"complete","input_tokens":12,"output_tokens":7}),
            ]
        );
        client.stop();
    }

    #[tokio::test]
    async fn cancellation_interrupts_the_active_turn() {
        let client = fixture(
            r#"read -r thread
printf '%s\n' '{"id":1,"result":{"thread":{"id":"thread"}}}'
read -r turn
printf '%s\n' '{"id":2,"result":{"turn":{"id":"turn"}}}' \
'{"method":"item/agentMessage/delta","params":{"threadId":"thread","itemId":"final","delta":"Started"}}'
read -r interrupt
case "$interrupt" in *turn/interrupt*) ;; *) exit 1 ;; esac
printf '%s\n' '{"id":3,"result":{}}'
read -r unsubscribe
case "$unsubscribe" in *thread/unsubscribe*) ;; *) exit 1 ;; esac
printf '%s\n' '{"id":4,"result":{}}'
read -r hold"#,
        );
        let (tx, mut cancel) = watch::channel(false);
        let channel = Channel::new(move |_| {
            tx.send(true).unwrap();
            Ok(())
        });
        let result = tokio::time::timeout(
            Duration::from_secs(2),
            run_generation(&client, &generation(), &channel, &mut cancel),
        )
        .await
        .unwrap();
        assert_eq!(result.unwrap_err().code, "cancelled");
        assert!(client.alive.load(Ordering::SeqCst));
        client.stop();
    }

    #[test]
    fn auth_and_quota_errors_do_not_leak_payloads_or_retry() {
        for (raw, expected) in [
            ("401 unauthorized sk-secret", "authentication"),
            ("429 quota sk-secret", "quota"),
            ("keyring access failed sk-secret", "keychain"),
        ] {
            let error = provider_error(&json!({"message":raw}));
            assert_eq!(error.code, expected);
            assert!(!error.retryable);
            assert!(!error.message.contains("sk-secret"));
        }
    }
}
