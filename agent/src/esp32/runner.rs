use std::sync::Arc;

use async_trait::async_trait;
use log::info;

use crate::config::Config;
use crate::core::runner::{LlmRunner, RunnerError};
use crate::core::types::{
    FunctionCall, LlmResponse, Message, MessageContent, Role, ToolCall, ToolDefinition,
};

/// LLM runner for ESP32 — speaks ONE wire format (OpenAI-compatible) for every
/// provider. Gemini, OpenAI, zAI, Anthropic, etc. all expose `/chat/completions`.
/// Per-provider quirks (e.g. Gemini's `extra_content.google.thought_signature`)
/// are carried opaquely on `ToolCall.extra_content` and round-tripped verbatim.
pub struct EspRunner {
    config: Arc<Config>,
}

impl EspRunner {
    pub fn new(config: Arc<Config>) -> Self {
        Self { config }
    }

    fn resolve_model(&self, model_override: Option<&str>) -> String {
        model_override
            .map(|s| s.to_string())
            .or_else(|| {
                self.config
                    .providers
                    .entries
                    .get(&self.config.providers.default)
                    .and_then(|e| e.model.clone())
            })
            .unwrap_or_else(|| "gemini-2.5-flash".to_string())
    }

    fn call_sync(
        &self,
        messages: &[Message],
        tools: &[ToolDefinition],
        model_override: Option<&str>,
    ) -> Result<LlmResponse, RunnerError> {
        let provider = self
            .config
            .providers
            .entries
            .get(&self.config.providers.default)
            .ok_or(RunnerError::NoApiKey)?;

        let api_key = provider
            .api_key
            .as_deref()
            .ok_or(RunnerError::NoApiKey)?;

        let model = self.resolve_model(model_override);
        let base_url = provider
            .base_url
            .as_deref()
            .unwrap_or("https://generativelanguage.googleapis.com/v1beta/openai");
        let base_url = normalize_base_url(base_url);

        let url = format!("{}/chat/completions", base_url);
        let payload = build_openai_payload(messages, tools, &model);
        let auth_header = format!("Bearer {}", api_key);

        let body = serde_json::to_string(&payload)
            .map_err(|e| RunnerError::Parse(e.to_string()))?;
        drop(payload); // Free JSON Value before opening TLS connection

        info!("LLM call: model={} body={}B", model, body.len());

        let (status, response_body) = esp_http_post(&url, &body, Some(&auth_header))
            .map_err(RunnerError::Network)?;
        drop(body); // Free request body before parsing response

        // A non-2xx status carries a real provider error — surface its code and
        // message (issue #14) instead of burying it as a generic failure. This
        // also classifies it as fatal vs. transient so the agent loop retries
        // only the transient ones.
        if status >= 400 {
            return Err(crate::core::runner::classify_http_error(status, &response_body));
        }

        let response: serde_json::Value = serde_json::from_str(&response_body)
            .map_err(|e| RunnerError::Parse(format!("JSON parse: {}", e)))?;

        // Some providers return 200 with an error envelope in the body.
        if let Some(error) = response.get("error") {
            let msg = error
                .get("message")
                .and_then(|m| m.as_str())
                .unwrap_or("Unknown API error");
            if msg.to_lowercase().contains("api key") || msg.contains("401") {
                return Err(RunnerError::Auth(msg.to_string()));
            }
            if msg.contains("429") || msg.to_lowercase().contains("rate") {
                return Err(RunnerError::RateLimit);
            }
            return Err(RunnerError::Api(msg.to_string()));
        }

        parse_openai_response(&response)
    }
}

/// Append `/openai` to Gemini's native `…/v1beta` base_url so it routes through
/// Google's OpenAI-compatibility endpoint. Existing user configs (which point at
/// the native endpoint) keep working without manual migration. Other providers
/// pass through untouched.
fn normalize_base_url(raw: &str) -> String {
    let trimmed = raw.trim_end_matches('/');
    if trimmed.contains("generativelanguage.googleapis.com") && !trimmed.contains("/openai") {
        format!("{}/openai", trimmed)
    } else {
        trimmed.to_string()
    }
}

#[async_trait]
impl LlmRunner for EspRunner {
    async fn call(
        &self,
        messages: &[Message],
        tools: &[ToolDefinition],
        model_override: Option<&str>,
    ) -> Result<LlmResponse, RunnerError> {
        // EspHttpConnection is blocking; call synchronously.
        // On ESP32 single-threaded tokio, this blocks the runtime which is fine
        // since we process one chat at a time.
        self.call_sync(messages, tools, model_override)
    }
}

// ---------------------------------------------------------------------------
// HTTP POST via esp-idf-svc (blocking, uses mbedtls for TLS)
// ---------------------------------------------------------------------------

/// Max attempts for one logical POST. z.ai's edge resets a large fraction of
/// *new* TLS handshakes (issue #14); a fresh handshake per call rolls the dice
/// every time. We (a) keep a warm connection so most calls skip the handshake
/// entirely, and (b) retry the handshake/write resets that do happen, with
/// exponential backoff, before they ever reach the agent-loop circuit breaker.
const MAX_HTTP_ATTEMPTS: usize = 4;
const INITIAL_BACKOFF_MS: u64 = 200;

thread_local! {
    /// Warm HTTP/TLS connection to the LLM provider, reused across calls on
    /// this thread — including the multiple round-trips within a single
    /// tool-using turn (issue #10's dominant failure mode). A kept-alive
    /// connection only handshakes once; subsequent requests reuse the socket
    /// (`esp_http_client_open` reuses it when keep-alive is on). Reset or dead
    /// connections are dropped and rebuilt by `esp_http_post`'s retry loop.
    ///
    /// Thread-local (not a global) so we never move the `!Send` connection
    /// handle across threads and never need `unsafe`. mbedTLS buffers live in
    /// PSRAM (`CONFIG_MBEDTLS_EXTERNAL_MEM_ALLOC`) and the idle footprint is
    /// freed (`CONFIG_MBEDTLS_DYNAMIC_BUFFER`), so a few warm connections (one
    /// per calling thread) are cheap.
    static PROVIDER_CONN: std::cell::RefCell<Option<esp_idf_svc::http::client::EspHttpConnection>> =
        const { std::cell::RefCell::new(None) };
}

fn build_provider_conn() -> Result<esp_idf_svc::http::client::EspHttpConnection, String> {
    use esp_idf_svc::http::client::{Configuration as HttpConfig, EspHttpConnection};
    let config = HttpConfig {
        buffer_size: Some(1024),
        buffer_size_tx: Some(1024),
        timeout: Some(std::time::Duration::from_secs(60)),
        crt_bundle_attach: Some(esp_idf_svc::sys::esp_crt_bundle_attach),
        // Reuse the socket across requests — the whole point of the fix.
        keep_alive_enable: true,
        ..Default::default()
    };
    EspHttpConnection::new(&config).map_err(|e| format!("HTTP init: {}", e))
}

/// Perform one request over the given (possibly warm) connection. Any error
/// here is treated as a transient transport failure: the caller drops the
/// connection and reconnects on the next attempt. Returns `(status, body)` for
/// *any* HTTP status — non-2xx is a real provider response, not a transport
/// failure, so the caller (not this function) decides how to classify it.
fn http_post_once(
    conn: &mut esp_idf_svc::http::client::EspHttpConnection,
    url: &str,
    body: &str,
    auth_header: Option<&str>,
) -> Result<(u16, String), String> {
    use esp_idf_svc::http::Method;

    let content_len = body.len().to_string();
    let mut headers: Vec<(&str, &str)> = vec![
        ("Content-Type", "application/json"),
        ("Content-Length", &content_len),
    ];
    if let Some(auth) = auth_header {
        headers.push(("Authorization", auth));
    }

    conn.initiate_request(Method::Post, url, &headers)
        .map_err(|e| format!("HTTP request: {}", e))?;

    conn.write_all(body.as_bytes())
        .map_err(|e| format!("HTTP write: {}", e))?;

    conn.initiate_response()
        .map_err(|e| format!("HTTP response: {}", e))?;

    let status = conn.status();

    let mut buf = [0u8; 1024];
    let mut resp_body = Vec::new();
    loop {
        let n = conn.read(&mut buf).map_err(|e| format!("HTTP read: {}", e))?;
        if n == 0 {
            break;
        }
        resp_body.extend_from_slice(&buf[..n]);
    }

    let body_str = String::from_utf8(resp_body).map_err(|e| format!("Invalid UTF-8: {}", e))?;
    Ok((status, body_str))
}

/// POST to the provider, returning `(status, body)` for any completed HTTP
/// response. Reuses a warm thread-local connection and retries transient
/// TLS/connection failures with exponential backoff. Only `Err` when every
/// attempt fails at the transport level (no HTTP response at all) — those are
/// the resets z.ai sheds, surfaced as a retryable `Network` error upstream.
fn esp_http_post(url: &str, body: &str, auth_header: Option<&str>) -> Result<(u16, String), String> {
    let mut backoff_ms = INITIAL_BACKOFF_MS;
    let mut last_err = String::new();

    for attempt in 1..=MAX_HTTP_ATTEMPTS {
        // Take the warm connection if we have one, else build a fresh one. A
        // failure to even allocate the client is not transient — bail.
        let conn = PROVIDER_CONN.with(|c| c.borrow_mut().take());
        let mut conn = match conn {
            Some(c) => c,
            None => build_provider_conn()?,
        };

        // Serialize the actual TLS I/O device-wide: mbedTLS shares global state
        // during operations, so the LLM call must be mutually exclusive with
        // the web tool's TLS calls (which also take this lock). Released during
        // the backoff sleep below so other TLS work isn't blocked while we wait.
        let result = {
            let _tls_guard = crate::TLS_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
            http_post_once(&mut conn, url, body, auth_header)
        };

        match result {
            Ok(resp) => {
                // Healthy — keep the connection warm for the next call.
                PROVIDER_CONN.with(|c| *c.borrow_mut() = Some(conn));
                return Ok(resp);
            }
            Err(e) => {
                // Transient: drop the (poisoned) connection so the next attempt
                // reconnects from scratch.
                drop(conn);
                last_err = e;
                if attempt < MAX_HTTP_ATTEMPTS {
                    log::warn!(
                        "LLM HTTP attempt {}/{} failed ({}); retrying in {}ms",
                        attempt,
                        MAX_HTTP_ATTEMPTS,
                        last_err,
                        backoff_ms
                    );
                    std::thread::sleep(std::time::Duration::from_millis(backoff_ms));
                    backoff_ms *= 2;
                }
            }
        }
    }

    Err(format!("{} (after {} attempts)", last_err, MAX_HTTP_ATTEMPTS))
}

// ---------------------------------------------------------------------------
// OpenAI wire format
// ---------------------------------------------------------------------------

fn build_openai_payload(
    messages: &[Message],
    tools: &[ToolDefinition],
    model: &str,
) -> serde_json::Value {
    let msgs: Vec<serde_json::Value> = messages
        .iter()
        .map(|msg| {
            let role = match msg.role {
                Role::System => "system",
                Role::User => "user",
                Role::Assistant => "assistant",
                Role::Tool => "tool",
            };
            let mut m = serde_json::json!({"role": role, "content": msg.content.as_text()});
            if let Some(ref tcs) = msg.tool_calls {
                let oai_tcs: Vec<serde_json::Value> = tcs
                    .iter()
                    .map(|tc| {
                        let mut obj = serde_json::json!({
                            "id": tc.id,
                            "type": "function",
                            "function": {
                                "name": tc.function.name,
                                "arguments": tc.function.arguments
                            }
                        });
                        // Round-trip opaque per-call extras (e.g. Gemini's
                        // `extra_content.google.thought_signature`) verbatim.
                        // Without this, Gemini returns 400 INVALID_ARGUMENT
                        // citing the missing signature on subsequent turns.
                        if let Some(extra) = &tc.extra_content {
                            obj["extra_content"] = extra.clone();
                        }
                        obj
                    })
                    .collect();
                m["tool_calls"] = serde_json::json!(oai_tcs);
            }
            if let Some(ref id) = msg.tool_call_id {
                m["tool_call_id"] = serde_json::json!(id);
            }
            m
        })
        .collect();

    let mut payload = serde_json::json!({"model": model, "messages": msgs});

    if !tools.is_empty() {
        let oai_tools: Vec<serde_json::Value> = tools
            .iter()
            .map(|t| {
                serde_json::json!({
                    "type": "function",
                    "function": {
                        "name": t.name,
                        "description": t.description,
                        "parameters": t.parameters
                    }
                })
            })
            .collect();
        payload["tools"] = serde_json::json!(oai_tools);
    }

    payload
}

fn parse_openai_response(data: &serde_json::Value) -> Result<LlmResponse, RunnerError> {
    let message = data
        .get("choices")
        .and_then(|c| c.get(0))
        .and_then(|c| c.get("message"))
        .ok_or_else(|| RunnerError::Parse("No choices in response".to_string()))?;

    let text = message
        .get("content")
        .and_then(|c| c.as_str())
        .unwrap_or("")
        .to_string();

    let tool_calls: Vec<ToolCall> = message
        .get("tool_calls")
        .and_then(|tc| tc.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|tc| {
                    let id = tc.get("id")?.as_str()?.to_string();
                    let func = tc.get("function")?;
                    let name = func.get("name")?.as_str()?.to_string();
                    let arguments = func
                        .get("arguments")
                        .and_then(|a| a.as_str())
                        .unwrap_or("{}")
                        .to_string();
                    let extra_content = tc.get("extra_content").cloned();
                    Some(ToolCall {
                        id,
                        function: FunctionCall { name, arguments },
                        extra_content,
                    })
                })
                .collect()
        })
        .unwrap_or_default();

    if tool_calls.is_empty() {
        Ok(LlmResponse::Text(text))
    } else if text.is_empty() {
        Ok(LlmResponse::ToolCalls {
            tool_calls,
            provider_data: None,
        })
    } else {
        Ok(LlmResponse::Mixed {
            text,
            tool_calls,
            provider_data: None,
        })
    }
}

