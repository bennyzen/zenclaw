use async_trait::async_trait;

/// Delivery channel identifier.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChannelKind {
    Cli,
    Telegram,
    Api,
}

/// Trait for delivering messages to a channel.
#[async_trait]
pub trait Channel: Send + Sync {
    fn kind(&self) -> ChannelKind;

    async fn deliver(
        &self,
        chat_id: &str,
        text: &str,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>>;

    async fn deliver_stream(
        &self,
        chat_id: &str,
        chunk: &str,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>>;
}

/// CLI channel — writes to stdout.
pub struct CliChannel;

#[async_trait]
impl Channel for CliChannel {
    fn kind(&self) -> ChannelKind {
        ChannelKind::Cli
    }

    async fn deliver(
        &self,
        _chat_id: &str,
        text: &str,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        println!("{}", text);
        Ok(())
    }

    async fn deliver_stream(
        &self,
        _chat_id: &str,
        chunk: &str,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        print!("{}", chunk);
        Ok(())
    }
}

/// Telegram channel — delivers messages via the Telegram Bot API.
#[cfg(feature = "desktop")]
pub struct TelegramChannel {
    pub bot_token: String,
    pub default_chat_id: String,
    pub client: reqwest::Client,
}

#[cfg(feature = "desktop")]
impl TelegramChannel {
    pub fn new(bot_token: String, default_chat_id: String) -> Self {
        Self {
            bot_token,
            default_chat_id,
            client: reqwest::Client::new(),
        }
    }

    fn api_url(&self, method: &str) -> String {
        format!("https://api.telegram.org/bot{}/{}", self.bot_token, method)
    }
}

#[cfg(feature = "desktop")]
#[async_trait]
impl Channel for TelegramChannel {
    fn kind(&self) -> ChannelKind {
        ChannelKind::Telegram
    }

    async fn deliver(
        &self,
        chat_id: &str,
        text: &str,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let url = self.api_url("sendMessage");
        let body = serde_json::json!({
            "chat_id": chat_id,
            "text": text,
        });

        tracing::info!(chat_id, text_len = text.len(), "Telegram sendMessage");

        let resp = self.client.post(&url).json(&body).send().await?;
        let status = resp.status();
        if !status.is_success() {
            let err_body = resp.text().await.unwrap_or_default();
            return Err(format!("Telegram API error {}: {}", status, err_body).into());
        }

        Ok(())
    }

    async fn deliver_stream(
        &self,
        chat_id: &str,
        chunk: &str,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        // Streaming via edit_message comes later — for now just send as a full message.
        self.deliver(chat_id, chunk).await
    }
}
