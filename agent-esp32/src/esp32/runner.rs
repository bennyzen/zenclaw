use std::sync::Arc;

use async_trait::async_trait;
use log::info;

use crate::config::Config;
use crate::core::runner::{LlmRunner, RunnerError};
use crate::core::types::{
    FunctionCall, LlmResponse, Message, MessageContent, ProviderData, Role, ToolCall,
    ToolDefinition,
};

/// LLM runner for ESP32 — calls Gemini/OpenAI APIs directly via esp-idf-svc HTTP client (mbedtls TLS).
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
            .unwrap_or("https://generativelanguage.googleapis.com/v1beta");

        let is_gemini = base_url.contains("generativelanguage.googleapis.com");

        let (url, payload, auth_header) = if is_gemini {
            let clean = base_url.replace("/openai", "");
            let url = format!("{}/models/{}:generateContent?key={}", clean, model, api_key);
            let payload = build_gemini_payload(messages, tools);
            (url, payload, None)
        } else {
            let url = format!("{}/chat/completions", base_url);
            let payload = build_openai_payload(messages, tools, &model);
            (url, payload, Some(format!("Bearer {}", api_key)))
        };

        let body = serde_json::to_string(&payload)
            .map_err(|e| RunnerError::Parse(e.to_string()))?;

        info!("LLM call: model={} body={}B", model, body.len());

        let response_body = esp_http_post(&url, &body, auth_header.as_deref())
            .map_err(|e| RunnerError::Network(e))?;

        let response: serde_json::Value = serde_json::from_str(&response_body)
            .map_err(|e| RunnerError::Parse(format!("JSON parse: {}", e)))?;

        // Check for API errors
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

        if is_gemini {
            parse_gemini_response(&response)
        } else {
            parse_openai_response(&response)
        }
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

fn esp_http_post(url: &str, body: &str, auth_header: Option<&str>) -> Result<String, String> {
    use esp_idf_svc::http::client::{Configuration as HttpConfig, EspHttpConnection};
    use esp_idf_svc::http::Method;

    let config = HttpConfig {
        buffer_size: Some(4096),
        buffer_size_tx: Some(4096),
        timeout: Some(std::time::Duration::from_secs(60)),
        crt_bundle_attach: Some(esp_idf_svc::sys::esp_crt_bundle_attach),
        ..Default::default()
    };

    let mut conn = EspHttpConnection::new(&config).map_err(|e| format!("HTTP init: {}", e))?;

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

    let mut buf = [0u8; 2048];
    let mut resp_body = Vec::new();
    loop {
        let n = conn.read(&mut buf).map_err(|e| format!("HTTP read: {}", e))?;
        if n == 0 {
            break;
        }
        resp_body.extend_from_slice(&buf[..n]);
    }

    let body_str = String::from_utf8(resp_body)
        .map_err(|e| format!("Invalid UTF-8: {}", e))?;

    if status >= 400 {
        return Err(format!(
            "HTTP {}: {}",
            status,
            &body_str[..body_str.len().min(500)]
        ));
    }

    Ok(body_str)
}

// ---------------------------------------------------------------------------
// Gemini wire format
// ---------------------------------------------------------------------------

fn build_gemini_payload(
    messages: &[Message],
    tools: &[ToolDefinition],
) -> serde_json::Value {
    let mut contents = Vec::new();
    let mut sys_instruction = None;

    for msg in messages {
        match msg.role {
            Role::System => {
                sys_instruction = Some(msg.content.as_text());
            }
            Role::User => {
                contents.push(serde_json::json!({
                    "role": "user",
                    "parts": [{"text": msg.content.as_text()}]
                }));
            }
            Role::Assistant => {
                if let Some(ref tcs) = msg.tool_calls {
                    let mut parts: Vec<serde_json::Value> = Vec::new();
                    let text = msg.content.as_text();
                    if !text.is_empty() {
                        parts.push(serde_json::json!({"text": text}));
                    }
                    for tc in tcs {
                        let args: serde_json::Value =
                            serde_json::from_str(&tc.function.arguments).unwrap_or_default();
                        parts.push(serde_json::json!({
                            "functionCall": {"name": tc.function.name, "args": args}
                        }));
                    }
                    contents.push(serde_json::json!({"role": "model", "parts": parts}));
                } else {
                    contents.push(serde_json::json!({
                        "role": "model",
                        "parts": [{"text": msg.content.as_text()}]
                    }));
                }
            }
            Role::Tool => {
                let call_id = msg.tool_call_id.as_deref().unwrap_or("unknown");
                let fn_name = call_id.strip_prefix("call_").unwrap_or(call_id);
                contents.push(serde_json::json!({
                    "role": "function",
                    "parts": [{
                        "functionResponse": {
                            "name": fn_name,
                            "response": {"result": msg.content.as_text()}
                        }
                    }]
                }));
            }
        }
    }

    let mut payload = serde_json::json!({"contents": contents});

    if let Some(sys) = sys_instruction {
        payload["systemInstruction"] = serde_json::json!({"parts": [{"text": sys}]});
    }

    if !tools.is_empty() {
        let declarations: Vec<serde_json::Value> = tools
            .iter()
            .map(|t| {
                let mut decl = serde_json::json!({
                    "name": t.name,
                    "description": t.description,
                });
                if !t.parameters.is_null() {
                    decl["parameters"] = t.parameters.clone();
                }
                decl
            })
            .collect();
        payload["tools"] = serde_json::json!([{"functionDeclarations": declarations}]);
    }

    payload
}

fn parse_gemini_response(data: &serde_json::Value) -> Result<LlmResponse, RunnerError> {
    let candidate = data
        .get("candidates")
        .and_then(|c| c.get(0))
        .ok_or_else(|| RunnerError::Parse("No candidates in response".to_string()))?;

    let parts = candidate
        .get("content")
        .and_then(|c| c.get("parts"))
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
            let args = fc.get("args").cloned().unwrap_or_default();
            tool_calls.push(ToolCall {
                id: format!("call_{}", name),
                function: FunctionCall {
                    name,
                    arguments: serde_json::to_string(&args).unwrap_or_else(|_| "{}".to_string()),
                },
            });
        }
    }

    let provider_data = if !parts.is_empty() {
        Some(ProviderData::GeminiParts(parts))
    } else {
        None
    };

    if tool_calls.is_empty() {
        Ok(LlmResponse::Text(text))
    } else if text.is_empty() {
        Ok(LlmResponse::ToolCalls {
            tool_calls,
            provider_data,
        })
    } else {
        Ok(LlmResponse::Mixed {
            text,
            tool_calls,
            provider_data,
        })
    }
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
                        serde_json::json!({
                            "id": tc.id,
                            "type": "function",
                            "function": {
                                "name": tc.function.name,
                                "arguments": tc.function.arguments
                            }
                        })
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
                    Some(ToolCall {
                        id,
                        function: FunctionCall { name, arguments },
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
