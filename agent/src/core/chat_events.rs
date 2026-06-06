//! Typed event stream for chat turns.
//!
//! Threaded through `Gateway::chat` → `agent_loop::run_loop` →
//! `execute_tool_calls` as `events: Option<&Sender<ChatEvent>>`. REST callers
//! pass `None` (no-op); WS handlers pass `Some` and forward each event to
//! the browser as a JSON text frame.
use serde::{Deserialize, Serialize};

/// Sender alias used across the codebase. `std::sync::mpsc` works on both
/// ESP32 (no tokio) and desktop (we wrap into the tokio runtime at the WS
/// boundary).
pub type Sender = std::sync::mpsc::Sender<ChatEvent>;
pub type Receiver = std::sync::mpsc::Receiver<ChatEvent>;

/// One event in a chat turn. Serialized with a `type` tag so each variant
/// becomes `{"type":"…", …}` on the wire — matches the inbound shape so the
/// browser can union-handle history replay and live events with one renderer.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ChatEvent {
    /// User turn boundary. Emitted only by the history endpoint when
    /// replaying — the live WS path receives this from the browser as the
    /// inbound frame and does not echo it back.
    UserMessage {
        chat_id: String,
        text: String,
    },

    /// LLM call started — the model is reasoning about the next step.
    ThinkingStarted,

    /// LLM call returned. Always paired with a preceding `ThinkingStarted`.
    ThinkingEnded,

    /// Tool dispatch begun. `id` matches the LLM's tool_call_id and pairs
    /// with exactly one `ToolCallFinished` later in the stream.
    ToolCallStarted {
        id: String,
        name: String,
        /// Parsed JSON args. Falls back to `null` if the LLM emitted
        /// invalid JSON (the loop tolerates that already).
        args: serde_json::Value,
    },

    /// Tool dispatch returned. `ok=true` carries the result string;
    /// `ok=false` carries an error message (cap-or-refuse, loop-detector
    /// block, cancellation).
    ToolCallFinished {
        id: String,
        ok: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        result: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        error: Option<String>,
    },

    /// Assistant text. v1 only emits this for the user-facing final reply
    /// (intermediate "tool_calls only, no text" assistant messages stay
    /// invisible in the UI by design — only their tool calls show).
    AssistantText {
        text: String,
        #[serde(rename = "final")]
        is_final: bool,
    },

    /// Turn completed successfully.
    Done,

    /// Turn aborted with an error.
    Error { error: String },

    /// Inbound only — browser-initiated cancellation of an active turn.
    /// Outbound side never emits this; included in the union so the WS
    /// handler can deserialize inbound frames with the same enum.
    Cancel { chat_id: String },
}

/// Send an event, swallowing errors. A closed channel means the browser
/// disconnected — the agent loop should keep running to completion (so the
/// turn lands in the session JSONL) rather than aborting.
pub fn try_send(events: Option<&Sender>, evt: ChatEvent) {
    if let Some(tx) = events {
        let _ = tx.send(evt);
    }
}
