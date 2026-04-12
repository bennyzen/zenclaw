use std::collections::HashMap;
use std::sync::Arc;
use tracing::info;

use crate::config::{Config, ProviderEntry};
use crate::core::types::{
    FunctionCall, LlmResponse, Message, MessageContent, Role, ToolCall, ToolDefinition,
};
use crate::platform::http_client::HttpClient;

// --- Constants ---

const MAX_RETRIES: usize = 3;
const INITIAL_BACKOFF_MS: u64 = 2000;
const MAX_BACKOFF_MS: u64 = 30000;

/// Model name -> provider key hints.
const MODEL_HINTS: &[(&str, &str)] = &[
    ("gpt-", "openai"),
    ("o1", "openai"),
    ("o3", "openai"),
    ("claude", "anthropic"),
    ("gemini", "google"),
    ("deepseek", "deepseek"),
    ("grok", "xai"),
];

/// Provider key -> default base URL.
const PROVIDER_BASE_URLS: &[(&str, &str)] = &[
    ("google", "https://generativelanguage.googleapis.com/v1beta"),
    ("openai", "https://api.openai.com/v1"),
    ("anthropic", "https://api.anthropic.com"),
    ("xai", "https://api.x.ai/v1"),
    ("deepseek", "https://api.deepseek.com/v1"),
    ("groq", "https://api.groq.com/openai/v1"),
    ("openrouter", "https://openrouter.ai/api/v1"),
];

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
    fn is_retryable(&self) -> bool {
        matches!(self, RunnerError::RateLimit | RunnerError::Network(_))
    }
}

// --- Runner ---

/// Provider dispatch with retry and model routing.
pub struct Runner {
    config: Arc<Config>,
    http: Arc<dyn HttpClient>,
}

impl Runner {
    pub fn new(config: Arc<Config>, http: Arc<dyn HttpClient>) -> Self {
        Self { config, http }
    }

    /// Send messages to the LLM with retry and get a response.
    pub async fn call(
        &self,
        messages: &[Message],
        tools: &[ToolDefinition],
        model_override: Option<&str>,
    ) -> Result<LlmResponse, RunnerError> {
        let provider = self.resolve_provider(model_override);
        let model = model_override
            .map(|s| s.to_string())
            .or_else(|| provider.model.clone())
            .unwrap_or_else(|| "gpt-4o-mini".to_string());
        let base_url = provider
            .base_url
            .clone()
            .unwrap_or_else(|| "https://api.openai.com/v1".to_string());
        let api_key = provider.api_key.clone().unwrap_or_default();

        let is_local = base_url.contains("127.0.0.1")
            || base_url.contains("localhost")
            || base_url.contains("192.168.");
        if !is_local && api_key.is_empty() {
            return Err(RunnerError::NoApiKey);
        }

        let is_gemini = is_gemini_url(&base_url);
        let tools_for_llm = build_tools_payload(tools, is_gemini, &model);

        info!(model = %model, provider = %self.config.providers.default, "LLM call");

        let mut last_error = None;
        let mut backoff_ms = INITIAL_BACKOFF_MS;

        for attempt in 1..=MAX_RETRIES {
            match self
                .call_once(&base_url, &api_key, &model, messages, &tools_for_llm, is_gemini)
                .await
            {
                Ok(response) => {
                    if attempt > 1 {
                        info!(attempt, "LLM call succeeded after retry");
                    }
                    return Ok(response);
                }
                Err(e) => {
                    info!(attempt, error = %e, "LLM call failed");
                    if !e.is_retryable() || attempt >= MAX_RETRIES {
                        last_error = Some(e);
                        break;
                    }

                    let sleep_ms = if matches!(e, RunnerError::RateLimit) {
                        (30_000 * (1u64 << (attempt as u64 - 1))).min(MAX_BACKOFF_MS)
                    } else {
                        backoff_ms.min(MAX_BACKOFF_MS)
                    };

                    info!(sleep_ms, "Retrying after backoff");
                    tokio::time::sleep(std::time::Duration::from_millis(sleep_ms)).await;
                    backoff_ms *= 2;
                    last_error = Some(e);
                }
            }
        }

        Err(last_error.unwrap_or(RunnerError::Api("Unknown error after retries".to_string())))
    }

    async fn call_once(
        &self,
        base_url: &str,
        api_key: &str,
        model: &str,
        messages: &[Message],
        tools_payload: &Option<serde_json::Value>,
        is_gemini: bool,
    ) -> Result<LlmResponse, RunnerError> {
        let mut headers = HashMap::new();
        headers.insert(
            "Content-Type".to_string(),
            "application/json; charset=utf-8".to_string(),
        );

        let (url, payload) = if is_gemini {
            build_gemini_request(base_url, api_key, model, messages, tools_payload)
        } else {
            if !api_key.is_empty() {
                headers.insert(
                    "Authorization".to_string(),
                    format!("Bearer {}", api_key),
                );
            }
            build_openai_request(base_url, model, messages, tools_payload)
        };

        let body = serde_json::to_vec(&payload)
            .map_err(|e| RunnerError::Parse(e.to_string()))?;

        let resp = self
            .http
            .post(&url, &headers, &body)
            .await
            .map_err(|e| RunnerError::Network(e.to_string()))?;

        if resp.status == 429 {
            return Err(RunnerError::RateLimit);
        }
        if resp.status == 401 || resp.status == 403 {
            return Err(RunnerError::Auth(format!("HTTP {}", resp.status)));
        }
        if resp.status >= 500 {
            return Err(RunnerError::Network(format!("HTTP {}", resp.status)));
        }
        if resp.status >= 400 {
            let body_str = String::from_utf8_lossy(&resp.body);
            return Err(RunnerError::Api(format!("HTTP {}: {}", resp.status, &body_str[..body_str.len().min(200)])));
        }

        let result: serde_json::Value = serde_json::from_slice(&resp.body)
            .map_err(|e| RunnerError::Parse(e.to_string()))?;

        if let Some(err) = result.get("error") {
            let msg = err
                .get("message")
                .and_then(|m| m.as_str())
                .unwrap_or("Unknown API error");
            if msg.to_lowercase().contains("api key") || msg.to_lowercase().contains("unauthorized") {
                return Err(RunnerError::Auth(msg.to_string()));
            }
            return Err(RunnerError::Api(msg.to_string()));
        }

        if is_gemini {
            parse_gemini_response(&result)
        } else {
            parse_openai_response(&result)
        }
    }

    fn resolve_provider(&self, model_override: Option<&str>) -> ProviderEntry {
        let providers = &self.config.providers;

        // If there's a model override, try to find its provider
        if let Some(model_name) = model_override {
            // Exact match in provider entries
            for (_, entry) in &providers.entries {
                if entry.model.as_deref() == Some(model_name) && entry.api_key.is_some() {
                    return entry.clone();
                }
            }

            // Hint-based match
            for (prefix, provider_key) in MODEL_HINTS {
                if model_name.starts_with(prefix) {
                    if let Some(entry) = providers.entries.get(*provider_key) {
                        return ProviderEntry {
                            api_key: entry.api_key.clone(),
                            base_url: entry.base_url.clone().or_else(|| {
                                PROVIDER_BASE_URLS
                                    .iter()
                                    .find(|(k, _)| k == provider_key)
                                    .map(|(_, v)| v.to_string())
                            }),
                            model: Some(model_name.to_string()),
                            context_window: entry.context_window,
                        };
                    }
                }
            }
        }

        // Default provider
        providers
            .entries
            .get(&providers.default)
            .cloned()
            .unwrap_or(ProviderEntry {
                api_key: None,
                base_url: Some("https://api.openai.com/v1".to_string()),
                model: Some("gpt-4o-mini".to_string()),
                context_window: None,
            })
    }
}

// --- Request building ---

fn is_gemini_url(base_url: &str) -> bool {
    base_url.contains("generativelanguage.googleapis.com")
}

fn build_openai_request(
    base_url: &str,
    model: &str,
    messages: &[Message],
    tools: &Option<serde_json::Value>,
) -> (String, serde_json::Value) {
    let url = format!("{}/chat/completions", base_url);

    let msgs: Vec<serde_json::Value> = messages.iter().map(message_to_openai_json).collect();

    let mut payload = serde_json::json!({
        "model": model,
        "messages": msgs,
    });

    if let Some(t) = tools {
        payload["tools"] = t.clone();
    }

    (url, payload)
}

fn build_gemini_request(
    base_url: &str,
    api_key: &str,
    model: &str,
    messages: &[Message],
    tools: &Option<serde_json::Value>,
) -> (String, serde_json::Value) {
    let clean_url = base_url.replace("/openai", "");
    let url = format!(
        "{}/models/{}:generateContent?key={}",
        clean_url, model, api_key
    );

    let mut contents = Vec::new();
    let mut sys_instruction = None;

    for msg in messages {
        match msg.role {
            Role::System => {
                sys_instruction = Some(msg.content.as_text());
            }
            Role::Tool => {
                let tool_call_id = msg.tool_call_id.as_deref().unwrap_or("unknown");
                let fn_name = tool_call_id
                    .strip_prefix("call_")
                    .unwrap_or(tool_call_id);
                contents.push(serde_json::json!({
                    "role": "function",
                    "parts": [{
                        "functionResponse": {
                            "name": fn_name,
                            "response": { "result": msg.content.as_text() }
                        }
                    }]
                }));
            }
            Role::Assistant => {
                if let Some(ref tcs) = msg.tool_calls {
                    let mut parts = Vec::new();
                    let text = msg.content.as_text();
                    if !text.is_empty() {
                        parts.push(serde_json::json!({"text": text}));
                    }
                    for tc in tcs {
                        let args: serde_json::Value =
                            serde_json::from_str(&tc.function.arguments).unwrap_or_default();
                        parts.push(serde_json::json!({
                            "functionCall": {
                                "name": tc.function.name,
                                "args": args,
                            }
                        }));
                    }
                    contents.push(serde_json::json!({"role": "model", "parts": parts}));
                } else {
                    let text = msg.content.as_text();
                    if !text.is_empty() {
                        contents.push(serde_json::json!({
                            "role": "model",
                            "parts": [{"text": text}]
                        }));
                    }
                }
            }
            Role::User => {
                let text = msg.content.as_text();
                if !text.is_empty() {
                    contents.push(serde_json::json!({
                        "role": "user",
                        "parts": [{"text": text}]
                    }));
                }
            }
        }
    }

    let mut payload = serde_json::json!({"contents": contents});

    if let Some(sys) = sys_instruction {
        payload["systemInstruction"] = serde_json::json!({"parts": [{"text": sys}]});
    }

    if let Some(t) = tools {
        payload["tools"] = t.clone();
    }

    (url, payload)
}

fn build_tools_payload(
    tools: &[ToolDefinition],
    is_gemini: bool,
    _model: &str,
) -> Option<serde_json::Value> {
    if tools.is_empty() {
        return if is_gemini {
            Some(serde_json::json!([{"google_search": {}}]))
        } else {
            None
        };
    }

    if is_gemini {
        let declarations: Vec<serde_json::Value> = tools
            .iter()
            .map(|t| {
                serde_json::json!({
                    "name": t.name,
                    "description": t.description,
                    "parameters": t.parameters,
                })
            })
            .collect();
        Some(serde_json::json!([{"functionDeclarations": declarations}]))
    } else {
        let defs: Vec<serde_json::Value> = tools
            .iter()
            .map(|t| {
                serde_json::json!({
                    "type": "function",
                    "function": {
                        "name": t.name,
                        "description": t.description,
                        "parameters": t.parameters,
                    }
                })
            })
            .collect();
        Some(serde_json::json!(defs))
    }
}

// --- Response parsing ---

fn parse_openai_response(result: &serde_json::Value) -> Result<LlmResponse, RunnerError> {
    let msg = result
        .pointer("/choices/0/message")
        .ok_or_else(|| RunnerError::Parse("No choices in response".to_string()))?;

    let content = msg.get("content").and_then(|c| c.as_str()).map(|s| s.to_string());
    let tool_calls = parse_openai_tool_calls(msg);

    match (content, tool_calls) {
        (Some(text), Some(tcs)) if !text.is_empty() => Ok(LlmResponse::Mixed {
            text,
            tool_calls: tcs,
        }),
        (_, Some(tcs)) => Ok(LlmResponse::ToolCalls(tcs)),
        (Some(text), None) => Ok(LlmResponse::Text(text)),
        (None, None) => Ok(LlmResponse::Text(String::new())),
    }
}

fn parse_openai_tool_calls(msg: &serde_json::Value) -> Option<Vec<ToolCall>> {
    let tcs = msg.get("tool_calls")?.as_array()?;
    let calls: Vec<ToolCall> = tcs
        .iter()
        .filter_map(|tc| {
            let id = tc.get("id")?.as_str()?.to_string();
            let func = tc.get("function")?;
            let name = func.get("name")?.as_str()?.to_string();
            let args = func
                .get("arguments")
                .and_then(|a| a.as_str())
                .unwrap_or("{}")
                .to_string();
            Some(ToolCall {
                id,
                function: FunctionCall {
                    name,
                    arguments: args,
                },
            })
        })
        .collect();

    if calls.is_empty() {
        None
    } else {
        Some(calls)
    }
}

fn parse_gemini_response(data: &serde_json::Value) -> Result<LlmResponse, RunnerError> {
    let parts = data
        .pointer("/candidates/0/content/parts")
        .and_then(|p| p.as_array())
        .cloned()
        .unwrap_or_default();

    let mut text = String::new();
    let mut tool_calls = Vec::new();

    for part in &parts {
        // Skip thought parts
        if part.get("thought").is_some() {
            continue;
        }
        if let Some(t) = part.get("text").and_then(|t| t.as_str()) {
            text.push_str(t);
        }
        if let Some(fc) = part.get("functionCall") {
            let name = fc
                .get("name")
                .and_then(|n| n.as_str())
                .unwrap_or("")
                .to_string();
            let args = fc
                .get("args")
                .map(|a| serde_json::to_string(a).unwrap_or_else(|_| "{}".to_string()))
                .unwrap_or_else(|| "{}".to_string());
            tool_calls.push(ToolCall {
                id: format!("call_{}", name),
                function: FunctionCall {
                    name,
                    arguments: args,
                },
            });
        }
    }

    // Append grounding sources
    if let Some(chunks) = data
        .pointer("/candidates/0/groundingMetadata/groundingChunks")
        .and_then(|c| c.as_array())
    {
        let sources: Vec<String> = chunks
            .iter()
            .filter_map(|c| {
                let web = c.get("web")?;
                let uri = web.get("uri")?.as_str()?;
                let title = web.get("title").and_then(|t| t.as_str());
                Some(if let Some(t) = title {
                    format!("[{}]({})", t, uri)
                } else {
                    uri.to_string()
                })
            })
            .collect();
        if !sources.is_empty() && !text.is_empty() {
            text.push_str("\n\nSources: ");
            text.push_str(&sources.join(", "));
        }
    }

    match (text.is_empty(), tool_calls.is_empty()) {
        (false, false) => Ok(LlmResponse::Mixed { text, tool_calls }),
        (true, false) => Ok(LlmResponse::ToolCalls(tool_calls)),
        (_, true) => Ok(LlmResponse::Text(text)),
    }
}

// --- Helpers ---

fn message_to_openai_json(msg: &Message) -> serde_json::Value {
    let mut obj = serde_json::json!({
        "role": msg.role,
        "content": msg.content.as_text(),
    });

    if let Some(ref tcs) = msg.tool_calls {
        let tc_json: Vec<serde_json::Value> = tcs
            .iter()
            .map(|tc| {
                serde_json::json!({
                    "id": tc.id,
                    "type": "function",
                    "function": {
                        "name": tc.function.name,
                        "arguments": tc.function.arguments,
                    }
                })
            })
            .collect();
        obj["tool_calls"] = serde_json::json!(tc_json);
    }

    if let Some(ref id) = msg.tool_call_id {
        obj["tool_call_id"] = serde_json::json!(id);
    }

    obj
}

impl MessageContent {
    /// Extract plain text from content, regardless of variant.
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_gemini_url() {
        assert!(is_gemini_url("https://generativelanguage.googleapis.com/v1beta"));
        assert!(!is_gemini_url("https://api.openai.com/v1"));
    }

    #[test]
    fn test_parse_openai_response_text() {
        let data = serde_json::json!({
            "choices": [{"message": {"role": "assistant", "content": "Hello!"}}]
        });
        let resp = parse_openai_response(&data).unwrap();
        assert!(matches!(resp, LlmResponse::Text(ref s) if s == "Hello!"));
    }

    #[test]
    fn test_parse_openai_response_tool_calls() {
        let data = serde_json::json!({
            "choices": [{"message": {
                "role": "assistant",
                "content": null,
                "tool_calls": [{
                    "id": "call_123",
                    "type": "function",
                    "function": {"name": "file", "arguments": "{\"action\":\"read\"}"}
                }]
            }}]
        });
        let resp = parse_openai_response(&data).unwrap();
        match resp {
            LlmResponse::ToolCalls(tcs) => {
                assert_eq!(tcs.len(), 1);
                assert_eq!(tcs[0].function.name, "file");
            }
            _ => panic!("Expected ToolCalls"),
        }
    }

    #[test]
    fn test_parse_gemini_response_text() {
        let data = serde_json::json!({
            "candidates": [{"content": {"parts": [{"text": "Hi there"}]}}]
        });
        let resp = parse_gemini_response(&data).unwrap();
        assert!(matches!(resp, LlmResponse::Text(ref s) if s == "Hi there"));
    }

    #[test]
    fn test_parse_gemini_response_function_call() {
        let data = serde_json::json!({
            "candidates": [{"content": {"parts": [{
                "functionCall": {"name": "file", "args": {"action": "read"}}
            }]}}]
        });
        let resp = parse_gemini_response(&data).unwrap();
        match resp {
            LlmResponse::ToolCalls(tcs) => {
                assert_eq!(tcs.len(), 1);
                assert_eq!(tcs[0].function.name, "file");
                assert_eq!(tcs[0].id, "call_file");
            }
            _ => panic!("Expected ToolCalls"),
        }
    }

    #[test]
    fn test_parse_gemini_skips_thoughts() {
        let data = serde_json::json!({
            "candidates": [{"content": {"parts": [
                {"thought": true, "text": "thinking..."},
                {"text": "Hello!"}
            ]}}]
        });
        let resp = parse_gemini_response(&data).unwrap();
        assert!(matches!(resp, LlmResponse::Text(ref s) if s == "Hello!"));
    }

    #[test]
    fn test_build_tools_payload_openai() {
        let tools = vec![ToolDefinition {
            name: "file".to_string(),
            description: "File ops".to_string(),
            parameters: serde_json::json!({"type": "object"}),
        }];
        let payload = build_tools_payload(&tools, false, "gpt-4o");
        assert!(payload.is_some());
        let p = payload.unwrap();
        assert!(p.is_array());
        assert_eq!(p[0]["type"], "function");
    }

    #[test]
    fn test_build_tools_payload_gemini() {
        let tools = vec![ToolDefinition {
            name: "file".to_string(),
            description: "File ops".to_string(),
            parameters: serde_json::json!({"type": "object"}),
        }];
        let payload = build_tools_payload(&tools, true, "gemini-2.5-flash");
        assert!(payload.is_some());
        let p = payload.unwrap();
        assert!(p[0].get("functionDeclarations").is_some());
    }

    #[test]
    fn test_message_content_as_text() {
        let text = MessageContent::Text("hello".to_string());
        assert_eq!(text.as_text(), "hello");

        let parts = MessageContent::Parts(vec![
            crate::core::types::ContentPart::Text {
                text: "hello ".to_string(),
            },
            crate::core::types::ContentPart::Text {
                text: "world".to_string(),
            },
        ]);
        assert_eq!(parts.as_text(), "hello world");
    }
}
