//! Telegram long-polling receiver.
//!
//! Polls `getUpdates` for incoming messages and exposes them as (chat_id, text)
//! pairs for the gateway to process. Desktop-only (requires reqwest).

#[cfg(feature = "desktop")]
use serde_json::json;

/// A single incoming Telegram message extracted from an update.
#[cfg(feature = "desktop")]
#[derive(Debug, Clone)]
pub struct IncomingMessage {
    pub chat_id: String,
    pub text: String,
    pub from_username: Option<String>,
}

/// Long-polling Telegram bot client.
#[cfg(feature = "desktop")]
pub struct TelegramPoller {
    bot_token: String,
    client: reqwest::Client,
    offset: i64,
}

#[cfg(feature = "desktop")]
impl TelegramPoller {
    pub fn new(bot_token: String) -> Self {
        Self {
            bot_token,
            client: reqwest::Client::new(),
            offset: 0,
        }
    }

    fn api_url(&self, method: &str) -> String {
        format!("https://api.telegram.org/bot{}/{}", self.bot_token, method)
    }

    /// Poll for new updates. Returns a batch of incoming messages.
    ///
    /// Uses long-polling with a 10-second timeout so the connection stays open
    /// on the Telegram side, reducing request overhead.
    pub async fn poll(&mut self) -> Result<Vec<IncomingMessage>, Box<dyn std::error::Error + Send + Sync>> {
        let url = format!(
            "{}?offset={}&timeout=10",
            self.api_url("getUpdates"),
            self.offset
        );

        let resp = self.client.get(&url).send().await?;
        let status = resp.status();
        if !status.is_success() {
            let err_body = resp.text().await.unwrap_or_default();
            return Err(format!("Telegram getUpdates error {}: {}", status, err_body).into());
        }

        let body: serde_json::Value = resp.json().await?;
        let results = body.get("result").and_then(|r| r.as_array());

        let updates = match results {
            Some(arr) => arr,
            None => return Ok(Vec::new()),
        };

        let mut messages = Vec::new();

        for update in updates {
            // Advance offset past this update
            if let Some(update_id) = update.get("update_id").and_then(|v| v.as_i64()) {
                if update_id >= self.offset {
                    self.offset = update_id + 1;
                }
            }

            // Extract message text and chat_id
            let msg = match update.get("message") {
                Some(m) => m,
                None => continue,
            };

            let chat_id = msg
                .get("chat")
                .and_then(|c| c.get("id"))
                .and_then(|id| id.as_i64())
                .map(|id| id.to_string());

            let text = msg.get("text").and_then(|t| t.as_str()).map(String::from);

            let from_username = msg
                .get("from")
                .and_then(|f| f.get("username"))
                .and_then(|u| u.as_str())
                .map(String::from);

            if let (Some(chat_id), Some(text)) = (chat_id, text) {
                messages.push(IncomingMessage {
                    chat_id,
                    text,
                    from_username,
                });
            }
        }

        Ok(messages)
    }

    /// Send a "typing" indicator to a chat.
    pub async fn send_typing(
        &self,
        chat_id: &str,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let url = self.api_url("sendChatAction");
        let body = json!({
            "chat_id": chat_id,
            "action": "typing",
        });

        tracing::info!(chat_id, "Sending typing indicator");

        let resp = self.client.post(&url).json(&body).send().await?;
        let status = resp.status();
        if !status.is_success() {
            let err_body = resp.text().await.unwrap_or_default();
            tracing::warn!(chat_id, status = %status, body = %err_body, "sendChatAction failed");
        }

        Ok(())
    }

    /// Run the poll loop continuously, yielding batches of messages.
    ///
    /// The caller should process each batch (e.g. feed into `gateway.chat()`)
    /// before calling `poll_loop` again, or spawn this in a task and receive
    /// via a channel.
    pub async fn poll_loop(
        &mut self,
        tx: tokio::sync::mpsc::Sender<IncomingMessage>,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        tracing::info!("Telegram poller started");

        loop {
            match self.poll().await {
                Ok(messages) => {
                    for msg in messages {
                        tracing::info!(
                            chat_id = %msg.chat_id,
                            from = ?msg.from_username,
                            text_len = msg.text.len(),
                            "Received Telegram message"
                        );
                        if tx.send(msg).await.is_err() {
                            tracing::info!("Poller channel closed, stopping");
                            return Ok(());
                        }
                    }
                }
                Err(e) => {
                    tracing::error!(error = %e, "Telegram poll error, retrying in 5s");
                    tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                }
            }
        }
    }
}
