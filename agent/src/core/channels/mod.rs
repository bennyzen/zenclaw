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
