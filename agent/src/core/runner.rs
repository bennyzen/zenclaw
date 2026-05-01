use std::sync::Arc;
use tracing::info;

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
        let mut last_error = None;
        let mut backoff_ms = INITIAL_BACKOFF_MS;

        for attempt in 1..=MAX_RETRIES {
            match self.client.exec_chat(&model, chat_req.clone(), None).await {
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

// --- Convert genai response to our types ---

#[cfg(feature = "desktop")]
fn parse_chat_response(response: genai::chat::ChatResponse) -> Result<LlmResponse, RunnerError> {
    let text = response.first_text().map(|s| s.to_string());
    let tool_calls_raw = response.into_tool_calls();
    let our_tcs = convert_tool_calls(&tool_calls_raw);
    let provider_data = if !tool_calls_raw.is_empty() {
        Some(ProviderData::GenaiToolCalls(tool_calls_raw))
    } else {
        None
    };

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
        })
        .collect()
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
}
