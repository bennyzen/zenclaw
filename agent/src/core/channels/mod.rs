//! Delivery channels for messages produced by the gateway.
//!
//! `Channel` is the trait every output sink implements. Today only
//! `TelegramChannel` (in `core/channels/telegram.rs`) is wired; future
//! sinks (Slack, Matrix, web push) drop in here without touching gateway.

use async_trait::async_trait;

#[async_trait]
pub trait Channel: Send + Sync {
    async fn deliver(
        &self,
        chat_id: &str,
        text: &str,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>>;

    /// Default implementation routes through `deliver`. Override for true
    /// streaming (e.g. Telegram editMessageText with debounce).
    async fn deliver_stream(
        &self,
        chat_id: &str,
        chunk: &str,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        self.deliver(chat_id, chunk).await
    }
}

pub mod telegram;
pub(crate) mod markdown_html;

#[cfg(all(test, feature = "desktop"))]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// Channel impl that captures every call for assertion in tests.
    struct CapturingChannel {
        delivered: Mutex<Vec<(String, String)>>,
    }

    #[async_trait]
    impl Channel for CapturingChannel {
        async fn deliver(
            &self,
            chat_id: &str,
            text: &str,
        ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
            self.delivered
                .lock()
                .unwrap()
                .push((chat_id.to_string(), text.to_string()));
            Ok(())
        }
    }

    #[tokio::test]
    async fn deliver_stream_default_routes_to_deliver() {
        let ch = CapturingChannel {
            delivered: Mutex::new(Vec::new()),
        };
        ch.deliver_stream("chat42", "hello").await.unwrap();
        let recorded = ch.delivered.lock().unwrap();
        assert_eq!(recorded.len(), 1);
        assert_eq!(recorded[0].0, "chat42");
        assert_eq!(recorded[0].1, "hello");
    }
}
