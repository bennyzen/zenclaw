use std::sync::Arc;
use axum::{
    extract::State,
    routing::{get, post},
    Json, Router,
};
use tower_http::cors::CorsLayer;

use crate::core::gateway::Gateway;

type AppState = Arc<Gateway>;

pub async fn start_api_server(gateway: Arc<Gateway>, port: u16) {
    let app = Router::new()
        .route("/api/status", get(api_status))
        .route("/api/chat", post(api_chat))
        .route("/api/chat/cancel", post(api_chat_cancel))
        .route("/api/config", get(api_config_get))
        .layer(CorsLayer::permissive())
        .with_state(gateway);

    let listener = tokio::net::TcpListener::bind(format!("0.0.0.0:{}", port))
        .await
        .expect("Failed to bind");

    tracing::info!(port, "API server listening");
    axum::serve(listener, app).await.expect("Server error");
}

async fn api_status(State(gw): State<AppState>) -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "status": "ok",
        "agent": gw.config.agent_name,
        "version": env!("CARGO_PKG_VERSION"),
        "platform": "rust",
        "tools": gw.tools.len(),
    }))
}

#[derive(serde::Deserialize)]
struct ChatRequest {
    message: String,
    #[serde(default = "default_chat_id")]
    chat_id: String,
}

fn default_chat_id() -> String {
    "default".to_string()
}

async fn api_chat(
    State(gw): State<AppState>,
    Json(req): Json<ChatRequest>,
) -> Json<serde_json::Value> {
    match gw.chat(&req.chat_id, &req.message, "api").await {
        Ok(response) => Json(serde_json::json!({
            "response": response,
            "chat_id": req.chat_id,
        })),
        Err(e) => {
            tracing::error!(error = %e, "Chat error");
            Json(serde_json::json!({
                "error": e.to_string(),
            }))
        }
    }
}

#[derive(serde::Deserialize)]
struct CancelRequest {
    #[serde(default = "default_chat_id")]
    chat_id: String,
}

async fn api_chat_cancel(
    State(gw): State<AppState>,
    Json(req): Json<CancelRequest>,
) -> Json<serde_json::Value> {
    let cancelled = gw.cancel_chat(&req.chat_id).await;
    Json(serde_json::json!({"cancelled": cancelled}))
}

async fn api_config_get(State(gw): State<AppState>) -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "agent_name": gw.config.agent_name,
        "default_provider": gw.config.providers.default,
    }))
}
