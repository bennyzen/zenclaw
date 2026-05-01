use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    System,
    User,
    Assistant,
    Tool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub role: Role,
    pub content: MessageContent,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<ToolCall>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    /// Opaque provider-specific data (e.g. genai ToolCall objects with thought_signatures).
    /// Preserved across turns to satisfy provider round-trip requirements.
    #[serde(skip)]
    pub provider_data: Option<ProviderData>,
}

/// Opaque provider data that must be round-tripped through the agent loop.
/// On ESP32 builds this enum has no inhabited variants — per-tool-call extras
/// are carried on `ToolCall::extra_content` instead, so they persist through
/// JSONL session files automatically.
#[derive(Debug, Clone)]
pub enum ProviderData {
    /// Raw genai tool calls with thought_signatures for Gemini 3.x compatibility.
    #[cfg(feature = "desktop")]
    GenaiToolCalls(Vec<genai::chat::ToolCall>),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum MessageContent {
    Text(String),
    Parts(Vec<ContentPart>),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ContentPart {
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(rename = "image_url")]
    ImageUrl { image_url: ImageUrl },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImageUrl {
    pub url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    pub function: FunctionCall,
    /// Opaque per-call provider extras, round-tripped verbatim. Holds
    /// `extra_content.google.thought_signature` for Gemini's OpenAI-compat
    /// endpoint; future providers can stash their own quirks here without
    /// new code paths. Serializes through JSONL so signatures survive
    /// across HTTP turns.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extra_content: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FunctionCall {
    pub name: String,
    pub arguments: String,
}

#[derive(Debug, Clone)]
pub enum LlmResponse {
    Text(String),
    ToolCalls {
        tool_calls: Vec<ToolCall>,
        provider_data: Option<ProviderData>,
    },
    Mixed {
        text: String,
        tool_calls: Vec<ToolCall>,
        provider_data: Option<ProviderData>,
    },
}

#[derive(Clone)]
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
}
