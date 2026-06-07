use std::sync::Arc;
use tracing::{info, warn};

use async_trait::async_trait;

use crate::config::Config;
use crate::core::types::{
    FunctionCall, LlmResponse, Message, MessageContent, ProviderData, Role, ToolCall,
    ToolDefinition,
};

// --- Constants ---

const MAX_RETRIES: usize = 3;
const INITIAL_BACKOFF_MS: u64 = 2000;
const MAX_BACKOFF_MS: u64 = 30000;

// --- Error types ---

#[derive(Debug, thiserror::Error)]
pub enum RunnerError {
    #[error("API error: {0}")]
    Api(String),
    #[error("Auth error: {0}")]
    Auth(String),
    #[error("Rate limited")]
    RateLimit,
    #[error("Network error: {0}")]
    Network(String),
    #[error("No API key configured for provider")]
    NoApiKey,
    #[error("Parse error: {0}")]
    Parse(String),
}

impl RunnerError {
    pub fn is_retryable(&self) -> bool {
        matches!(self, RunnerError::RateLimit | RunnerError::Network(_))
    }
}

/// Pull a human-readable error message out of an OpenAI-compatible error
/// response body. Handles the common shapes:
/// `{"error": {"message": "..."}}`, `{"error": "..."}`, `{"message": "..."}`.
/// Returns `None` if nothing usable is found (caller should fall back to the
/// raw body).
pub fn extract_provider_error_message(body: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(body).ok()?;
    let candidate = v
        .get("error")
        .and_then(|e| e.get("message").or(Some(e)))
        .or_else(|| v.get("message"))
        .or_else(|| v.get("error"));
    match candidate {
        Some(serde_json::Value::String(s)) if !s.is_empty() => Some(s.clone()),
        _ => None,
    }
}

/// Classify a *completed* HTTP response (status + body) from an
/// OpenAI-compatible provider into a `RunnerError`, surfacing the status code
/// and the provider's error message verbatim so the user sees the real cause
/// instead of a generic "network error".
///
/// Retryability follows `is_retryable()`:
/// - `401/403` → `Auth` (fatal — wrong/expired key)
/// - `429` → `RateLimit` (transient, retried) — UNLESS the body indicates an
///   insufficient-balance condition, which z.ai returns as 429 but is fatal
/// - `5xx` → `Network` (transient, retried with backoff)
/// - any other `4xx` → `Api` (fatal — bad request, wrong model/endpoint, etc.)
pub fn classify_http_error(status: u16, body: &str) -> RunnerError {
    let msg = extract_provider_error_message(body).unwrap_or_else(|| {
        let trimmed = body.trim();
        trimmed[..trimmed.len().min(300)].to_string()
    });
    let with_code = format!("HTTP {}: {}", status, msg);
    match status {
        401 | 403 => RunnerError::Auth(with_code),
        429 if msg.to_lowercase().contains("balance")
            || msg.to_lowercase().contains("insufficient")
            || msg.to_lowercase().contains("quota") =>
        {
            // z.ai (and others) return 429 for a depleted balance/quota — that
            // never recovers on retry, so surface it as a fatal API error.
            RunnerError::Api(with_code)
        }
        429 => RunnerError::RateLimit,
        500 | 502 | 503 | 504 => RunnerError::Network(with_code),
        _ => RunnerError::Api(with_code),
    }
}

#[cfg(test)]
mod classify_tests {
    use super::*;

    #[test]
    fn extracts_nested_error_message() {
        let body = r#"{"error":{"message":"Invalid API key","type":"auth"}}"#;
        assert_eq!(
            extract_provider_error_message(body).as_deref(),
            Some("Invalid API key")
        );
    }

    #[test]
    fn extracts_string_error_and_top_level_message() {
        assert_eq!(
            extract_provider_error_message(r#"{"error":"boom"}"#).as_deref(),
            Some("boom")
        );
        assert_eq!(
            extract_provider_error_message(r#"{"message":"nope"}"#).as_deref(),
            Some("nope")
        );
    }

    #[test]
    fn extract_returns_none_on_junk() {
        assert_eq!(extract_provider_error_message("not json"), None);
        assert_eq!(extract_provider_error_message("{}"), None);
    }

    #[test]
    fn auth_codes_are_fatal_and_carry_code() {
        for status in [401u16, 403] {
            let e = classify_http_error(status, r#"{"error":{"message":"bad key"}}"#);
            assert!(matches!(e, RunnerError::Auth(_)), "{status} should be Auth");
            assert!(!e.is_retryable());
            let s = e.to_string();
            assert!(s.contains(&format!("HTTP {status}")) && s.contains("bad key"));
        }
    }

    #[test]
    fn plain_rate_limit_is_retryable() {
        let e = classify_http_error(429, r#"{"error":{"message":"too many requests"}}"#);
        assert!(matches!(e, RunnerError::RateLimit));
        assert!(e.is_retryable());
    }

    #[test]
    fn insufficient_balance_429_is_fatal() {
        let e = classify_http_error(429, r#"{"error":{"message":"Insufficient balance"}}"#);
        assert!(matches!(e, RunnerError::Api(_)));
        assert!(!e.is_retryable());
        assert!(e.to_string().contains("HTTP 429") && e.to_string().contains("balance"));
    }

    #[test]
    fn server_errors_are_transient_with_code() {
        for status in [500u16, 502, 503, 504] {
            let e = classify_http_error(status, "upstream boom");
            assert!(matches!(e, RunnerError::Network(_)), "{status} should be Network");
            assert!(e.is_retryable());
            assert!(e.to_string().contains(&format!("HTTP {status}")));
        }
    }

    #[test]
    fn other_4xx_is_fatal_api_with_code() {
        for status in [400u16, 404, 422] {
            let e = classify_http_error(status, r#"{"error":{"message":"nope"}}"#);
            assert!(matches!(e, RunnerError::Api(_)), "{status} should be Api");
            assert!(!e.is_retryable());
            assert!(e.to_string().contains(&format!("HTTP {status}")));
        }
    }
}

// --- LLM Runner trait (shared between desktop and ESP32) ---

#[async_trait]
pub trait LlmRunner: Send + Sync {
    async fn call(
        &self,
        messages: &[Message],
        tools: &[ToolDefinition],
        model_override: Option<&str>,
    ) -> Result<LlmResponse, RunnerError>;
}

// --- MessageContent helper ---

impl MessageContent {
    pub fn as_text(&self) -> String {
        match self {
            MessageContent::Text(s) => s.clone(),
            MessageContent::Parts(parts) => parts
                .iter()
                .filter_map(|p| match p {
                    crate::core::types::ContentPart::Text { text } => Some(text.as_str()),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join(""),
        }
    }
}

// --- Desktop Runner using genai crate ---

#[cfg(feature = "desktop")]
pub struct Runner {
    config: Arc<Config>,
    client: genai::Client,
}

#[cfg(feature = "desktop")]
fn lookup_provider<'a>(
    config: &'a Config,
    model: &genai::ModelIden,
) -> Option<&'a crate::config::ProviderEntry> {
    let key = model.adapter_kind.as_str().to_lowercase();
    config
        .providers
        .entries
        .get(&key)
        .or_else(|| config.providers.entries.get(&config.providers.default))
}

#[cfg(feature = "desktop")]
impl Runner {
    pub fn new(config: Arc<Config>) -> Self {
        // Resolver lookup: try the genai adapter's lowercase name as a config
        // key; otherwise fall back to the configured default provider. No
        // adapter→config-key translation table — provider keys in config are
        // expected to match the genai adapter name (e.g. "zai", "gemini",
        // "openai"). Existing devices using legacy keys ("z-ai", "google")
        // still work via the default-provider fallback.
        let config_for_auth = config.clone();
        let auth_resolver = genai::resolver::AuthResolver::from_resolver_fn(
            move |model_iden: genai::ModelIden| -> genai::resolver::Result<Option<genai::resolver::AuthData>> {
                let entry = lookup_provider(&config_for_auth, &model_iden);
                Ok(entry.and_then(|e| e.api_key.clone()).map(genai::resolver::AuthData::Key))
            },
        );

        // ServiceTargetResolver: override the per-adapter default endpoint when
        // config supplies a `base_url`. Useful for private OpenAI-compatible
        // deployments (Ollama, Fireworks, etc.).
        //
        // NOTE: a few built-in adapters route by model-name namespace and will
        // overwrite this from inside `to_web_request_data`. For z.ai, the
        // convention is `model: "zai::glm-5.1"` (coding plan) vs `glm-5.1`
        // (standard plan) — the model name carries the endpoint, not base_url.
        let config_for_target = config.clone();
        let service_target_resolver = genai::resolver::ServiceTargetResolver::from_resolver_fn(
            move |service_target: genai::ServiceTarget| -> Result<genai::ServiceTarget, genai::resolver::Error> {
                let base_url = lookup_provider(&config_for_target, &service_target.model)
                    .and_then(|e| e.base_url.clone());
                if let Some(url) = base_url {
                    return Ok(genai::ServiceTarget {
                        endpoint: genai::resolver::Endpoint::from_owned(url),
                        auth: service_target.auth,
                        model: service_target.model,
                    });
                }
                Ok(service_target)
            },
        );

        let client = genai::Client::builder()
            .with_auth_resolver(auth_resolver)
            .with_service_target_resolver(service_target_resolver)
            .build();

        Self { config, client }
    }

    /// Send messages to the LLM with retry (internal, called by trait impl).
    async fn call_impl(
        &self,
        messages: &[Message],
        tools: &[ToolDefinition],
        model_override: Option<&str>,
    ) -> Result<LlmResponse, RunnerError> {
        let model = self.resolve_model(model_override);
        info!(model = %model, provider = %self.config.providers.default, "LLM call");

        let chat_req = build_chat_request(messages, tools);

        // TEMP DIAGNOSTIC: dump first request body to disk so we can diff
        // against a known-working raw HTTP probe.
        {
            use std::sync::atomic::{AtomicBool, Ordering};
            static DUMPED: AtomicBool = AtomicBool::new(false);
            if !DUMPED.swap(true, Ordering::SeqCst) {
                if let Ok(json) = serde_json::to_string_pretty(&chat_req) {
                    let _ = std::fs::write("/tmp/zai_actual_request.json", json);
                    info!("dumped chat_req to /tmp/zai_actual_request.json");
                }
            }
        }

        // TEMP DIAGNOSTIC: enable raw response body capture so parse_chat_response
        // can dump it on the failure signature (text non-empty, 0 tool_calls).
        let chat_options = genai::chat::ChatOptions::default().with_capture_raw_body(true);

        let mut last_error = None;
        let mut backoff_ms = INITIAL_BACKOFF_MS;

        for attempt in 1..=MAX_RETRIES {
            match self.client.exec_chat(&model, chat_req.clone(), Some(&chat_options)).await {
                Ok(response) => {
                    if attempt > 1 {
                        info!(attempt, "LLM call succeeded after retry");
                    }
                    return parse_chat_response(response);
                }
                Err(e) => {
                    let err = classify_genai_error(&e);
                    info!(attempt, error = %e, "LLM call failed");
                    if !err.is_retryable() || attempt >= MAX_RETRIES {
                        last_error = Some(err);
                        break;
                    }
                    let sleep_ms = if matches!(err, RunnerError::RateLimit) {
                        (30_000 * (1u64 << (attempt as u64 - 1))).min(MAX_BACKOFF_MS)
                    } else {
                        backoff_ms.min(MAX_BACKOFF_MS)
                    };
                    info!(sleep_ms, "Retrying after backoff");
                    tokio::time::sleep(std::time::Duration::from_millis(sleep_ms)).await;
                    backoff_ms *= 2;
                    last_error = Some(err);
                }
            }
        }

        Err(last_error.unwrap_or(RunnerError::Api("Unknown error after retries".to_string())))
    }

    /// Stream LLM response, calling on_delta for each text chunk (desktop only).
    pub async fn call_stream(
        &self,
        messages: &[Message],
        tools: &[ToolDefinition],
        model_override: Option<&str>,
        on_delta: &mut dyn FnMut(&str),
    ) -> Result<LlmResponse, RunnerError> {
        use futures::StreamExt;
        use genai::chat::{ChatOptions, ChatStreamEvent};

        let model = self.resolve_model(model_override);
        info!(model = %model, "LLM stream call");

        let chat_req = build_chat_request(messages, tools);
        let chat_options = ChatOptions::default().with_capture_tool_calls(true);

        let mut chat_stream = self
            .client
            .exec_chat_stream(&model, chat_req, Some(&chat_options))
            .await
            .map_err(|e| classify_genai_error(&e))?;

        let mut acc_text = String::new();
        let mut tool_calls = Vec::new();

        while let Some(result) = chat_stream.stream.next().await {
            match result.map_err(|e| classify_genai_error(&e))? {
                ChatStreamEvent::Chunk(chunk) => {
                    let text = &chunk.content;
                    if !text.is_empty() {
                        on_delta(text);
                        acc_text.push_str(text);
                    }
                }
                ChatStreamEvent::End(end) => {
                    if let Some(content) = end.captured_content {
                        for part in content.into_parts() {
                            if let genai::chat::ContentPart::ToolCall(tc) = part {
                                tool_calls.push(tc);
                            }
                        }
                    }
                }
                _ => {} // Start, ReasoningChunk, ThoughtSignatureChunk, ToolCallChunk
            }
        }

        let our_tcs = convert_tool_calls(&tool_calls);
        let provider_data = if !tool_calls.is_empty() {
            Some(ProviderData::GenaiToolCalls(tool_calls))
        } else {
            None
        };

        info!(
            shape = "stream",
            text_len = acc_text.len(),
            tool_calls = our_tcs.len(),
            "LLM response shape"
        );
        probe_tool_call_leak(&acc_text, "stream");

        // GLM-5.1 leak recovery (see recover_glm_tool_calls() doc).
        if our_tcs.is_empty() && acc_text.contains(GLM_OPEN) {
            if let Some(recovered) = recover_glm_tool_calls(&acc_text) {
                warn!(
                    count = recovered.len(),
                    "recovered GLM-leaked tool calls from streamed content"
                );
                return Ok(LlmResponse::ToolCalls {
                    tool_calls: recovered,
                    provider_data: None,
                });
            }
            warn!("GLM markup detected in stream but recovery failed");
        }

        if our_tcs.is_empty() {
            Ok(LlmResponse::Text(acc_text))
        } else if acc_text.is_empty() {
            Ok(LlmResponse::ToolCalls { tool_calls: our_tcs, provider_data })
        } else {
            Ok(LlmResponse::Mixed { text: acc_text, tool_calls: our_tcs, provider_data })
        }
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
            .unwrap_or_else(|| "gpt-4o-mini".to_string())
    }
}

#[cfg(feature = "desktop")]
#[async_trait]
impl LlmRunner for Runner {
    async fn call(
        &self,
        messages: &[Message],
        tools: &[ToolDefinition],
        model_override: Option<&str>,
    ) -> Result<LlmResponse, RunnerError> {
        self.call_impl(messages, tools, model_override).await
    }
}

// --- Convert our types to genai types (desktop only) ---

#[cfg(feature = "desktop")]
fn build_chat_request(
    messages: &[Message],
    tools: &[ToolDefinition],
) -> genai::chat::ChatRequest {
    use genai::chat::{ChatMessage, ChatRequest, Tool, ToolResponse};

    let mut req = ChatRequest::new(Vec::new());

    for msg in messages {
        match msg.role {
            Role::System => {
                req = req.with_system(msg.content.as_text());
            }
            Role::User => {
                req = req.append_message(ChatMessage::user(msg.content.as_text()));
            }
            Role::Assistant => {
                if let Some(ref tcs) = msg.tool_calls {
                    // Use preserved genai ToolCalls if available (carries thought_signatures)
                    let genai_calls = match &msg.provider_data {
                        Some(ProviderData::GenaiToolCalls(raw)) => raw.clone(),
                        _ => {
                            // Fallback: reconstruct from our types (no thought_signatures)
                            tcs.iter()
                                .map(|tc| {
                                    let args: serde_json::Value =
                                        serde_json::from_str(&tc.function.arguments)
                                            .unwrap_or_default();
                                    genai::chat::ToolCall {
                                        call_id: tc.id.clone(),
                                        fn_name: tc.function.name.clone(),
                                        fn_arguments: args,
                                        thought_signatures: None,
                                    }
                                })
                                .collect()
                        }
                    };
                    req = req.append_message(ChatMessage::from(genai_calls));
                } else {
                    req = req.append_message(ChatMessage::assistant(msg.content.as_text()));
                }
            }
            Role::Tool => {
                let call_id = msg.tool_call_id.clone().unwrap_or_default();
                let response = ToolResponse::new(call_id, msg.content.as_text());
                req = req.append_message(ChatMessage::from(response));
            }
        }
    }

    if !tools.is_empty() {
        let genai_tools: Vec<Tool> = tools
            .iter()
            .map(|t| {
                Tool::new(&t.name)
                    .with_description(&t.description)
                    .with_schema(t.parameters.clone())
            })
            .collect();
        req = req.with_tools(genai_tools);
    }

    req
}

// --- Diagnostic: detect GLM XML tool-call markup leaking into Text content ---
//
// genai's `zai` adapter is a thin pass-through over the OpenAI adapter; it
// expects z.ai's server-side parser to have already converted GLM's native
// `<tool_call>...</tool_call>` XML into OpenAI-shape structured tool_calls.
// If the marker shows up in Text content we know the parser leaked.
// Background: memory/project_compound_turn_handoff.md (Step 0).
#[cfg(feature = "desktop")]
fn probe_tool_call_leak(text: &str, source: &str) {
    if let Some(idx) = text.find("<tool_call") {
        let start = idx.saturating_sub(60);
        let end = (idx + 200).min(text.len());
        let excerpt = &text[start..end];
        warn!(
            source,
            marker_offset = idx,
            text_len = text.len(),
            excerpt = %excerpt,
            "glm_leak_probe: <tool_call> markup in Text content (server parser bypass)"
        );
    }
}

// --- Convert genai response to our types ---

#[cfg(feature = "desktop")]
fn parse_chat_response(response: genai::chat::ChatResponse) -> Result<LlmResponse, RunnerError> {
    // Pull captured raw body off before consuming the response.
    let raw_body = response.captured_raw_body.clone();

    let text = response.first_text().map(|s| s.to_string());
    let tool_calls_raw = response.into_tool_calls();
    let our_tcs = convert_tool_calls(&tool_calls_raw);
    let provider_data = if !tool_calls_raw.is_empty() {
        Some(ProviderData::GenaiToolCalls(tool_calls_raw))
    } else {
        None
    };

    let text_len = text.as_ref().map(|t| t.len()).unwrap_or(0);
    let tc_count = our_tcs.len();
    info!(
        shape = "non_stream",
        text_len,
        tool_calls = tc_count,
        "LLM response shape"
    );
    if let Some(ref t) = text {
        probe_tool_call_leak(t, "non_stream");
    }

    // GLM-5.1 leak recovery: when z.ai's coding-plan endpoint fails to convert
    // GLM-native tool-call markup to OpenAI tool_calls, parse the leaked
    // markup ourselves. See recover_glm_tool_calls() doc.
    if tc_count == 0 {
        if let Some(ref t) = text {
            if t.contains(GLM_OPEN) {
                match recover_glm_tool_calls(t) {
                    Some(recovered) => {
                        warn!(
                            count = recovered.len(),
                            "recovered GLM-leaked tool calls from content"
                        );
                        return Ok(LlmResponse::ToolCalls {
                            tool_calls: recovered,
                            provider_data: None,
                        });
                    }
                    None => {
                        if let Some(body) = raw_body.as_ref() {
                            if let Ok(s) = serde_json::to_string_pretty(body) {
                                use std::sync::atomic::{AtomicUsize, Ordering};
                                static SEQ: AtomicUsize = AtomicUsize::new(0);
                                let seq = SEQ.fetch_add(1, Ordering::SeqCst);
                                let path =
                                    format!("/tmp/zai_unrecovered_glm_leak_{:03}.json", seq);
                                let _ = std::fs::write(&path, s);
                                warn!(path, "GLM markup detected but recovery failed");
                            }
                        }
                    }
                }
            }
        }
    }

    match (text, our_tcs.is_empty()) {
        (Some(t), true) if !t.is_empty() => Ok(LlmResponse::Text(t)),
        (_, false) => {
            Ok(LlmResponse::ToolCalls {
                tool_calls: our_tcs,
                provider_data,
            })
        }
        _ => Ok(LlmResponse::Text(String::new())),
    }
}

#[cfg(feature = "desktop")]
fn convert_tool_calls(tool_calls: &[genai::chat::ToolCall]) -> Vec<ToolCall> {
    tool_calls
        .iter()
        .map(|tc| ToolCall {
            id: if tc.call_id.is_empty() {
                format!("call_{}", tc.fn_name)
            } else {
                tc.call_id.clone()
            },
            function: FunctionCall {
                name: tc.fn_name.clone(),
                arguments: serde_json::to_string(&tc.fn_arguments)
                    .unwrap_or_else(|_| "{}".to_string()),
            },
            extra_content: None,
        })
        .collect()
}

// --- GLM-5.1 native tool-call markup recovery ---
//
// z.ai's coding-plan endpoint sometimes fails to convert GLM's native
// tool-call markup to OpenAI tool_calls and leaks the raw markup into the
// `content` field. Format observed:
//   <tool_call家政>裳FUNC(KWARGS)裳FUNC(KWARGS)...</tool_call家政>
// The closing tag is sometimes truncated. KWARGS uses Python-like kwargs
// syntax with JSON-encoded string values: key="value", key2="value2".
//
// `家政` (housekeeping) and `裳` (garment) are deliberately rare CJK
// ideographs chosen by GLM as delimiters that won't collide with text.

#[cfg(feature = "desktop")]
const GLM_OPEN: &str = "<tool_call家政>";
#[cfg(feature = "desktop")]
const GLM_CLOSE: &str = "</tool_call家政>";
#[cfg(feature = "desktop")]
const GLM_PREFIX: &str = "裳";

#[cfg(feature = "desktop")]
fn recover_glm_tool_calls(content: &str) -> Option<Vec<ToolCall>> {
    let open_idx = content.find(GLM_OPEN)?;
    let mut body = &content[open_idx + GLM_OPEN.len()..];
    if let Some(close_idx) = body.rfind(GLM_CLOSE) {
        body = &body[..close_idx];
    }

    let mut calls = Vec::new();
    for chunk in body.split(GLM_PREFIX) {
        let chunk = chunk.trim();
        if chunk.is_empty() {
            continue;
        }
        let parsed = parse_glm_call(chunk, calls.len())?;
        calls.push(parsed);
    }

    if calls.is_empty() {
        None
    } else {
        Some(calls)
    }
}

#[cfg(feature = "desktop")]
fn parse_glm_call(chunk: &str, idx: usize) -> Option<ToolCall> {
    let lparen = chunk.find('(')?;
    let name = chunk[..lparen].trim();
    if name.is_empty() {
        return None;
    }
    let after_lparen = &chunk[lparen + 1..];
    let close_offset = find_top_level_close_paren(after_lparen)?;
    let kwargs_str = &after_lparen[..close_offset];

    let args_json = parse_glm_kwargs(kwargs_str)?;
    let arguments = serde_json::to_string(&args_json).ok()?;
    Some(ToolCall {
        id: format!("call_glm_{}", idx),
        function: FunctionCall {
            name: name.to_string(),
            arguments,
        },
        extra_content: None,
    })
}

/// Find the offset of the first `)` that is not inside a JSON string literal.
#[cfg(feature = "desktop")]
fn find_top_level_close_paren(s: &str) -> Option<usize> {
    let mut in_string = false;
    let mut escape = false;
    for (i, b) in s.bytes().enumerate() {
        if escape {
            escape = false;
            continue;
        }
        if in_string {
            match b {
                b'\\' => escape = true,
                b'"' => in_string = false,
                _ => {}
            }
        } else {
            match b {
                b'"' => in_string = true,
                b')' => return Some(i),
                _ => {}
            }
        }
    }
    None
}

/// Parse `key1="value1", key2="value2"` into a JSON object. Values are
/// JSON-decoded (so escaped quotes and unicode escapes work). Unquoted
/// values are best-effort: try as JSON literal (number/bool/null), fall
/// back to string.
#[cfg(feature = "desktop")]
fn parse_glm_kwargs(s: &str) -> Option<serde_json::Value> {
    let mut obj = serde_json::Map::new();
    for seg in split_top_level_commas(s) {
        let seg = seg.trim();
        if seg.is_empty() {
            continue;
        }
        let eq = seg.find('=')?;
        let key = seg[..eq].trim();
        if key.is_empty() {
            return None;
        }
        let val = seg[eq + 1..].trim();
        let parsed: serde_json::Value = if val.starts_with('"') {
            serde_json::from_str(val).ok()?
        } else {
            serde_json::from_str(val)
                .unwrap_or_else(|_| serde_json::Value::String(val.to_string()))
        };
        obj.insert(key.to_string(), parsed);
    }
    if obj.is_empty() {
        None
    } else {
        Some(serde_json::Value::Object(obj))
    }
}

/// Split on commas that are not inside JSON string literals.
#[cfg(feature = "desktop")]
fn split_top_level_commas(s: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut start = 0usize;
    let mut in_string = false;
    let mut escape = false;
    for (i, b) in s.bytes().enumerate() {
        if escape {
            escape = false;
            continue;
        }
        if in_string {
            match b {
                b'\\' => escape = true,
                b'"' => in_string = false,
                _ => {}
            }
        } else {
            match b {
                b'"' => in_string = true,
                b',' => {
                    out.push(&s[start..i]);
                    start = i + 1;
                }
                _ => {}
            }
        }
    }
    if start <= s.len() {
        out.push(&s[start..]);
    }
    out
}

#[cfg(feature = "desktop")]
fn classify_genai_error(e: &genai::Error) -> RunnerError {
    let msg = e.to_string();
    if msg.contains("429") || msg.to_lowercase().contains("rate") {
        RunnerError::RateLimit
    } else if msg.contains("401") || msg.contains("403") || msg.to_lowercase().contains("auth") {
        RunnerError::Auth(msg)
    } else if msg.contains("500") || msg.contains("502") || msg.contains("503") {
        RunnerError::Network(msg)
    } else {
        RunnerError::Api(msg)
    }
}

#[cfg(all(test, feature = "desktop"))]
mod tests {
    use super::*;

    #[test]
    fn test_build_chat_request_basic() {
        let messages = vec![
            Message {
                role: Role::System,
                content: MessageContent::Text("You are helpful.".to_string()),
                tool_calls: None,
                tool_call_id: None,
                    provider_data: None,
            },
            Message {
                role: Role::User,
                content: MessageContent::Text("Hello".to_string()),
                tool_calls: None,
                tool_call_id: None,
                    provider_data: None,
            },
        ];

        let req = build_chat_request(&messages, &[]);
        assert!(req.system.is_some());
        assert_eq!(req.messages.len(), 1);
    }

    #[test]
    fn test_build_chat_request_with_tools() {
        let messages = vec![Message {
            role: Role::User,
            content: MessageContent::Text("Read a file".to_string()),
            tool_calls: None,
            tool_call_id: None,
                    provider_data: None,
        }];
        let tools = vec![ToolDefinition {
            name: "file".to_string(),
            description: "File ops".to_string(),
            parameters: serde_json::json!({"type": "object"}),
        }];

        let req = build_chat_request(&messages, &tools);
        assert!(req.tools.is_some());
        assert_eq!(req.tools.as_ref().unwrap().len(), 1);
    }

    #[test]
    fn test_message_content_as_text() {
        let text = MessageContent::Text("hello".to_string());
        assert_eq!(text.as_text(), "hello");
    }

    // --- GLM-5.1 tool-call recovery parser ---
    //
    // z.ai's coding-plan endpoint sometimes fails to convert the model's
    // native tool-call markup to OpenAI tool_calls and leaks the raw
    // markup into the `content` field. recover_glm_tool_calls() parses
    // that markup back into structured ToolCalls.
    //
    // Format observed in captured leaks:
    //   <tool_call家政>裳FUNC(KWARGS)裳FUNC(KWARGS)...</tool_call家政>
    // The closing token is sometimes truncated. KWARGS uses Python kwargs
    // syntax with JSON-encoded string values: key="value", key2="value2".

    #[test]
    fn test_recover_glm_three_parallel_file_reads() {
        let content = "The root directory has three `.md` files: `AGENTS.md`, `MEMORY.md`, and `SOUL.md`. Let me read all of them now. <tool_call家政>裳file(action=\"read\", path=\"AGENTS.md\")裳file(action=\"read\", path=\"MEMORY.md\")裳file(action=\"read\", path=\"SOUL.md\")";
        let calls = recover_glm_tool_calls(content).expect("expected recovered calls");
        assert_eq!(calls.len(), 3);
        for (i, fname) in ["AGENTS.md", "MEMORY.md", "SOUL.md"].iter().enumerate() {
            assert_eq!(calls[i].function.name, "file");
            let args: serde_json::Value =
                serde_json::from_str(&calls[i].function.arguments).expect("args parse");
            assert_eq!(args["action"], "read");
            assert_eq!(args["path"], *fname);
        }
    }

    #[test]
    fn test_recover_glm_with_closing_tag_and_apostrophes() {
        // Sample 011 — three memory.save calls with comma-rich content that
        // includes an apostrophe ("agent's"); also has the closing tag.
        let content = "Now I have all three files. <tool_call家政>裳memory(action=\"save\", tags=\"summary,SOUL.md,project_root\", content=\"SOUL.md summary: Defines the agent's identity as ZenClaw.\")</tool_call家政>";
        let calls = recover_glm_tool_calls(content).expect("expected recovered calls");
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].function.name, "memory");
        let args: serde_json::Value =
            serde_json::from_str(&calls[0].function.arguments).expect("args parse");
        assert_eq!(args["action"], "save");
        assert_eq!(args["tags"], "summary,SOUL.md,project_root");
        assert_eq!(
            args["content"],
            "SOUL.md summary: Defines the agent's identity as ZenClaw."
        );
    }

    #[test]
    fn test_recover_glm_no_markup_returns_none() {
        assert!(recover_glm_tool_calls("plain text reply, no markup").is_none());
        assert!(recover_glm_tool_calls("").is_none());
    }

    #[test]
    fn test_recover_glm_handles_escaped_quotes_in_value() {
        let content = "<tool_call家政>裳memory(action=\"save\", content=\"He said \\\"hello\\\" loudly\")</tool_call家政>";
        let calls = recover_glm_tool_calls(content).expect("expected recovered calls");
        assert_eq!(calls.len(), 1);
        let args: serde_json::Value =
            serde_json::from_str(&calls[0].function.arguments).expect("args parse");
        assert_eq!(args["content"], "He said \"hello\" loudly");
    }

    #[test]
    fn test_recover_glm_malformed_returns_none() {
        // No closing paren on first call — parser should give up rather than
        // emit garbage.
        let content = "<tool_call家政>裳file(action=\"read\", path=\"AGENTS.md\"";
        assert!(recover_glm_tool_calls(content).is_none());
    }
}
