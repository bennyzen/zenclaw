use std::path::{Component, Path, PathBuf};
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

use crate::core::chat_events::ChatEvent;
use crate::core::gateway::Gateway;
use crate::core::sessions::SessionEntry;
use crate::core::types::Role;
use crate::desktop::MemStats;

// ---------------------------------------------------------------------------
// App state
// ---------------------------------------------------------------------------

#[derive(Clone)]
pub struct AppState {
    pub gateway: Arc<Gateway>,
    pub start_time: Instant,
    pub config_path: String,
    /// Canonical root for /api/files. Every file handler resolves user paths
    /// through `safe_join(&data_root, ...)` — without this, `std::fs` hands
    /// the caller the entire host filesystem (ESP32 gets containment for
    /// free via the partition mount; desktop does not).
    pub data_root: Arc<PathBuf>,
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
    Json(build_status_payload(&state))
}

/// Builds the JSON payload returned on `/api/status` and pushed every 5s
/// over `/ws/stats`. The two transports must serve the **same shape** —
/// the web client uses a single `setStatus` (full replace) handler
/// regardless of transport.
///
/// Desktop has fewer real datapoints than ESP32 (no temperature sensor,
/// no real wifi metrics, no on-flash storage info, no USB) — those fields are returned
/// as `null` so the JSON shape matches and the consumer doesn't need
/// per-platform branches.
///
/// See `docs/superpowers/specs/2026-05-03-stats-transport-model.md`.
fn build_status_payload(state: &AppState) -> serde_json::Value {
    let uptime = state.start_time.elapsed().as_secs();
    let providers = &state.gateway.config.providers;
    let model = providers
        .entries
        .get(&providers.default)
        .and_then(|e| e.model.as_deref())
        .unwrap_or("");
    json!({
        "agent_name": state.gateway.config.agent_name,
        "version": env!("CARGO_PKG_VERSION"),
        "built": "",
        "memory": memory_json(),
        "temperature_c": null,
        "wifi": null,
        "storage": { "total_kb": null, "free_kb": null },
        "provider": providers.default,
        "model": model,
        "uptime_s": uptime
    })
}

/// Shared shape for `/api/status` and `/ws/stats` memory fields. Maps Linux
/// process + system stats onto the field names the ESP32 build populates,
/// so consumers (web UI, harnesses) work against either platform unchanged.
/// `used_kb` is this process's RSS; `free_kb` is system MemAvailable.
fn memory_json() -> serde_json::Value {
    match MemStats::read() {
        Some(m) => json!({
            "free_kb": m.system_available_kb,
            "used_kb": m.rss_kb,
            "total_kb": m.system_total_kb,
            "rss_peak_kb": m.rss_peak_kb,
        }),
        None => json!({"free_kb": null, "used_kb": null, "total_kb": null}),
    }
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
            return Json(json!({"events": []}));
        }
    };

    let events = synthesize_history_events(&branch, &q.chat_id);

    let trimmed = if events.len() > q.limit {
        let start = events.len() - q.limit;
        events.into_iter().skip(start).collect::<Vec<_>>()
    } else {
        events
    };

    Json(json!({"events": trimmed}))
}

/// Replay a chat session as the same `ChatEvent` stream a live turn would
/// produce. Lossy on tool success/failure (the JSONL doesn't record an
/// explicit `ok` flag — historical tool finishes always emit `ok: true`).
/// Intermediate assistant prose attached to a tool-calls turn is not
/// surfaced, matching the live agent loop which only emits `AssistantText`
/// on the final-text branch.
fn synthesize_history_events(branch: &[SessionEntry], chat_id: &str) -> Vec<ChatEvent> {
    let mut out: Vec<ChatEvent> = Vec::new();

    for entry in branch {
        let SessionEntry::Message {
            role,
            content,
            tool_calls,
            tool_call_id,
            ..
        } = entry
        else {
            continue;
        };

        match role {
            Role::User => {
                if !content.is_empty() {
                    out.push(ChatEvent::UserMessage {
                        chat_id: chat_id.to_string(),
                        text: strip_envelope(content),
                    });
                }
            }
            Role::Assistant => {
                if let Some(calls) = tool_calls {
                    for tc in calls {
                        let args: serde_json::Value =
                            serde_json::from_str(&tc.function.arguments).unwrap_or(json!(null));
                        out.push(ChatEvent::ToolCallStarted {
                            id: tc.id.clone(),
                            name: tc.function.name.clone(),
                            args,
                        });
                    }
                } else if !content.is_empty() {
                    out.push(ChatEvent::AssistantText {
                        text: content.clone(),
                        is_final: true,
                    });
                }
            }
            Role::Tool => {
                if let Some(id) = tool_call_id {
                    out.push(ChatEvent::ToolCallFinished {
                        id: id.clone(),
                        ok: true,
                        result: Some(content.clone()),
                        error: None,
                    });
                }
            }
            Role::System => {}
        }
    }

    out
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

/// Resolve a user-supplied path against `root`, refusing anything that would
/// escape it. Two layers:
///   1. Lexical — strip leading slashes, walk components, reject `..` and
///      absolute prefixes. Catches direct traversal regardless of fs state.
///   2. Symlink — if the resolved path exists, canonicalize and verify the
///      result still starts with the canonical root. Catches symlink escapes
///      that lexical analysis can't see.
/// Empty / `.` resolve to `root` itself.
fn safe_join(root: &Path, user_input: &str) -> Result<PathBuf, String> {
    let stripped = user_input.trim_start_matches(|c| c == '/' || c == '\\');
    let mut joined = root.to_path_buf();
    for component in Path::new(stripped).components() {
        match component {
            Component::Normal(s) => joined.push(s),
            Component::CurDir => {}
            Component::ParentDir => {
                return Err("path escapes data root".into());
            }
            Component::RootDir | Component::Prefix(_) => {
                return Err("absolute paths not allowed".into());
            }
        }
    }
    if let Ok(canon) = joined.canonicalize() {
        let canon_root = root
            .canonicalize()
            .map_err(|e| format!("data root unavailable: {}", e))?;
        if !canon.starts_with(&canon_root) {
            return Err("path escapes data root via symlink".into());
        }
    }
    Ok(joined)
}

fn bad_path(msg: String) -> Response {
    (StatusCode::BAD_REQUEST, Json(json!({"error": msg}))).into_response()
}

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

async fn api_files_list(
    State(state): State<AppState>,
    Query(q): Query<FilesListQuery>,
) -> Response {
    let resolved = match safe_join(&state.data_root, &q.path) {
        Ok(p) => p,
        Err(e) => return bad_path(e),
    };
    match std::fs::read_dir(&resolved) {
        Ok(rd) => {
            let mut entries: Vec<_> = rd.filter_map(|e| e.ok()).collect();
            entries.sort_by_key(|e| e.file_name());

            let prefix = q.path.trim_matches(|c| c == '/' || c == '.');
            let result: Vec<serde_json::Value> = entries
                .iter()
                .map(|e| {
                    let name = e.file_name().to_string_lossy().to_string();
                    let full = if prefix.is_empty() {
                        name.clone()
                    } else {
                        format!("{}/{}", prefix, name)
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

async fn api_files_read(
    State(state): State<AppState>,
    Query(q): Query<FilePathQuery>,
) -> Response {
    if q.path.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "path parameter required"})),
        )
            .into_response();
    }
    let resolved = match safe_join(&state.data_root, &q.path) {
        Ok(p) => p,
        Err(e) => return bad_path(e),
    };
    match std::fs::read_to_string(&resolved) {
        Ok(content) => Json(json!({"path": q.path, "content": content})).into_response(),
        Err(e) => (
            StatusCode::NOT_FOUND,
            Json(json!({"error": format!("Cannot read file: {}", e)})),
        )
            .into_response(),
    }
}

async fn api_files_write(
    State(state): State<AppState>,
    Json(req): Json<WriteFileBody>,
) -> Response {
    let resolved = match safe_join(&state.data_root, &req.path) {
        Ok(p) => p,
        Err(e) => return bad_path(e),
    };
    if let Some(parent) = resolved.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let size = req.content.len();
    match std::fs::write(&resolved, &req.content) {
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

async fn api_files_delete(
    State(state): State<AppState>,
    Query(q): Query<FilePathQuery>,
) -> Response {
    if q.path.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "path parameter required"})),
        )
            .into_response();
    }
    let resolved = match safe_join(&state.data_root, &q.path) {
        Ok(p) => p,
        Err(e) => return bad_path(e),
    };
    if resolved == *state.data_root {
        return bad_path("refusing to delete data root".into());
    }
    let result = if resolved.is_dir() {
        std::fs::remove_dir(&resolved)
    } else {
        std::fs::remove_file(&resolved)
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

async fn api_files_mkdir(State(state): State<AppState>, Json(req): Json<MkdirBody>) -> Response {
    let resolved = match safe_join(&state.data_root, &req.path) {
        Ok(p) => p,
        Err(e) => return bad_path(e),
    };
    match std::fs::create_dir_all(&resolved) {
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

async fn api_files_upload(
    State(state): State<AppState>,
    Query(q): Query<FilePathQuery>,
    body: Bytes,
) -> Response {
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
    let resolved = match safe_join(&state.data_root, &q.path) {
        Ok(p) => p,
        Err(e) => return bad_path(e),
    };
    if let Some(parent) = resolved.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    match std::fs::write(&resolved, &body) {
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

async fn handle_chat_ws(socket: WebSocket, state: AppState) {
    use futures::{SinkExt, StreamExt};
    let (mut ws_tx, mut ws_rx) = socket.split();

    // One tokio channel for the connection's lifetime. Each turn spawns a
    // std::thread bridge that pumps the agent's std::sync::mpsc events into
    // a clone of `turn_async_tx`. The async receiver here multiplexes events
    // from any in-flight turn into outbound WS frames.
    let (turn_async_tx, mut turn_async_rx) =
        tokio::sync::mpsc::unbounded_channel::<ChatEvent>();

    loop {
        tokio::select! {
            biased;
            // Drain agent events first so back-pressure flows to the bridge.
            Some(evt) = turn_async_rx.recv() => {
                let json = match serde_json::to_string(&evt) {
                    Ok(s) => s,
                    Err(_) => continue,
                };
                if ws_tx.send(WsMsg::Text(json.into())).await.is_err() {
                    break;
                }
            }
            recv = ws_rx.next() => {
                let msg = match recv {
                    Some(Ok(m)) => m,
                    _ => break,
                };
                let text = match msg {
                    WsMsg::Text(t) => t.to_string(),
                    WsMsg::Close(_) => break,
                    _ => continue,
                };
                let evt: ChatEvent = match serde_json::from_str(&text) {
                    Ok(v) => v,
                    Err(_) => continue,
                };
                match evt {
                    ChatEvent::UserMessage { chat_id, text } => {
                        if text.is_empty() {
                            let err = ChatEvent::Error {
                                error: "Empty message".to_string(),
                            };
                            let json = serde_json::to_string(&err).unwrap();
                            let _ = ws_tx.send(WsMsg::Text(json.into())).await;
                            continue;
                        }
                        spawn_turn(state.gateway.clone(), chat_id, text, turn_async_tx.clone());
                    }
                    ChatEvent::Cancel { chat_id } => {
                        state.gateway.cancel_chat(&chat_id).await;
                    }
                    _ => { /* ignore — outbound-only events on inbound */ }
                }
            }
        }
    }
}

/// Run one chat turn: bridge the agent's std mpsc events to the connection's
/// async sender via a dedicated std::thread. The thread exits when the chat
/// task drops its sender (either via `Done`/`Error` emission and return, or
/// via task abort).
fn spawn_turn(
    gateway: Arc<Gateway>,
    chat_id: String,
    text: String,
    async_tx: tokio::sync::mpsc::UnboundedSender<ChatEvent>,
) {
    let (sync_tx, sync_rx) = std::sync::mpsc::channel::<ChatEvent>();

    std::thread::spawn(move || {
        while let Ok(evt) = sync_rx.recv() {
            if async_tx.send(evt).is_err() {
                break;
            }
        }
    });

    tokio::spawn(async move {
        let _ = gateway
            .chat_with_events(&chat_id, &text, "api", Some(&sync_tx))
            .await;
        // sync_tx drops here; bridge thread exits
    });
}

// ---------------------------------------------------------------------------
// WebSocket: Stats (push every 10s — symmetric with ESP32 build)
// ---------------------------------------------------------------------------

async fn ws_stats(ws: WebSocketUpgrade, State(state): State<AppState>) -> Response {
    ws.on_upgrade(move |socket| handle_stats_ws(socket, state))
}

async fn handle_stats_ws(mut socket: WebSocket, state: AppState) {
    loop {
        let payload = build_status_payload(&state);
        if socket
            .send(WsMsg::Text(payload.to_string().into()))
            .await
            .is_err()
        {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_secs(10)).await;
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

#[cfg(test)]
mod safe_join_tests {
    use super::safe_join;
    use std::path::PathBuf;

    fn root() -> PathBuf {
        PathBuf::from("/srv/data")
    }

    #[test]
    fn empty_resolves_to_root() {
        assert_eq!(safe_join(&root(), "").unwrap(), root());
    }

    #[test]
    fn dot_resolves_to_root() {
        assert_eq!(safe_join(&root(), ".").unwrap(), root());
    }

    #[test]
    fn relative_under_root_ok() {
        assert_eq!(
            safe_join(&root(), "sessions/web.jsonl").unwrap(),
            PathBuf::from("/srv/data/sessions/web.jsonl"),
        );
    }

    #[test]
    fn double_dot_rejected() {
        assert!(safe_join(&root(), "..").is_err());
        assert!(safe_join(&root(), "../etc/passwd").is_err());
        assert!(safe_join(&root(), "sessions/../../etc/passwd").is_err());
    }

    #[test]
    fn absolute_path_defanged_to_relative() {
        // Leading slash is stripped, so the request lands inside the root.
        // It will 404 if etc/passwd doesn't exist under the root, which is
        // the correct outcome — never the host's /etc/passwd.
        assert_eq!(
            safe_join(&root(), "/etc/passwd").unwrap(),
            PathBuf::from("/srv/data/etc/passwd"),
        );
    }

    #[test]
    fn dot_segments_collapse() {
        assert_eq!(
            safe_join(&root(), "./sessions/./web.jsonl").unwrap(),
            PathBuf::from("/srv/data/sessions/web.jsonl"),
        );
    }

    #[test]
    fn symlink_escape_rejected() {
        // Real-fs test: build a temp dir, drop a symlink that points outside,
        // and verify safe_join refuses to resolve through it.
        let tmp = std::env::temp_dir().join(format!("zenclaw-safejoin-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        let outside = std::env::temp_dir();
        #[cfg(unix)]
        std::os::unix::fs::symlink(&outside, tmp.join("escape")).unwrap();
        let res = safe_join(&tmp, "escape");
        assert!(res.is_err(), "expected symlink escape to be rejected, got {:?}", res);
        std::fs::remove_dir_all(&tmp).unwrap();
    }
}
