//! Per-session metadata sidecar (`data/sessions/<chat_id>.meta.json`).
//!
//! Replicates alongside the session's JSONL via the existing per-chat
//! cloud prefix `sys/sessions/<chat_id>/meta.json`. Self-healing: a
//! missing or corrupt sidecar degrades to a synthesized default; only
//! LLM-summarized and user-renamed titles are lost, both rebuildable.

use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum SessionKind {
    Web,
    Telegram,
    Cron,
    Other,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum TitleSource {
    Llm,
    User,
    FirstMessage,
    Default,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SessionMeta {
    pub chat_id: String,
    pub kind: SessionKind,
    pub title: String,
    pub title_source: TitleSource,
    pub created_at_ms: u64,
    pub last_activity_ms: u64,
    pub last_message_preview: String,
    #[serde(default = "default_version")]
    pub version: u32,
}

fn default_version() -> u32 {
    1
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn meta_serde_roundtrip() {
        let meta = SessionMeta {
            chat_id: "chat-1714914000000".into(),
            kind: SessionKind::Web,
            title: "Tomato propagation".into(),
            title_source: TitleSource::Llm,
            created_at_ms: 1714914000000,
            last_activity_ms: 1714915800000,
            last_message_preview: "air-layering instead.".into(),
            version: 1,
        };
        let json = serde_json::to_string(&meta).unwrap();
        let back: SessionMeta = serde_json::from_str(&json).unwrap();
        assert_eq!(meta, back);
    }

    #[test]
    fn version_field_round_trips_when_missing() {
        // Schema-evolution insurance: an older meta file with no `version`
        // field must deserialize via the serde default.
        let json = r#"{
            "chatId": "x",
            "kind": "web",
            "title": "t",
            "titleSource": "default",
            "createdAtMs": 1,
            "lastActivityMs": 1,
            "lastMessagePreview": ""
        }"#;
        let meta: SessionMeta = serde_json::from_str(json).unwrap();
        assert_eq!(meta.version, 1);
    }
}
