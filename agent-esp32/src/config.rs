use serde::Deserialize;
use std::collections::HashMap;

fn default_agent_name() -> String {
    "ZenClaw".to_string()
}

fn deserialize_nonempty_or_default_name<'de, D>(de: D) -> Result<String, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let s: Option<String> = Option::deserialize(de)?;
    Ok(s.filter(|v| !v.is_empty()).unwrap_or_else(default_agent_name))
}

fn default_provider() -> String {
    "google".to_string()
}

fn default_search_provider() -> String {
    "google".to_string()
}

fn default_memory_top_k() -> usize {
    5
}

fn default_compaction_threshold() -> usize {
    50
}

fn default_heartbeat_secs() -> u64 {
    300
}

fn default_stream_debounce_ms() -> u64 {
    500
}

#[derive(Debug, Clone, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub providers: ProvidersConfig,
    #[serde(default = "default_agent_name", deserialize_with = "deserialize_nonempty_or_default_name")]
    pub agent_name: String,
    #[serde(default)]
    pub channels: ChannelsConfig,
    #[serde(default)]
    pub heartbeat: HeartbeatConfig,
    #[serde(default)]
    pub memory: MemoryConfig,
    #[serde(default)]
    pub compaction: CompactionConfig,
    #[serde(default)]
    pub search: SearchConfig,
    #[serde(default)]
    pub api: Option<ApiConfig>,
    #[serde(default)]
    pub storage: Option<StorageConfig>,
    #[serde(default)]
    pub google: Option<GoogleConfig>,
    #[serde(default)]
    pub hub_url: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct ProvidersConfig {
    #[serde(default = "default_provider")]
    pub default: String,
    #[serde(flatten)]
    pub entries: HashMap<String, ProviderEntry>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ProviderEntry {
    pub api_key: Option<String>,
    pub base_url: Option<String>,
    pub model: Option<String>,
    pub context_window: Option<usize>,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct ChannelsConfig {
    #[serde(default)]
    pub telegram: Option<TelegramConfig>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct TelegramConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub bot_token: String,
    #[serde(default)]
    pub default_chat_id: String,
    #[serde(default)]
    pub allowed_chat_ids: Option<Vec<String>>,
    #[serde(default = "default_stream_debounce_ms")]
    pub stream_debounce_ms: u64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct HeartbeatConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_heartbeat_secs")]
    pub every_secs: u64,
}

impl Default for HeartbeatConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            every_secs: default_heartbeat_secs(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct MemoryConfig {
    #[serde(default = "default_memory_top_k")]
    pub top_k: usize,
}

impl Default for MemoryConfig {
    fn default() -> Self {
        Self {
            top_k: default_memory_top_k(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct CompactionConfig {
    #[serde(default = "default_compaction_threshold")]
    pub threshold: usize,
}

impl Default for CompactionConfig {
    fn default() -> Self {
        Self {
            threshold: default_compaction_threshold(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct SearchConfig {
    #[serde(default = "default_search_provider")]
    pub provider: String,
}

impl Default for SearchConfig {
    fn default() -> Self {
        Self {
            provider: default_search_provider(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct ApiConfig {
    #[serde(default)]
    pub tls: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct StorageConfig {
    pub path: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct GoogleConfig {
    pub client_id: Option<String>,
    pub client_secret: Option<String>,
}

impl Config {
    pub fn load(path: &str) -> Result<Config, Box<dyn std::error::Error>> {
        let contents = std::fs::read_to_string(path)?;
        let config: Config = serde_json::from_str(&contents)?;
        Ok(config)
    }
}
