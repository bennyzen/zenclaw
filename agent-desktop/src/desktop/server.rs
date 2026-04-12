use std::sync::Arc;
use std::time::Instant;

use axum::{
    body::Bytes,
    extract::{
        ws::{Message as WsMsg, WebSocket, WebSocketUpgrade},
        Query, State,
    },
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post, put},
    Json, Router,
};
use serde::Deserialize;
use serde_json::json;
use tower_http::cors::CorsLayer;

use crate::core::gateway::Gateway;
use crate::core::sessions::SessionEntry;
use crate::core::types::Role;

// ---------------------------------------------------------------------------
// App state
// ---------------------------------------------------------------------------

#[derive(Clone)]
pub struct AppState {
    pub gateway: Arc<Gateway>,
    pub start_time: Instant,
    pub config_path: String,
}

// ---------------------------------------------------------------------------
// Server setup
// ---------------------------------------------------------------------------

pub async fn start_api_server(state: AppState, port: u16) {
    let app = Router::new()
        // Status
        .route("/api/status", get(api_status))
        .route("/api/restart", post(api_restart))
        // Chat
        .route("/api/chat", post(api_chat))
        .route("/api/chat/cancel", post(api_chat_cancel))
        .route("/api/chat/history", get(api_chat_history))
        // Config & WiFi
        .route("/api/config", get(api_config_get).put(api_config_put))
        .route("/api/wifi", get(api_wifi_get).put(api_wifi_put))
        // Files
        .route("/api/files", get(api_files_list).delete(api_files_delete))
        .route("/api/files/read", get(api_files_read))
        .route("/api/files/write", put(api_files_write))
        .route("/api/files/mkdir", post(api_files_mkdir))
        .route("/api/files/upload", post(api_files_upload))
        // WebSockets
        .route("/ws/chat", get(ws_chat))
        .route("/ws/stats", get(ws_stats))
        .route("/ws/logs", get(ws_logs))
        .layer(CorsLayer::permissive())
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(format!("0.0.0.0:{}", port))
        .await
        .expect("Failed to bind");

    tracing::info!(port, "API server listening");
    axum::serve(listener, app).await.expect("Server error");
}

// ---------------------------------------------------------------------------
// Status
// ---------------------------------------------------------------------------

async fn api_status(State(state): State<AppState>) -> Json<serde_json::Value> {
    let uptime = state.start_time.elapsed().as_secs();
    Json(json!({
        "agent_name": state.gateway.config.agent_name,
        "version": env!("CARGO_PKG_VERSION"),
        "built": "",
        "memory": { "free_kb": null, "used_kb": null, "total_kb": null },
        "temperature_c": null,
        "wifi": null,
        "storage": { "total_kb": null, "free_kb": null },
        "uptime_s": uptime
    }))
}

async fn api_restart() -> Response {
    (
        StatusCode::NOT_IMPLEMENTED,
        Json(json!({"error": "Restart not available on desktop"})),
    )
        .into_response()
}

// ---------------------------------------------------------------------------
// Chat
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct ChatRequest {
    message: String,
    #[serde(default = "default_web")]
    chat_id: String,
}

fn default_web() -> String {
    "web".to_string()
}

async fn api_chat(State(state): State<AppState>, Json(req): Json<ChatRequest>) -> Response {
    if req.message.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "JSON body with message required"})),
        )
            .into_response();
    }
    match state.gateway.chat(&req.chat_id, &req.message, "api").await {
        Ok(reply) => Json(json!({"reply": reply})).into_response(),
        Err(e) => {
            tracing::error!(error = %e, "Chat error");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": e.to_string()})),
            )
                .into_response()
        }
    }
}

#[derive(Deserialize)]
struct CancelRequest {
    #[serde(default = "default_web")]
    chat_id: String,
}

async fn api_chat_cancel(
    State(state): State<AppState>,
    Json(req): Json<CancelRequest>,
) -> Json<serde_json::Value> {
    let cancelled = state.gateway.cancel_chat(&req.chat_id).await;
    Json(json!({"cancelled": cancelled}))
}

#[derive(Deserialize)]
struct HistoryQuery {
    #[serde(default = "default_web")]
    chat_id: String,
    #[serde(default = "default_limit")]
    limit: usize,
}

fn default_limit() -> usize {
    50
}

async fn api_chat_history(
    State(state): State<AppState>,
    Query(q): Query<HistoryQuery>,
) -> Json<serde_json::Value> {
    let branch = match state.gateway.sessions.get_branch(&q.chat_id) {
        Ok(b) => b,
        Err(e) => {
            tracing::error!(error = %e, "Chat history error");
            return Json(json!({"messages": []}));
        }
    };

    let mut messages: Vec<serde_json::Value> = Vec::new();
    for entry in &branch {
        if let SessionEntry::Message { role, content, .. } = entry {
            let role_str = match role {
                Role::User => "user",
                Role::Assistant => "assistant",
                _ => continue,
            };
            if content.is_empty() {
                continue;
            }
            let content = if matches!(role, Role::User) {
                strip_envelope(content)
            } else {
                content.to_string()
            };
            messages.push(json!({"role": role_str, "content": content}));
        }
    }

    if messages.len() > q.limit {
        messages = messages.split_off(messages.len() - q.limit);
    }

    Json(json!({"messages": messages}))
}

/// Strip [channel timestamp] prefix from user messages.
fn strip_envelope(content: &str) -> String {
    if content.starts_with('[') {
        if let Some(pos) = content.find("] ") {
            return content[pos + 2..].to_string();
        }
    }
    content.to_string()
}

// ---------------------------------------------------------------------------
// Config
// ---------------------------------------------------------------------------

async fn api_config_get(State(state): State<AppState>) -> Response {
    match std::fs::read_to_string(&state.config_path) {
        Ok(data) => match serde_json::from_str::<serde_json::Value>(&data) {
            Ok(config) => Json(config).into_response(),
            Err(e) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": format!("Invalid config JSON: {}", e)})),
            )
                .into_response(),
        },
        Err(e) => (
            StatusCode::NOT_FOUND,
            Json(json!({"error": format!("Cannot read config: {}", e)})),
        )
            .into_response(),
    }
}

async fn api_config_put(
    State(state): State<AppState>,
    Json(body): Json<serde_json::Value>,
) -> Response {
    if body.get("providers").is_none() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "Config must contain a providers key"})),
        )
            .into_response();
    }
    match serde_json::to_string_pretty(&body) {
        Ok(data) => match std::fs::write(&state.config_path, data) {
            Ok(()) => {
                tracing::info!("Config updated via API");
                Json(json!({"ok": true})).into_response()
            }
            Err(e) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": format!("Cannot write config: {}", e)})),
            )
                .into_response(),
        },
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": format!("Cannot serialize config: {}", e)})),
        )
            .into_response(),
    }
}

// ---------------------------------------------------------------------------
// WiFi (desktop stubs)
// ---------------------------------------------------------------------------

async fn api_wifi_get() -> Json<serde_json::Value> {
    let hn = hostname::get()
        .ok()
        .and_then(|h| h.into_string().ok());
    Json(json!({
        "ssid": null,
        "connected": false,
        "ip": null,
        "rssi": null,
        "hostname": hn
    }))
}

async fn api_wifi_put() -> Response {
    (
        StatusCode::NOT_IMPLEMENTED,
        Json(json!({"error": "WiFi not available on desktop"})),
    )
        .into_response()
}

// ---------------------------------------------------------------------------
// Files
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct FilesListQuery {
    #[serde(default = "default_dot")]
    path: String,
}

fn default_dot() -> String {
    ".".to_string()
}

#[derive(Deserialize)]
struct FilePathQuery {
    #[serde(default)]
    path: String,
}

#[derive(Deserialize)]
struct WriteFileBody {
    path: String,
    content: String,
}

#[derive(Deserialize)]
struct MkdirBody {
    path: String,
}

async fn api_files_list(Query(q): Query<FilesListQuery>) -> Response {
    match std::fs::read_dir(&q.path) {
        Ok(rd) => {
            let mut entries: Vec<_> = rd.filter_map(|e| e.ok()).collect();
            entries.sort_by_key(|e| e.file_name());

            let result: Vec<serde_json::Value> = entries
                .iter()
                .map(|e| {
                    let name = e.file_name().to_string_lossy().to_string();
                    let full = if q.path == "." {
                        name.clone()
                    } else {
                        format!("{}/{}", q.path, name)
                    };
                    let meta = e.metadata();
                    let is_dir = meta.as_ref().map(|m| m.is_dir()).unwrap_or(false);
                    let size = if is_dir {
                        0u64
                    } else {
                        meta.as_ref().map(|m| m.len()).unwrap_or(0)
                    };
                    json!({"name": name, "path": full, "is_dir": is_dir, "size": size})
                })
                .collect();

            Json(json!({"path": q.path, "entries": result})).into_response()
        }
        Err(e) => (
            StatusCode::NOT_FOUND,
            Json(json!({"error": format!("Cannot list directory: {}", e)})),
        )
            .into_response(),
    }
}

async fn api_files_read(Query(q): Query<FilePathQuery>) -> Response {
    if q.path.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "path parameter required"})),
        )
            .into_response();
    }
    match std::fs::read_to_string(&q.path) {
        Ok(content) => Json(json!({"path": q.path, "content": content})).into_response(),
        Err(e) => (
            StatusCode::NOT_FOUND,
            Json(json!({"error": format!("Cannot read file: {}", e)})),
        )
            .into_response(),
    }
}

async fn api_files_write(Json(req): Json<WriteFileBody>) -> Response {
    if let Some(parent) = std::path::Path::new(&req.path).parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let size = req.content.len();
    match std::fs::write(&req.path, &req.content) {
        Ok(()) => {
            tracing::info!(path = %req.path, size, "File written");
            Json(json!({"path": req.path, "size": size})).into_response()
        }
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": format!("Cannot write file: {}", e)})),
        )
            .into_response(),
    }
}

async fn api_files_delete(Query(q): Query<FilePathQuery>) -> Response {
    if q.path.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "path parameter required"})),
        )
            .into_response();
    }
    let p = std::path::Path::new(&q.path);
    let result = if p.is_dir() {
        std::fs::remove_dir(&q.path)
    } else {
        std::fs::remove_file(&q.path)
    };
    match result {
        Ok(()) => {
            tracing::info!(path = %q.path, "Deleted");
            Json(json!({"deleted": q.path})).into_response()
        }
        Err(e) => (
            StatusCode::NOT_FOUND,
            Json(json!({"error": format!("Cannot delete: {}", e)})),
        )
            .into_response(),
    }
}

async fn api_files_mkdir(Json(req): Json<MkdirBody>) -> Response {
    match std::fs::create_dir_all(&req.path) {
        Ok(()) => {
            tracing::info!(path = %req.path, "Directory created");
            Json(json!({"path": req.path})).into_response()
        }
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": format!("Cannot create directory: {}", e)})),
        )
            .into_response(),
    }
}

const MAX_LOCAL_UPLOAD: usize = 256 * 1024;

async fn api_files_upload(Query(q): Query<FilePathQuery>, body: Bytes) -> Response {
    if q.path.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "path query parameter required"})),
        )
            .into_response();
    }
    let size = body.len();
    if size > MAX_LOCAL_UPLOAD {
        return (
            StatusCode::PAYLOAD_TOO_LARGE,
            Json(json!({
                "error": format!("File too large for device storage ({} bytes). Use cloud storage upload instead.", size),
                "max_size": MAX_LOCAL_UPLOAD
            })),
        )
            .into_response();
    }
    if let Some(parent) = std::path::Path::new(&q.path).parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    match std::fs::write(&q.path, &body) {
        Ok(()) => {
            tracing::info!(path = %q.path, size, "File uploaded");
            Json(json!({"path": q.path, "size": size})).into_response()
        }
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": format!("Cannot upload file: {}", e)})),
        )
            .into_response(),
    }
}

// ---------------------------------------------------------------------------
// WebSocket: Chat
// ---------------------------------------------------------------------------

async fn ws_chat(ws: WebSocketUpgrade, State(state): State<AppState>) -> Response {
    ws.on_upgrade(move |socket| handle_chat_ws(socket, state))
}

async fn handle_chat_ws(mut socket: WebSocket, state: AppState) {
    while let Some(Ok(msg)) = socket.recv().await {
        let text = match msg {
            WsMsg::Text(t) => t.to_string(),
            WsMsg::Close(_) => break,
            _ => continue,
        };

        let parsed: serde_json::Value = match serde_json::from_str(&text) {
            Ok(v) => v,
            Err(_) => continue,
        };

        let message = match parsed.get("message").and_then(|m| m.as_str()) {
            Some(m) if !m.is_empty() => m.to_string(),
            _ => {
                let err = json!({"type": "error", "error": "Empty message"}).to_string();
                let _ = socket.send(WsMsg::Text(err.into())).await;
                continue;
            }
        };

        let chat_id = parsed
            .get("chat_id")
            .and_then(|c| c.as_str())
            .unwrap_or("web")
            .to_string();

        // No streaming yet — run full chat and send done
        match state.gateway.chat(&chat_id, &message, "api").await {
            Ok(reply) => {
                let done = json!({"type": "done", "text": reply}).to_string();
                let _ = socket.send(WsMsg::Text(done.into())).await;
            }
            Err(e) => {
                let err = json!({"type": "error", "error": e.to_string()}).to_string();
                let _ = socket.send(WsMsg::Text(err.into())).await;
            }
        }
    }
}

// ---------------------------------------------------------------------------
// WebSocket: Stats (push every 3s)
// ---------------------------------------------------------------------------

async fn ws_stats(ws: WebSocketUpgrade, State(state): State<AppState>) -> Response {
    ws.on_upgrade(move |socket| handle_stats_ws(socket, state))
}

async fn handle_stats_ws(mut socket: WebSocket, state: AppState) {
    loop {
        let uptime = state.start_time.elapsed().as_secs();
        let stats = json!({
            "memory": {"free_kb": null, "used_kb": null, "total_kb": null},
            "temperature_c": null,
            "wifi": null,
            "storage": {"total_kb": null, "free_kb": null},
            "uptime_s": uptime
        });
        if socket
            .send(WsMsg::Text(stats.to_string().into()))
            .await
            .is_err()
        {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_secs(3)).await;
    }
}

// ---------------------------------------------------------------------------
// WebSocket: Logs (stub — keeps connection alive)
// ---------------------------------------------------------------------------

async fn ws_logs(ws: WebSocketUpgrade) -> Response {
    ws.on_upgrade(handle_logs_ws)
}

async fn handle_logs_ws(mut socket: WebSocket) {
    loop {
        tokio::time::sleep(std::time::Duration::from_secs(5)).await;
        if socket
            .send(WsMsg::Ping(vec![].into()))
            .await
            .is_err()
        {
            break;
        }
    }
}
