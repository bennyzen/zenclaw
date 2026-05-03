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

#[derive(Debug, Clone, Deserialize, Default)]
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

fn default_session_max_bytes() -> usize {
    256_000
}
fn default_log_compaction_bytes() -> usize {
    16_384
}
fn default_replicator_queue_max() -> u32 {
    32
}
fn default_replicator_retry_max() -> u8 {
    5
}
fn default_replicator_backoff_cap_secs() -> u32 {
    60
}
fn default_snapshot_interval_secs() -> u32 {
    900
}
fn default_snapshot_stale_queue_threshold_secs() -> u32 {
    300
}

#[derive(Debug, Clone, Deserialize, Default)]
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

    /// Per-chat log budget. When the on-disk + cloud session log for a
    /// chat exceeds this, [`SessionManager`] rotates it through compaction.
    #[serde(default = "default_session_max_bytes")]
    pub session_max_bytes: usize,
    /// Threshold for opportunistic in-place log compaction (turning the
    /// append-only stream into a snapshot + tail). Smaller than
    /// `session_max_bytes` so compaction kicks in before the rotation cap.
    #[serde(default = "default_log_compaction_bytes")]
    pub log_compaction_bytes: usize,

    #[serde(default)]
    pub replicator: ReplicatorConfig,
    #[serde(default)]
    pub snapshot: SnapshotConfig,
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

/// Cloud-write replicator tunables. The replicator drains the eager-path
/// queue, signs + PUTs each entry, retries with exponential backoff up to
/// `retry_max`, and demotes failures to a dead-letter list.
#[derive(Debug, Clone, Deserialize)]
pub struct ReplicatorConfig {
    /// Max in-flight + pending writes before the producer must spill to
    /// flash (or block, depending on caller).
    #[serde(default = "default_replicator_queue_max")]
    pub queue_max: u32,
    #[serde(default = "default_replicator_retry_max")]
    pub retry_max: u8,
    /// Cap on per-entry exponential backoff (seconds).
    #[serde(default = "default_replicator_backoff_cap_secs")]
    pub backoff_cap_secs: u32,
}

impl Default for ReplicatorConfig {
    fn default() -> Self {
        Self {
            queue_max: default_replicator_queue_max(),
            retry_max: default_replicator_retry_max(),
            backoff_cap_secs: default_replicator_backoff_cap_secs(),
        }
    }
}

/// Periodic flash snapshot for boot-time fallback when S3 is unreachable.
#[derive(Debug, Clone, Deserialize)]
pub struct SnapshotConfig {
    /// Snapshot cadence in seconds. Default 15 min keeps wear low while
    /// bounding RPO when the network drops.
    #[serde(default = "default_snapshot_interval_secs")]
    pub interval_secs: u32,
    /// If the replicator queue's oldest entry has been waiting longer
    /// than this, force a snapshot ahead of the next interval tick.
    #[serde(default = "default_snapshot_stale_queue_threshold_secs")]
    pub stale_queue_threshold_secs: u32,
}

impl Default for SnapshotConfig {
    fn default() -> Self {
        Self {
            interval_secs: default_snapshot_interval_secs(),
            stale_queue_threshold_secs: default_snapshot_stale_queue_threshold_secs(),
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

    /// True when cloud-persistence is configured and operational — i.e. the
    /// `storage` block has bucket + endpoint + access_key_id + secret_access_key
    /// all set. Wraps [`StorageConfig::is_cloud_configured`] with the
    /// outer-Option flatten.
    pub fn is_cloud_enabled(&self) -> bool {
        self.storage
            .as_ref()
            .is_some_and(StorageConfig::is_cloud_configured)
    }
}

#[cfg(test)]
mod cloud_persistence_config_tests {
    use super::*;

    #[test]
    fn storage_config_defaults_session_budget_to_256k() {
        let cfg: StorageConfig = serde_json::from_str(r#"{}"#).unwrap();
        assert_eq!(cfg.session_max_bytes, 256_000);
        assert_eq!(cfg.log_compaction_bytes, 16_384);
    }

    #[test]
    fn storage_config_defaults_replicator() {
        let cfg: StorageConfig = serde_json::from_str(r#"{}"#).unwrap();
        assert_eq!(cfg.replicator.queue_max, 32);
        assert_eq!(cfg.replicator.retry_max, 5);
        assert_eq!(cfg.replicator.backoff_cap_secs, 60);
    }

    #[test]
    fn storage_config_defaults_snapshot() {
        let cfg: StorageConfig = serde_json::from_str(r#"{}"#).unwrap();
        assert_eq!(cfg.snapshot.interval_secs, 900);
        assert_eq!(cfg.snapshot.stale_queue_threshold_secs, 300);
    }

    #[test]
    fn is_cloud_enabled_requires_bucket_and_keys() {
        let mut cfg = Config::default();
        assert!(!cfg.is_cloud_enabled());

        cfg.storage = Some(StorageConfig {
            bucket: Some("b".to_string()),
            access_key_id: Some("k".to_string()),
            secret_access_key: Some("s".to_string()),
            endpoint: Some("https://e".to_string()),
            ..Default::default()
        });
        assert!(cfg.is_cloud_enabled());

        // Missing endpoint → not enabled.
        cfg.storage.as_mut().unwrap().endpoint = None;
        assert!(!cfg.is_cloud_enabled());
    }
}
