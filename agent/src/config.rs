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

fn default_compaction_enabled() -> bool {
    true
}
fn default_compaction_token_threshold() -> usize {
    50_000
}
fn default_compaction_byte_threshold() -> usize {
    200 * 1024
}
fn default_compaction_keep_recent() -> usize {
    6
}
fn default_compaction_max_summary_bytes() -> usize {
    5 * 1024
}
fn default_compaction_max_kept_message_bytes() -> usize {
    24 * 1024
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
pub struct CompactionConfig {
    #[serde(default = "default_compaction_enabled")]
    pub enabled: bool,
    #[serde(default = "default_compaction_token_threshold")]
    pub token_threshold: usize,
    #[serde(default = "default_compaction_byte_threshold")]
    pub byte_threshold: usize,
    #[serde(default = "default_compaction_keep_recent")]
    pub keep_recent: usize,
    #[serde(default = "default_compaction_max_summary_bytes")]
    pub max_summary_bytes: usize,
    /// If a kept Message's content exceeds this, it is replaced with a
    /// short redaction marker during compaction. None disables the cap.
    /// Default catches single huge tool results (e.g. a 491 KB web_fetch
    /// body) sitting inside the keep_recent window — without it, the
    /// byte threshold can re-trip on subsequent turns until the message
    /// rotates out organically.
    #[serde(default = "default_compaction_max_kept_message_bytes")]
    pub max_kept_message_bytes: usize,
}

impl Default for CompactionConfig {
    fn default() -> Self {
        Self {
            enabled: default_compaction_enabled(),
            token_threshold: default_compaction_token_threshold(),
            byte_threshold: default_compaction_byte_threshold(),
            keep_recent: default_compaction_keep_recent(),
            max_summary_bytes: default_compaction_max_summary_bytes(),
            max_kept_message_bytes: default_compaction_max_kept_message_bytes(),
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

fn default_storage_region() -> String {
    "auto".to_string()
}

#[derive(Debug, Clone, Deserialize)]
pub struct StorageConfig {
    pub path: Option<String>,

    // S3/R2-compatible cloud storage. Mirrors the MicroPython config shape
    // (firmware/lib/api/routes_status.py, firmware/lib/s3.py).
    #[serde(default)]
    pub access_key_id: Option<String>,
    #[serde(default)]
    pub secret_access_key: Option<String>,
    #[serde(default)]
    pub endpoint: Option<String>,
    #[serde(default)]
    pub bucket: Option<String>,
    #[serde(default = "default_storage_region")]
    pub region: String,
}

impl StorageConfig {
    /// True only when all four required fields for cloud storage are present
    /// and non-empty.
    pub fn is_cloud_configured(&self) -> bool {
        let nonempty = |v: &Option<String>| v.as_deref().is_some_and(|s| !s.is_empty());
        nonempty(&self.access_key_id)
            && nonempty(&self.secret_access_key)
            && nonempty(&self.endpoint)
            && nonempty(&self.bucket)
    }

    /// "r2" if the endpoint hostname contains r2.cloudflarestorage, else "s3".
    pub fn provider(&self) -> &'static str {
        match self.endpoint.as_deref() {
            Some(ep) if ep.contains("r2.cloudflarestorage") => "r2",
            _ => "s3",
        }
    }
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
