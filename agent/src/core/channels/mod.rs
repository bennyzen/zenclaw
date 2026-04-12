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

/// Telegram channel — stub, filled in Task 8.
pub struct TelegramChannel {
    pub bot_token: String,
    pub default_chat_id: String,
}

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
        tracing::info!(chat_id, text_len = text.len(), "Telegram delivery (stub)");
        Ok(())
    }

    async fn deliver_stream(
        &self,
        chat_id: &str,
        chunk: &str,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        tracing::info!(chat_id, chunk_len = chunk.len(), "Telegram stream (stub)");
        Ok(())
    }
}
