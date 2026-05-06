// --- Global log ring buffer for /ws/logs streaming ---
#[cfg(feature = "esp32")]
mod log_ring {
    use std::sync::Mutex;
    use std::collections::VecDeque;

    const MAX_ENTRIES: usize = 200;

    struct Entry {
        level: &'static str,
        msg: String,
        source: String,
    }

    static RING: Mutex<VecDeque<Entry>> = Mutex::new(VecDeque::new());
    static EPOCH: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

    pub fn push(level: &'static str, msg: String, source: String) {
        if let Ok(mut ring) = RING.lock() {
            if ring.len() >= MAX_ENTRIES { ring.pop_front(); }
            ring.push_back(Entry { level, msg, source });
            EPOCH.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }
    }

    pub fn epoch() -> u32 {
        EPOCH.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Drain entries newer than `since_epoch`. Returns (entries_json, new_epoch).
    pub fn drain_since(since: u32) -> (Vec<String>, u32) {
        let current = epoch();
        if current == since { return (vec![], since); }
        let mut out = Vec::new();
        if let Ok(ring) = RING.lock() {
            // Take the last (current - since) entries, capped to ring length
            let count = (current - since) as usize;
            let skip = ring.len().saturating_sub(count);
            for e in ring.iter().skip(skip) {
                out.push(format!(
                    r#"{{"level":"{}","msg":"{}","source":"{}"}}"#,
                    e.level,
                    e.msg.replace('\\', "\\\\").replace('"', "\\\""),
                    e.source.replace('\\', "\\\\").replace('"', "\\\""),
                ));
            }
        }
        (out, current)
    }

    /// Custom logger that tees to ESP-IDF console + ring buffer.
    pub struct RingLogger;

    impl log::Log for RingLogger {
        fn enabled(&self, _metadata: &log::Metadata) -> bool { true }
        fn log(&self, record: &log::Record) {
            if record.level() <= log::max_level() {
                let level = match record.level() {
                    log::Level::Error => "error",
                    log::Level::Warn => "warn",
                    log::Level::Info => "info",
                    log::Level::Debug => "debug",
                    log::Level::Trace => "trace",
                };
                let msg = format!("{}", record.args());
                let source = record.target().to_string();
                println!("[{}] {}: {}", level.to_uppercase(), source, msg);
                push(level, msg, source);
            }
        }
        fn flush(&self) {}
    }

    static LOGGER: RingLogger = RingLogger;

    pub fn init() {
        let _ = log::set_logger(&LOGGER);
        log::set_max_level(log::LevelFilter::Info);
    }
}

/// `HostFacts` impl for the ESP32 build. Reads heap/PSRAM via esp-idf-sys,
/// link details via the `Nic` trait. Hostname is captured at boot from
/// the resolved mDNS name (NVS-stored or MAC-derived fallback).
#[cfg(feature = "esp32")]
struct Esp32HostFacts {
    hostname: String,
    started: std::time::Instant,
    nic: std::sync::Arc<Box<dyn zenclaw_agent::net::Nic>>,
}

#[cfg(feature = "esp32")]
impl Esp32HostFacts {
    fn new(
        hostname: String,
        nic: std::sync::Arc<Box<dyn zenclaw_agent::net::Nic>>,
    ) -> Self {
        Self {
            hostname,
            started: std::time::Instant::now(),
            nic,
        }
    }
}

#[cfg(feature = "esp32")]
impl zenclaw_agent::core::commands::HostFacts for Esp32HostFacts {
    fn hostname(&self) -> String {
        self.hostname.clone()
    }

    fn ip(&self) -> Option<String> {
        self.nic.ip_info().map(|i| i.ip.to_string())
    }

    fn link(&self) -> zenclaw_agent::core::commands::LinkKind {
        match self.nic.kind() {
            zenclaw_agent::net::NicKind::Wifi => {
                zenclaw_agent::core::commands::LinkKind::Wifi {
                    ssid: self.nic.ssid().unwrap_or_else(|| "?".to_string()),
                    rssi: self.nic.rssi(),
                }
            }
            zenclaw_agent::net::NicKind::Ethernet => {
                zenclaw_agent::core::commands::LinkKind::Ethernet
            }
        }
    }

    fn free_internal_heap(&self) -> Option<u32> {
        Some(unsafe { esp_idf_svc::sys::esp_get_free_heap_size() } as u32)
    }

    fn free_psram(&self) -> Option<u32> {
        let v = unsafe {
            esp_idf_svc::sys::heap_caps_get_free_size(esp_idf_svc::sys::MALLOC_CAP_SPIRAM)
        };
        // 0 → "no PSRAM" or "unsupported"; both surface as None.
        if v == 0 { None } else { Some(v as u32) }
    }

    fn uptime_secs(&self) -> u64 {
        self.started.elapsed().as_secs()
    }
}

#[cfg(feature = "esp32")]
fn main() {
    esp_idf_svc::sys::link_patches();
    log_ring::init();
    log::info!("=== ZenClaw ESP32 boot ===");

    // --- Status LED (WS2812; GPIO 40 — needs per-board pin mapping; benign no-op if unconnected) ---
    zenclaw_agent::led_status::init(40);
    zenclaw_agent::led_status::set(zenclaw_agent::led_status::State::Boot);

    // --- NVS (always needed for config) ---
    use esp_idf_svc::nvs::EspDefaultNvsPartition;
    let nvs = EspDefaultNvsPartition::take().unwrap();

    // --- Primary NIC (WiFi or Ethernet, selected by cargo features) ---
    let peripherals = esp_idf_svc::hal::peripherals::Peripherals::take().unwrap();
    let sysloop = esp_idf_svc::eventloop::EspSystemEventLoop::take().unwrap();

    let nic: Box<dyn zenclaw_agent::net::Nic> = match zenclaw_agent::net::bring_up_primary(
        peripherals,
        sysloop.clone(),
        nvs.clone(),
    ) {
        Ok(n) => {
            log::info!(
                "Primary NIC up: kind={:?} ip={:?}",
                n.kind(),
                n.ip_info().map(|i| i.ip),
            );
            zenclaw_agent::led_status::set(zenclaw_agent::led_status::State::Idle);
            n
        }
        Err(e) => {
            log::error!("NIC bring-up failed: {}", e);
            zenclaw_agent::led_status::set(zenclaw_agent::led_status::State::LinkFailed);
            loop { std::thread::sleep(std::time::Duration::from_secs(60)); }
        }
    };
    let nic = std::sync::Arc::new(nic);

    let ip_str = nic
        .ip_info()
        .map(|i| i.ip.to_string())
        .unwrap_or_else(|| "0.0.0.0".to_string());

    // --- mDNS ---
    #[cfg(any(esp_idf_comp_mdns_enabled, esp_idf_comp_espressif__mdns_enabled))]
    let hostname = {
        let h = resolve_hostname(&nvs);
        let mut mdns = esp_idf_svc::mdns::EspMdns::take().unwrap();
        mdns.set_hostname(&h).unwrap();
        mdns.set_instance_name("ZenClaw Agent").unwrap();
        mdns.add_service(None, "_http", "_tcp", 80, &[]).unwrap();
        log::info!("mDNS: {}.local", h);
        std::mem::forget(mdns);
        h
    };
    #[cfg(not(any(esp_idf_comp_mdns_enabled, esp_idf_comp_espressif__mdns_enabled)))]
    let hostname = {
        log::warn!("mDNS: not available (needs cargo clean && cargo build)");
        resolve_hostname(&nvs)
    };

    // --- SNTP (UTC clock sync, deferred) ---
    // R2/S3 SigV4 rejects requests with skew >15 min. Kick this off in a
    // background thread so a slow NTP handshake can never block boot.
    std::thread::Builder::new()
        .name("sntp-init".into())
        .stack_size(8192)
        .spawn(|| {
            std::thread::sleep(std::time::Duration::from_secs(2));
            match esp_idf_svc::sntp::EspSntp::new_default() {
                Ok(sntp) => {
                    log::info!("SNTP: started default service");
                    std::mem::forget(sntp);
                }
                Err(e) => log::warn!("SNTP: start failed: {}", e),
            }
        })
        .ok();

    // --- Load config ---
    let config = load_config(&nvs);
    log::info!("Config: agent={}, provider={}", config.agent_name, config.providers.default);

    // --- Mount LittleFS at /data ---
    // (Replaces SPIFFS — directories work, no readdir-on-empty-path hangs.)
    // Bitfield init via zeroed() + setter is bindgen-version-stable; the
    // alternative struct-literal form depends on internal `_bitfield_1`
    // field naming.
    let mut littlefs_conf: esp_idf_svc::sys::esp_vfs_littlefs_conf_t =
        unsafe { core::mem::zeroed() };
    littlefs_conf.base_path = b"/data\0".as_ptr() as *const core::ffi::c_char;
    littlefs_conf.partition_label = b"storage\0".as_ptr() as *const core::ffi::c_char;
    littlefs_conf.set_format_if_mount_failed(1);
    let ret = unsafe { esp_idf_svc::sys::esp_vfs_littlefs_register(&littlefs_conf) };
    if ret != 0 {
        log::error!("LittleFS mount failed: err={}", ret);
    } else {
        let mut total: usize = 0;
        let mut used: usize = 0;
        unsafe {
            esp_idf_svc::sys::esp_littlefs_info(
                b"storage\0".as_ptr() as *const core::ffi::c_char,
                &mut total,
                &mut used,
            );
        }
        log::info!("LittleFS mounted at /data ({}KB total, {}KB used)", total / 1024, used / 1024);
    }

    // --- USB mass storage (optional) ---
    #[cfg(feature = "usb_storage")]
    zenclaw_agent::usb_storage::init();

    // --- SD card (optional, P4 only today) ---
    // Failure is non-fatal: /sdcard simply remains unavailable and the
    // status payload reports `sdcard.mounted: false` so the web UI can
    // hide its tab.
    #[cfg(feature = "sdcard")]
    zenclaw_agent::sdcard::init();

    // --- Create gateway ---
    let data_dir = "/data";
    let _ = std::fs::create_dir_all(format!("{}/sessions", data_dir));
    let _ = std::fs::create_dir_all(format!("{}/memory", data_dir));
    zenclaw_agent::core::workspace::seed_defaults(data_dir);

    // --- Cloud bootstrap (only if storage is configured) -------------------
    // Build the cache + replicator, run boot_restore against S3, spawn the
    // drainer + snapshot timer. Cloud-disabled boot keeps the existing
    // local-file path with zero overhead.
    let cloud_handles = bootstrap_cloud(&config, data_dir, &hostname);

    let config_for_tg = config.clone();
    let config_arc = std::sync::Arc::new(config.clone());
    let runner = Box::new(zenclaw_agent::esp32::runner::EspRunner::new(config_arc));
    let host_facts: std::sync::Arc<dyn zenclaw_agent::core::commands::HostFacts> =
        std::sync::Arc::new(Esp32HostFacts::new(hostname.clone(), nic.clone()));
    let gateway = match cloud_handles {
        Some(h) => zenclaw_agent::core::gateway::Gateway::new_with_cloud(
            config, data_dir, runner, h, host_facts,
        ),
        None => zenclaw_agent::core::gateway::Gateway::new(
            config, data_dir, runner, host_facts,
        ),
    };
    let gateway = std::sync::Arc::new(gateway);

    // --- Chat request channel (httpd → agent thread) ---
    let (chat_tx, chat_rx) = std::sync::mpsc::channel::<ChatRequest>();
    let chat_tx = std::sync::Arc::new(std::sync::Mutex::new(chat_tx));

    // --- Start HTTP server ---
    start_http_server(gateway.clone(), nic.clone(), &ip_str, &hostname, nvs, chat_tx);
    zenclaw_agent::led_status::set(zenclaw_agent::led_status::State::Idle);

    // --- Construct shared HTTP client + optional Telegram resources ---
    let http: std::sync::Arc<dyn zenclaw_agent::platform::http_client::HttpClient> =
        std::sync::Arc::new(zenclaw_agent::esp32::http_client::EspHttpClient::new());

    let tg_resources = config_for_tg
        .channels
        .telegram
        .as_ref()
        .filter(|t| t.enabled && !t.bot_token.is_empty())
        .map(|t| {
            (
                zenclaw_agent::core::channels::telegram::Poller::new(t.bot_token.clone()),
                zenclaw_agent::core::channels::telegram::TelegramChannel::new(
                    t.bot_token.clone(),
                    http.clone(),
                ),
                t.allowed_chat_ids.clone(),
            )
        });

    // --- Start agent thread (handles both Telegram + HTTP chat) ---
    // 24KB. Was 32KB until cloud bootstrap added the drainer (8KB) +
    // snapshot timer (4KB) threads, which fragment internal SRAM
    // enough that a contiguous 32KB block isn't always available by
    // the time we get here (observed: cold boots after `/api/config`
    // strict_put + reboot, when boot_restore loads one extra cache
    // entry, push the heap over the edge → "Failed to spawn agent
    // thread: OutOfMemory"). 24KB matches the typical peak used by
    // agent_loop (LLM HTTPS handshake ~8KB + tool dispatch ~6KB +
    // serde_json + futures wrapper). Telegram poller threads spawned
    // inside agent_thread use 8KB stacks of their own.
    {
        let gw = gateway.clone();
        let http_for_thread = http.clone();
        std::thread::Builder::new()
            .name("agent".into())
            .stack_size(24 * 1024)
            .spawn(move || agent_thread(chat_rx, gw, http_for_thread, tg_resources))
            .expect("Failed to spawn agent thread");
        log::info!("Agent thread started");
    }

    loop { std::thread::sleep(std::time::Duration::from_secs(60)); }
}

#[cfg(feature = "esp32")]
fn nvs_get_string(
    nvs: &esp_idf_svc::nvs::EspNvs<esp_idf_svc::nvs::NvsDefault>,
    key: &str,
) -> Option<String> {
    // Try str first, fall back to blob (supports both provisioning methods)
    if let Ok(Some(len)) = nvs.str_len(key) {
        let mut buf = vec![0u8; len];
        if let Ok(Some(s)) = nvs.get_str(key, &mut buf) {
            return Some(s.to_string());
        }
    }
    if let Ok(Some(len)) = nvs.blob_len(key) {
        let mut buf = vec![0u8; len];
        if nvs.get_blob(key, &mut buf).is_ok() {
            return String::from_utf8(buf).ok();
        }
    }
    None
}

fn format_mac_suffix(mac: &[u8; 6]) -> String {
    format!("zenclaw-{:02x}{:02x}{:02x}", mac[3], mac[4], mac[5])
}

#[cfg(feature = "esp32")]
fn read_device_hostname(nvs: &esp_idf_svc::nvs::EspDefaultNvsPartition) -> Option<String> {
    let handle = esp_idf_svc::nvs::EspNvs::new(nvs.clone(), "device", false).ok()?;
    nvs_get_string(&handle, "hostname").filter(|s| !s.is_empty())
}

#[cfg(feature = "esp32")]
fn resolve_hostname(nvs: &esp_idf_svc::nvs::EspDefaultNvsPartition) -> String {
    if let Some(h) = read_device_hostname(nvs) {
        return h;
    }
    let mut mac = [0u8; 6];
    // SAFETY: esp_read_mac writes exactly 6 bytes into `mac`, which is sized [u8; 6].
    let err = unsafe {
        esp_idf_svc::sys::esp_read_mac(
            mac.as_mut_ptr(),
            esp_idf_svc::sys::esp_mac_type_t_ESP_MAC_WIFI_STA,
        )
    };
    if err != 0 {
        log::warn!("esp_read_mac failed: {} — using static fallback", err);
        return "zenclaw".to_string();
    }
    format_mac_suffix(&mac)
}

#[cfg(feature = "esp32")]
fn load_config(nvs: &esp_idf_svc::nvs::EspDefaultNvsPartition) -> zenclaw_agent::config::Config {
    // Read config JSON from NVS "config" namespace, key "json"
    match esp_idf_svc::nvs::EspNvs::new(nvs.clone(), "config", false) {
        Ok(handle) => {
            match nvs_get_string(&handle, "json") {
                Some(data) if !data.is_empty() => {
                    match serde_json::from_str(&data) {
                        Ok(c) => {
                            log::info!("Loaded config from NVS ({}B)", data.len());
                            return c;
                        }
                        Err(e) => log::error!("Invalid NVS config: {}", e),
                    }
                }
                _ => log::warn!("No config in NVS"),
            }
        }
        Err(e) => log::error!("NVS config namespace: {}", e),
    }
    log::info!("Using default config");
    serde_json::from_str("{}").unwrap()
}

#[cfg(feature = "esp32")]
fn save_config_nvs(nvs: &esp_idf_svc::nvs::EspDefaultNvsPartition, json: &str) -> Result<(), String> {
    let handle = esp_idf_svc::nvs::EspNvs::new(nvs.clone(), "config", true)
        .map_err(|e| format!("NVS open: {}", e))?;
    handle.set_blob("json", json.as_bytes())
        .map_err(|e| format!("NVS write: {}", e))?;
    Ok(())
}


#[cfg(feature = "esp32")]
fn read_temp(handle_val: usize) -> Option<f64> {
    let handle = handle_val as esp_idf_svc::sys::temperature_sensor_handle_t;
    let mut celsius: f32 = 0.0;
    let ret = unsafe { esp_idf_svc::sys::temperature_sensor_get_celsius(handle, &mut celsius) };
    if ret == 0 { Some(((celsius as f64) * 10.0).round() / 10.0) } else { None }
}

#[cfg(feature = "esp32")]
fn chip_label() -> &'static str {
    use esp_idf_svc::sys;
    let mut info: sys::esp_chip_info_t = unsafe { std::mem::zeroed() };
    unsafe { sys::esp_chip_info(&mut info) };
    match info.model {
        m if m == sys::esp_chip_model_t_CHIP_ESP32 => "ESP32",
        m if m == sys::esp_chip_model_t_CHIP_ESP32S2 => "ESP32-S2",
        m if m == sys::esp_chip_model_t_CHIP_ESP32S3 => "ESP32-S3",
        m if m == sys::esp_chip_model_t_CHIP_ESP32C3 => "ESP32-C3",
        m if m == sys::esp_chip_model_t_CHIP_ESP32C6 => "ESP32-C6",
        m if m == sys::esp_chip_model_t_CHIP_ESP32H2 => "ESP32-H2",
        m if m == sys::esp_chip_model_t_CHIP_ESP32P4 => "ESP32-P4",
        _ => "ESP32",
    }
}

/// Boot-time stash of `BootResult` so `/api/status.cloud_storage` can
/// surface warnings + heartbeat conflict without re-running the
/// restore. Set once during cloud bootstrap; never mutated after.
#[cfg(feature = "esp32")]
static CLOUD_BOOT_RESULT: std::sync::OnceLock<zenclaw_agent::core::cloud::BootResult> =
    std::sync::OnceLock::new();

/// Stand up the cloud-persistence machinery for this boot if storage is
/// configured. Returns `None` when cloud is disabled — the caller falls
/// back to local-file mode with no behavior change.
///
/// Side effects on the `Some` path:
/// - runs `boot_restore` (populates the cache from S3, applies L3/L4/L5
///   safety layers per chat, restores memory + cron + identity files);
/// - stashes the resulting `BootResult` in `CLOUD_BOOT_RESULT` for
///   `/api/status.cloud_storage` to surface;
/// - spawns the replicator drainer thread (32KB stack — TLS handshakes
///   dominate; smaller stacks blow up at handshake);
/// - spawns the snapshot timer thread (8KB stack — pure file IO).
///
/// Both spawned threads run for the lifetime of the process; we never
/// shut them down (the device only exits via reboot).
#[cfg(feature = "esp32")]
fn bootstrap_cloud(
    config: &zenclaw_agent::config::Config,
    data_dir: &str,
    hostname: &str,
) -> Option<zenclaw_agent::core::gateway::CloudHandles> {
    use std::sync::Arc;
    use std::time::Duration;
    use zenclaw_agent::core::cloud::{
        boot_restore, client::ObjectStore, client::S3Client, snapshots, BootConfig, CloudCache,
        Replicator, ReplicatorConfig,
    };

    if !config.is_cloud_enabled() {
        log::info!("Cloud: disabled (no storage block in config)");
        return None;
    }
    let storage = config.storage.as_ref().expect("is_cloud_enabled checked");
    let s3 = S3Client::from_config(storage)?;
    let store: Arc<dyn ObjectStore> = Arc::new(s3);

    let cache = CloudCache::new();

    // Step 1: boot restore. On hard failure (top-level LIST blew up),
    // log and continue with an empty cache — better to come up with no
    // history than to refuse boot. Per-chat failures are absorbed into
    // the BootResult.warnings vec and surface on /api/status.
    let boot_cfg = BootConfig {
        session_max_bytes: storage.session_max_bytes,
        log_compaction_bytes: storage.log_compaction_bytes,
        device_id: hostname.to_string(),
        heartbeat_stale_secs: 3600,
        sessions_dir: Some(format!("{}/sessions", data_dir)),
    };
    match boot_restore(&store, &cache, &boot_cfg) {
        Ok(result) => {
            log::info!(
                "Cloud boot_restore: {} warning(s), heartbeat_conflict={:?}, cache={} keys",
                result.warnings.len(),
                result.heartbeat_conflict,
                cache.snapshot().len()
            );
            let _ = CLOUD_BOOT_RESULT.set(result);
        }
        Err(e) => {
            log::warn!("Cloud boot_restore failed (continuing with empty cache): {}", e);
            // Try snapshot fallback before giving up.
            let snap_path = format!("{}/.snapshot.bin", data_dir);
            match snapshots::read_from(&snap_path) {
                Ok(Some(snap)) => {
                    log::info!(
                        "Cloud: restored from snapshot ({} keys, written_at={})",
                        snap.entries.len(),
                        snap.written_at
                    );
                    cache.restore_from(snap.entries);
                }
                Ok(None) => log::info!("Cloud: no snapshot found"),
                Err(e2) => log::warn!("Cloud: snapshot read failed: {}", e2),
            }
        }
    }

    // Step 2: replicator + drainer.
    let replicator = Arc::new(Replicator::new(ReplicatorConfig::from(&storage.replicator)));
    let _drainer = replicator.spawn_drainer(store.clone());
    log::info!(
        "Cloud: drainer thread spawned (queue_max={}, retry_max={})",
        storage.replicator.queue_max,
        storage.replicator.retry_max
    );

    // Step 3: snapshot timer thread. 4 KiB stack — the loop body is
    // pure file IO via snapshots::write_to (length-prefixed binary
    // serializer + atomic rename), no TLS, no LLM. 8 KiB was over-
    // provisioned and contributed to the agent-thread OOM during cold
    // boots after a /api/config strict_put + reboot.
    let cache_for_snap = cache.clone();
    let snap_path = format!("{}/.snapshot.bin", data_dir);
    let snap_interval = Duration::from_secs(storage.snapshot.interval_secs as u64);
    std::thread::Builder::new()
        .name("cloud-snap".into())
        .stack_size(4 * 1024)
        .spawn(move || loop {
            std::thread::sleep(snap_interval);
            if let Err(e) = snapshots::write_to(&cache_for_snap, &snap_path) {
                log::warn!("snapshot write failed: {}", e);
            }
        })
        .expect("Failed to spawn snapshot thread");
    log::info!("Cloud: snapshot timer thread spawned (interval={}s)", storage.snapshot.interval_secs);

    Some(zenclaw_agent::core::gateway::CloudHandles {
        cache,
        replicator,
        store,
        log_compaction_bytes: storage.log_compaction_bytes,
        retry_max: storage.replicator.retry_max,
        backoff_cap_secs: storage.replicator.backoff_cap_secs,
    })
}

/// No-op stub for non-esp32 builds (desktop). Cloud bootstrap is
/// ESP32-only because S3Client itself is `#[cfg(feature = "esp32")]`.
#[cfg(not(feature = "esp32"))]
#[allow(dead_code)]
fn bootstrap_cloud(
    _config: &zenclaw_agent::config::Config,
    _data_dir: &str,
    _hostname: &str,
) -> Option<zenclaw_agent::core::gateway::CloudHandles> {
    None
}

#[cfg(feature = "esp32")]
fn get_query_param(uri: &str, key: &str) -> Option<String> {
    let query = uri.split('?').nth(1)?;
    for pair in query.split('&') {
        let mut kv = pair.splitn(2, '=');
        if let (Some(k), Some(v)) = (kv.next(), kv.next()) {
            if k == key { return Some(url_decode(v)); }
        }
    }
    None
}

/// Confine `/api/files*` to the supported writable mounts: `/data`
/// (LittleFS, always present) and — when the board has the `sdcard`
/// feature enabled and a card actually mounted — `/sdcard` (FATFS).
///
/// Empty input maps to `/data` so a path-less `GET /api/files` lists the
/// LittleFS root (back-compat with the original device-mode contract).
/// Any explicit path must already start with one of the mount roots and
/// contain no `..` segments — matching the relative-to-data-root jail
/// that the desktop server enforces via `safe_join`.
///
/// `/sdcard` requests are rejected (with a distinct error) when the SD
/// is unmounted so the web UI can surface the cause to the user.
#[cfg(feature = "esp32")]
fn jail_filesystem_path(input: &str) -> Result<String, &'static str> {
    let candidate = if input.is_empty() || input == "/" {
        "/data"
    } else {
        input
    };
    if candidate.split('/').any(|seg| seg == "..") {
        return Err("path escapes mount root");
    }
    if candidate == "/data" || candidate.starts_with("/data/") {
        return Ok(candidate.to_string());
    }
    if candidate == "/sdcard" || candidate.starts_with("/sdcard/") {
        #[cfg(feature = "sdcard")]
        {
            if zenclaw_agent::sdcard::is_mounted() {
                return Ok(candidate.to_string());
            }
            return Err("sdcard not mounted");
        }
        #[cfg(not(feature = "sdcard"))]
        {
            return Err("sdcard not supported on this board");
        }
    }
    Err("path must be under /data or /sdcard")
}

/// True when `path` points at the root of a writable mount. Used by file
/// handlers that refuse to act on a bare mount root (empty path was a
/// likely client bug, and deleting a mount root would be catastrophic).
#[cfg(feature = "esp32")]
fn is_mount_root(path: &str) -> bool {
    path == "/data" || path == "/sdcard"
}

/// Cache for the `/api/status.cloud_storage` block. R2 LIST is rate-
/// metered and we don't want to hit it on every status poll; the
/// MicroPython side caches this for 60s and we match.
#[cfg(feature = "esp32")]
static CLOUD_STATUS_CACHE: std::sync::Mutex<Option<(std::time::Instant, serde_json::Value)>> =
    std::sync::Mutex::new(None);

#[cfg(feature = "esp32")]
fn cloud_status_block(
    gw: &std::sync::Arc<zenclaw_agent::core::gateway::Gateway>,
) -> Option<serde_json::Value> {
    use zenclaw_agent::core::cloud::client::S3Client;
    let cfg = gw.config.as_ref();
    let storage = cfg.storage.as_ref()?;
    if !storage.is_cloud_configured() {
        return None;
    }

    {
        let cache = CLOUD_STATUS_CACHE.lock().ok()?;
        if let Some((ts, val)) = cache.as_ref() {
            if ts.elapsed() < std::time::Duration::from_secs(60) {
                // Re-attach the live (uncached) sync/boot/heartbeat
                // sub-blocks so the UI reflects the queue depth and
                // dead-letter state in real time.
                let mut v = val.clone();
                if let Some(obj) = v.as_object_mut() {
                    obj.insert("sync".to_string(), sync_block(gw));
                    obj.insert("boot".to_string(), boot_block());
                }
                return Some(v);
            }
        }
    }

    let provider = storage.provider();
    let bucket = storage.bucket.clone().unwrap_or_default();

    let (mut block, cache_ok) = match S3Client::from_config(storage) {
        Some(client) => match client.list("", None, 1000) {
            Ok(listing) => {
                let total: u64 = listing.objects.iter().map(|o| o.size).sum();
                let v = serde_json::json!({
                    "configured": true,
                    "enabled": true,
                    "provider": provider,
                    "bucket": bucket,
                    "endpoint": storage.endpoint.clone().unwrap_or_default(),
                    "region": storage.region,
                    "objects": listing.objects.len(),
                    "total_bytes": total,
                });
                (v, true)
            }
            Err(e) => (
                serde_json::json!({
                    "configured": true,
                    "enabled": true,
                    "provider": provider,
                    "bucket": bucket,
                    "error": e.to_string(),
                }),
                false,
            ),
        },
        None => (
            serde_json::json!({
                "configured": true,
                "enabled": true,
                "provider": provider,
                "bucket": bucket,
                "error": "client init failed",
            }),
            false,
        ),
    };

    // Live sync/boot blocks always overlay (not subject to 60s cache).
    if let Some(obj) = block.as_object_mut() {
        obj.insert("sync".to_string(), sync_block(gw));
        obj.insert("boot".to_string(), boot_block());
    }

    // Only cache successful results — caching transient failures (e.g.
    // RequestTimeTooSkewed before SNTP syncs) would freeze them in for 60s.
    if cache_ok {
        if let Ok(mut cache) = CLOUD_STATUS_CACHE.lock() {
            *cache = Some((std::time::Instant::now(), block.clone()));
        }
    }
    Some(block)
}

/// Builds the JSON payload returned on `/api/status` and pushed every 5s
/// over `/ws/stats`. The two transports must serve the **same shape** so
/// the web client can use a single `setStatus` (full replace) handler
/// regardless of where the message arrived from.
///
/// Includes everything the UI displays: live metrics (memory, temp, wifi,
/// storage, uptime), config-derived identity (provider, model, board,
/// platform, agent_name), and capability flags
/// (channels.telegram.has_token, cloud_storage). Secrets (api keys,
/// tokens, secret access keys) are never present — they live in
/// `/api/config` and are only fetched on explicit GET.
///
/// Cloud-storage stats are 60s-cached server-side (see
/// `cloud_status_block`), so the cost of the 5s push remains bounded.
///
/// See `docs/superpowers/specs/2026-05-03-stats-transport-model.md`.
#[cfg(feature = "esp32")]
fn build_status_payload(
    gw: &std::sync::Arc<zenclaw_agent::core::gateway::Gateway>,
    nic: &std::sync::Arc<Box<dyn zenclaw_agent::net::Nic>>,
    temp_handle: usize,
) -> serde_json::Value {
    let heap_free = unsafe { esp_idf_svc::sys::esp_get_free_heap_size() } as usize;
    let heap_total = unsafe { esp_idf_svc::sys::heap_caps_get_total_size(4096) }; // MALLOC_CAP_DEFAULT
    let uptime_us = unsafe { esp_idf_svc::sys::esp_timer_get_time() };
    let mut fs_total: usize = 0;
    let mut fs_used: usize = 0;
    unsafe {
        esp_idf_svc::sys::esp_littlefs_info(
            b"storage\0".as_ptr() as *const core::ffi::c_char,
            &mut fs_total,
            &mut fs_used,
        );
    }
    let info = nic.ip_info();
    let nic_kind_str = match nic.kind() {
        zenclaw_agent::net::NicKind::Wifi => "wifi",
        zenclaw_agent::net::NicKind::Ethernet => "ethernet",
    };
    let mac = nic.mac();
    let mac_str = format!(
        "{:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
        mac[0], mac[1], mac[2], mac[3], mac[4], mac[5],
    );
    let is_wifi = nic.kind() == zenclaw_agent::net::NicKind::Wifi;
    let mut body = serde_json::json!({
        "agent_name": gw.config.agent_name,
        "version": env!("CARGO_PKG_VERSION"),
        "platform": chip_label(),
        "board": env!("ZENCLAW_BOARD"),
        "memory": {
            "free_kb": heap_free / 1024,
            "total_kb": heap_total / 1024,
            "used_kb": heap_total.saturating_sub(heap_free) / 1024,
        },
        "temperature_c": read_temp(temp_handle),
        "network": {
            "kind": nic_kind_str,
            "ip": info.map(|i| i.ip.to_string()),
            "link_speed_mbps": nic.link_speed_mbps(),
            "mac": mac_str,
        },
        "wifi": {
            "connected": is_wifi && nic.link_up(),
            "ip": if is_wifi { info.map(|i| i.ip.to_string()) } else { None },
            // SSID is whatever the NIC reports. WifiNic caches it at
            // construction (no NVS read on the hot path); EthNic returns
            // None (Ethernet has no SSID). Reading NVS here every status
            // push was both useless on WiFi and misleading on Ethernet
            // (would surface a phantom SSID from a prior wizard flash).
            "ssid": nic.ssid(),
            "rssi": nic.rssi(),
            "driver": zenclaw_agent::net::wifi_ui::driver_label(),
        },
        "storage": {
            "total_kb": fs_total / 1024,
            "free_kb": fs_total.saturating_sub(fs_used) / 1024,
        },
        "sdcard": sdcard_status_block(),
        "channels": {
            "telegram": {
                "configured": gw.config.channels.telegram.is_some(),
                "enabled": gw.config.channels.telegram.as_ref().map_or(false, |t| t.enabled),
                "has_token": gw.config.channels.telegram.as_ref().map_or(false, |t| !t.bot_token.is_empty()),
            },
        },
        "provider": gw.config.providers.default,
        "model": gw.config.providers.entries.get(&gw.config.providers.default)
            .and_then(|e| e.model.as_deref())
            .unwrap_or(""),
        "usb": {
            "mounted": cfg!(feature = "usb_storage") && {
                #[cfg(feature = "usb_storage")]
                { zenclaw_agent::usb_storage::is_mounted() }
                #[cfg(not(feature = "usb_storage"))]
                { false }
            },
            "path": if cfg!(feature = "usb_storage") { "/usb" } else { "" },
        },
        "uptime_s": uptime_us / 1_000_000
    });
    if let Some(cloud) = cloud_status_block(gw) {
        body["cloud_storage"] = cloud;
    }
    body
}

/// Live replicator state — queue depth, dead-letter count, last-sync
/// age. Kept out of the 60s LIST cache so the UI reflects writes in
/// real time.
#[cfg(feature = "esp32")]
fn sync_block(gw: &std::sync::Arc<zenclaw_agent::core::gateway::Gateway>) -> serde_json::Value {
    use zenclaw_agent::core::cloud::DeadLetterEntry;
    let Some(rep) = gw.cloud_replicator.as_ref() else {
        return serde_json::json!({ "active": false });
    };
    let dl: Vec<DeadLetterEntry> = rep.dead_letter();
    let last_sync_age = rep
        .last_sync_at()
        .map(|i| i.elapsed().as_secs() as i64)
        .unwrap_or(-1);
    serde_json::json!({
        "active": true,
        "queue_depth": rep.queue_depth(),
        "queue_max": gw.config.storage.as_ref()
            .map(|s| s.replicator.queue_max).unwrap_or(0),
        "last_sync_age_secs": last_sync_age,
        "dead_letter_count": dl.len(),
        "failures": dl.iter().take(10).map(|e| serde_json::json!({
            "key": e.key,
            "retry_count": e.retry_count,
            "last_error": e.last_error_msg,
        })).collect::<Vec<_>>(),
    })
}

/// `/api/status.sdcard` block. Always present so the JSON shape is the
/// same across boards — boards without the `sdcard` feature (DevKitC)
/// just report `mounted: false` and the web UI hides its tab.
#[cfg(feature = "esp32")]
fn sdcard_status_block() -> serde_json::Value {
    #[cfg(feature = "sdcard")]
    {
        if zenclaw_agent::sdcard::is_mounted() {
            let (total, free) = zenclaw_agent::sdcard::info().unwrap_or((0, 0));
            return serde_json::json!({
                "mounted": true,
                "path": "/sdcard",
                "total_kb": total / 1024,
                "free_kb": free / 1024,
                "type": zenclaw_agent::sdcard::type_str(),
                "bus_width": zenclaw_agent::sdcard::bus_width(),
            });
        }
    }
    serde_json::json!({ "mounted": false })
}

/// Boot warnings + heartbeat conflict captured at startup. Stable for
/// the lifetime of the process — set once in `bootstrap_cloud`.
#[cfg(feature = "esp32")]
fn boot_block() -> serde_json::Value {
    let Some(result) = CLOUD_BOOT_RESULT.get() else {
        return serde_json::json!({ "ran": false });
    };
    serde_json::json!({
        "ran": true,
        "warnings": result.warnings,
        "heartbeat_conflict": result.heartbeat_conflict,
    })
}

#[cfg(feature = "esp32")]
fn url_decode(s: &str) -> String {
    let mut out = Vec::with_capacity(s.len());
    let b = s.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let Ok(byte) = u8::from_str_radix(
                std::str::from_utf8(&b[i + 1..i + 3]).unwrap_or(""), 16,
            ) {
                out.push(byte);
                i += 3;
                continue;
            }
        }
        out.push(if b[i] == b'+' { b' ' } else { b[i] });
        i += 1;
    }
    String::from_utf8(out).unwrap_or_default()
}

#[cfg(feature = "esp32")]
use esp_idf_svc::io::Write as _;

#[cfg(feature = "esp32")]
const CORS_HEADERS: &[(&str, &str)] = &[
    ("Content-Type", "application/json"),
    ("Access-Control-Allow-Origin", "*"),
];

#[cfg(feature = "esp32")]
fn start_http_server(
    gateway: std::sync::Arc<zenclaw_agent::core::gateway::Gateway>,
    nic: std::sync::Arc<Box<dyn zenclaw_agent::net::Nic>>,
    ip_str: &str,
    hostname: &str,
    nvs: esp_idf_svc::nvs::EspDefaultNvsPartition,
    chat_tx: std::sync::Arc<std::sync::Mutex<std::sync::mpsc::Sender<ChatRequest>>>,
) {
    use esp_idf_svc::http::server::{Configuration as HttpConfig, EspHttpServer};
    use esp_idf_svc::http::Method;
    use zenclaw_agent::led_status::{self, State as Led};

    let mut server = EspHttpServer::new(&HttpConfig {
        http_port: 80,
        stack_size: 16384,
        // We register ~36 handlers (REST + OPTIONS preflights + WS). Leave
        // headroom for future routes; the per-handler memory cost is tiny.
        max_uri_handlers: 64,
        // Required for /api/sessions/* and any future wildcard routes.
        // Without this, ESP-IDF httpd uses exact string matching only, so
        // the literal '*' in a pattern never matches a real URI segment.
        uri_match_wildcard: true,
        ..Default::default()
    }).unwrap();

    // --- Temperature sensor (init once, read via handle) ---
    let temp_handle = {
        use esp_idf_svc::sys::*;
        let config = temperature_sensor_config_t {
            range_min: -10,
            range_max: 80,
            ..Default::default()
        };
        let mut handle: temperature_sensor_handle_t = std::ptr::null_mut();
        unsafe {
            temperature_sensor_install(&config, &mut handle);
            temperature_sensor_enable(handle);
        }
        handle as usize
    };

    // --- CORS preflight (OPTIONS for all /api/* paths) ---
    for path in &[
        "/api/status", "/api/chat", "/api/chat/history", "/api/config", "/api/wifi",
        "/api/files", "/api/files/read", "/api/files/write", "/api/files/mkdir",
        "/api/files/upload", "/api/restart", "/api/cloud/files", "/api/cloud/sign",
        "/api/sessions", "/api/sessions/*",
    ] {
        server.fn_handler::<anyhow::Error, _>(path, Method::Options, |req| {
            let mut resp = req.into_response(204, None, &[
                ("Access-Control-Allow-Origin", "*"),
                ("Access-Control-Allow-Methods", "GET, POST, PUT, PATCH, DELETE, OPTIONS"),
                ("Access-Control-Allow-Headers", "Content-Type"),
                ("Access-Control-Max-Age", "86400"),
            ])?;
            resp.write_all(&[])?;
            Ok(())
        }).unwrap();
    }

    // --- / (landing page) ---
    let gw_root = gateway.clone();
    let ip_owned = ip_str.to_string();
    server.fn_handler::<anyhow::Error, _>("/", Method::Get, move |req| {
        let heap = unsafe { esp_idf_svc::sys::esp_get_free_heap_size() };
        let name = &gw_root.config.agent_name;
        let html = format!(
            r#"<!DOCTYPE html>
<html><head><meta charset="utf-8"><meta name="viewport" content="width=device-width">
<title>{name}</title>
<style>
*{{margin:0;padding:0;box-sizing:border-box}}
body{{font-family:system-ui,sans-serif;background:#0a0a0a;color:#e0e0e0;display:flex;justify-content:center;align-items:center;min-height:100vh}}
.c{{max-width:420px;width:100%;padding:2rem}}
h1{{font-size:1.5rem;margin-bottom:.5rem}}
.sub{{color:#888;margin-bottom:1.5rem}}
.stat{{display:flex;justify-content:space-between;padding:.5rem 0;border-bottom:1px solid #222}}
.stat:last-child{{border:none}}
.label{{color:#888}}
a{{color:#60a5fa;text-decoration:none}}
</style></head><body><div class="c">
<h1>{name}</h1>
<p class="sub">{chip} &middot; v{ver}</p>
<div class="stat"><span class="label">IP</span><span>{ip}</span></div>
<div class="stat"><span class="label">Heap free</span><span>{heap}KB</span></div>
<div class="stat"><span class="label">API</span><a href="/api/status">/api/status</a></div>
<div class="stat"><span class="label">Chat</span><span>POST /api/chat</span></div>
</div></body></html>"#,
            name = name,
            chip = chip_label(),
            ver = env!("CARGO_PKG_VERSION"),
            ip = ip_owned,
            heap = heap / 1024,
        );
        let mut resp = req.into_response(200, None, &[
            ("Content-Type", "text/html; charset=utf-8"),
        ])?;
        resp.write_all(html.as_bytes())?;
        Ok(())
    }).unwrap();

    // --- /api/status ---
    //
    // Both this handler and the `/ws/stats` push thread call
    // `build_status_payload` so the JSON shape served on either transport
    // is identical. See docs/superpowers/specs/2026-05-03-stats-transport-model.md.
    let gw = gateway.clone();
    let nic_for_status = nic.clone();
    let th = temp_handle;
    server.fn_handler::<anyhow::Error, _>("/api/status", Method::Get, move |req| {
        let body = build_status_payload(&gw, &nic_for_status, th);
        let body_str = body.to_string();
        let mut resp = req.into_response(200, None, CORS_HEADERS)?;
        resp.write_all(body_str.as_bytes())?;
        Ok(())
    }).unwrap();

    // --- /api/chat (POST) ---
    // Routes through the agent worker thread (32KB stack) to avoid stack overflow
    // in the httpd's 16KB task. The httpd handler just parses, dispatches, and waits.
    let chat_tx_clone = chat_tx.clone();
    server.fn_handler::<anyhow::Error, _>("/api/chat", Method::Post, move |mut req| {
        let (message, chat_id) = {
            let mut buf = [0u8; 512];
            let mut body = Vec::new();
            loop {
                let n = req.read(&mut buf)?;
                if n == 0 { break; }
                body.extend_from_slice(&buf[..n]);
            }
            let parsed: serde_json::Value = serde_json::from_slice(&body)
                .unwrap_or_default();
            drop(body);
            let msg = parsed.get("message")
                .and_then(|m| m.as_str())
                .unwrap_or("")
                .to_string();
            let cid = parsed.get("chat_id")
                .and_then(|c| c.as_str())
                .unwrap_or("web")
                .to_string();
            (msg, cid)
        };

        if message.is_empty() {
            let err = serde_json::json!({"error": "JSON body with message required"}).to_string();
            let mut resp = req.into_response(400, None, &[
                ("Content-Type", "application/json"),
            ])?;
            resp.write_all(err.as_bytes())?;
            return Ok(());
        }

        log::info!("Chat: chat_id={} msg_len={}", chat_id, message.len());

        // Send to agent worker thread and wait for result
        let (reply_tx, reply_rx) = std::sync::mpsc::channel();
        {
            let tx = chat_tx_clone.lock().unwrap();
            tx.send(ChatRequest {
                chat_id,
                message,
                reply_tx: Some(reply_tx),
                events_tx: None,
            })
                .map_err(|e| anyhow::anyhow!("send to agent worker: {}", e))?;
        }
        let result = reply_rx.recv()
            .map_err(|e| anyhow::anyhow!("agent worker recv: {}", e))?;

        let resp_body = match result {
            Ok(reply) => serde_json::json!({"reply": reply}),
            Err(e) => {
                log::error!("Chat error: {}", e);
                serde_json::json!({"error": e})
            }
        };
        let resp_str = resp_body.to_string();
        let mut resp = req.into_response(200, None, CORS_HEADERS)?;
        resp.write_all(resp_str.as_bytes())?;
        Ok(())
    }).unwrap();

    // --- /api/chat/history (GET) ---
    let gw = gateway.clone();
    server.fn_handler::<anyhow::Error, _>("/api/chat/history", Method::Get, move |req| {
        use zenclaw_agent::core::chat_events::ChatEvent;
        use zenclaw_agent::core::sessions::SessionEntry;
        use zenclaw_agent::core::types::Role;

        let uri = req.uri();
        let chat_id = uri.split("chat_id=")
            .nth(1)
            .and_then(|s| s.split('&').next())
            .unwrap_or("web");

        let branch = gw.sessions.get_branch(chat_id).unwrap_or_default();

        // Synthesize the same `ChatEvent` stream a live turn would produce.
        // Lossy on tool ok/err (the JSONL doesn't record an explicit success
        // flag — historical tool finishes always emit `ok: true`).
        let mut events: Vec<ChatEvent> = Vec::new();
        for entry in &branch {
            let SessionEntry::Message { role, content, tool_calls, tool_call_id, .. } = entry else {
                continue;
            };
            match role {
                Role::User => {
                    if !content.is_empty() {
                        events.push(ChatEvent::UserMessage {
                            chat_id: chat_id.to_string(),
                            text: content.clone(),
                        });
                    }
                }
                Role::Assistant => {
                    if let Some(calls) = tool_calls {
                        for tc in calls {
                            let args: serde_json::Value =
                                serde_json::from_str(&tc.function.arguments)
                                    .unwrap_or(serde_json::Value::Null);
                            events.push(ChatEvent::ToolCallStarted {
                                id: tc.id.clone(),
                                name: tc.function.name.clone(),
                                args,
                            });
                        }
                    } else if !content.is_empty() {
                        events.push(ChatEvent::AssistantText {
                            text: content.clone(),
                            is_final: true,
                        });
                    }
                }
                Role::Tool => {
                    if let Some(id) = tool_call_id {
                        events.push(ChatEvent::ToolCallFinished {
                            id: id.clone(),
                            ok: true,
                            result: Some(content.clone()),
                            error: None,
                        });
                    }
                }
                Role::System => {}
            }
        }

        // Cap to last N events. 200 is generous (≈ 40 user turns with
        // mid-single-digit tool calls each); the wire format is JSON so
        // the cap is mostly about response size, not memory.
        const MAX_EVENTS: usize = 200;
        if events.len() > MAX_EVENTS {
            let start = events.len() - MAX_EVENTS;
            events = events.into_iter().skip(start).collect();
        }

        let body = serde_json::json!({"events": events}).to_string();
        let mut resp = req.into_response(200, None, CORS_HEADERS)?;
        resp.write_all(body.as_bytes())?;
        Ok(())
    }).unwrap();

    // --- /api/config (GET) ---
    let nvs_r = nvs.clone();
    server.fn_handler::<anyhow::Error, _>("/api/config", Method::Get, move |req| {
        let body = match esp_idf_svc::nvs::EspNvs::new(nvs_r.clone(), "config", false) {
            Ok(handle) => nvs_get_string(&handle, "json").unwrap_or_else(|| "{}".to_string()),
            Err(_) => "{}".to_string(),
        };
        let mut resp = req.into_response(200, None, CORS_HEADERS)?;
        resp.write_all(body.as_bytes())?;
        Ok(())
    }).unwrap();

    // --- /api/config (PUT) ---
    let nvs_w = nvs.clone();
    server.fn_handler::<anyhow::Error, _>("/api/config", Method::Put, move |mut req| {
        let mut buf = [0u8; 4096];
        let mut body = Vec::new();
        loop {
            let n = req.read(&mut buf)?;
            if n == 0 { break; }
            body.extend_from_slice(&buf[..n]);
        }
        match serde_json::from_slice::<serde_json::Value>(&body) {
            Ok(config) => {
                let json = serde_json::to_string(&config).unwrap();

                // Strict S3 PUT happens BEFORE the NVS write so a cloud
                // failure leaves the device's running state intact. We
                // only attempt it when the new config typed-parses as
                // Config AND has cloud configured — pushing a non-Config
                // shape (or disabling cloud) skips the strict path.
                if let Ok(typed_cfg) = serde_json::from_slice::<zenclaw_agent::config::Config>(&body) {
                    if typed_cfg.is_cloud_enabled() {
                        use zenclaw_agent::core::cloud::client::S3Client;
                        let storage = typed_cfg.storage.as_ref().expect("is_cloud_enabled checked");
                        match S3Client::from_config(storage) {
                            Some(client) => {
                                use zenclaw_agent::core::cloud::client::ObjectStore;
                                let store: std::sync::Arc<dyn ObjectStore> = std::sync::Arc::new(client);
                                if let Err(e) = zenclaw_agent::core::cloud::strict::strict_put(
                                    &store,
                                    "sys/config.json",
                                    json.as_bytes(),
                                    storage.replicator.retry_max,
                                    storage.replicator.backoff_cap_secs,
                                ) {
                                    log::warn!("Config strict_put failed; aborting reboot: {}", e);
                                    let err_body = serde_json::json!({
                                        "error": format!("Cloud strict_put failed: {}", e),
                                    }).to_string();
                                    let mut resp = req.into_response(503, None, CORS_HEADERS)?;
                                    resp.write_all(err_body.as_bytes())?;
                                    return Ok(());
                                }
                                log::info!("Config strict_put OK ({}B → sys/config.json)", json.len());
                            }
                            None => {
                                // is_cloud_enabled() guarantees the four required
                                // fields are present, so client construction
                                // failing here means an internal error. Surface it.
                                let err_body = serde_json::json!({
                                    "error": "Cloud client construction failed despite is_cloud_enabled",
                                }).to_string();
                                let mut resp = req.into_response(503, None, CORS_HEADERS)?;
                                resp.write_all(err_body.as_bytes())?;
                                return Ok(());
                            }
                        }
                    }
                }

                save_config_nvs(&nvs_w, &json).map_err(|e| anyhow::anyhow!(e))?;
                log::info!("Config saved to NVS ({}B), restarting...", json.len());
                let resp_body = serde_json::json!({"ok": true}).to_string();
                let mut resp = req.into_response(200, None, CORS_HEADERS)?;
                resp.write_all(resp_body.as_bytes())?;
                // Restart so gateway picks up new config
                led_status::set(Led::Updating);
                std::thread::spawn(|| {
                    std::thread::sleep(std::time::Duration::from_millis(500));
                    unsafe { esp_idf_svc::sys::esp_restart(); }
                });
            }
            Err(e) => {
                let err = serde_json::json!({"error": format!("Invalid JSON: {}", e)}).to_string();
                let mut resp = req.into_response(400, None, &[
                    ("Content-Type", "application/json"),
                ])?;
                resp.write_all(err.as_bytes())?;
            }
        }
        Ok(())
    }).unwrap();

    // --- /api/cloud/files (GET) — directory-style listing of R2 bucket ---
    let gw_cloud_list = gateway.clone();
    server.fn_handler::<anyhow::Error, _>("/api/cloud/files", Method::Get, move |req| {
        use zenclaw_agent::core::cloud::client::S3Client;
        let prefix = get_query_param(req.uri(), "prefix").unwrap_or_default();

        let client = match gw_cloud_list
            .config
            .storage
            .as_ref()
            .and_then(S3Client::from_config)
        {
            Some(c) => c,
            None => {
                let body = r#"{"error":"cloud storage not configured"}"#;
                let mut resp = req.into_response(503, None, CORS_HEADERS)?;
                resp.write_all(body.as_bytes())?;
                return Ok(());
            }
        };

        let body = match client.list(&prefix, Some("/"), 1000) {
            Ok(listing) => {
                let mut entries: Vec<serde_json::Value> = Vec::new();
                for cp in &listing.common_prefixes {
                    // Strip the prefix to get the trailing dir name.
                    let display = cp
                        .strip_prefix(&prefix as &str)
                        .unwrap_or(cp)
                        .trim_end_matches('/');
                    entries.push(serde_json::json!({
                        "name": display,
                        "path": cp,
                        "is_dir": true,
                        "size": serde_json::Value::Null,
                    }));
                }
                for obj in &listing.objects {
                    let display = obj
                        .key
                        .strip_prefix(&prefix as &str)
                        .unwrap_or(&obj.key);
                    if display.is_empty() { continue; } // S3 sometimes returns the prefix itself
                    entries.push(serde_json::json!({
                        "name": display,
                        "path": obj.key,
                        "is_dir": false,
                        "size": obj.size,
                    }));
                }
                serde_json::json!({"prefix": prefix, "entries": entries}).to_string()
            }
            Err(e) => {
                serde_json::json!({"error": e.to_string()}).to_string()
            }
        };
        let mut resp = req.into_response(200, None, CORS_HEADERS)?;
        resp.write_all(body.as_bytes())?;
        Ok(())
    }).unwrap();

    // --- /api/cloud/sign (GET) — presigned URL for browser-direct R2 ops ---
    let gw_cloud_sign = gateway.clone();
    server.fn_handler::<anyhow::Error, _>("/api/cloud/sign", Method::Get, move |req| {
        use zenclaw_agent::core::cloud::client::S3Client;
        let method = get_query_param(req.uri(), "method").unwrap_or_default().to_uppercase();
        let key = get_query_param(req.uri(), "key").unwrap_or_default();

        if !matches!(method.as_str(), "GET" | "PUT" | "DELETE") {
            let body = r#"{"error":"method must be GET, PUT or DELETE"}"#;
            let mut resp = req.into_response(400, None, CORS_HEADERS)?;
            resp.write_all(body.as_bytes())?;
            return Ok(());
        }
        if key.is_empty() {
            let body = r#"{"error":"missing key"}"#;
            let mut resp = req.into_response(400, None, CORS_HEADERS)?;
            resp.write_all(body.as_bytes())?;
            return Ok(());
        }

        let client = match gw_cloud_sign
            .config
            .storage
            .as_ref()
            .and_then(S3Client::from_config)
        {
            Some(c) => c,
            None => {
                let body = r#"{"error":"cloud storage not configured"}"#;
                let mut resp = req.into_response(503, None, CORS_HEADERS)?;
                resp.write_all(body.as_bytes())?;
                return Ok(());
            }
        };

        // Match the MicroPython side's hardcoded 15-minute expiry.
        let url = client.presign(&method, &key, 900);
        let body = serde_json::json!({"url": url, "method": method, "key": key}).to_string();
        let mut resp = req.into_response(200, None, CORS_HEADERS)?;
        resp.write_all(body.as_bytes())?;
        Ok(())
    }).unwrap();

    // --- /api/wifi (GET) ---
    let nic_for_wifi_get = nic.clone();
    let nvs_for_wifi_get = nvs.clone();
    server.fn_handler::<anyhow::Error, _>("/api/wifi", Method::Get, move |req| {
        let creds = zenclaw_agent::net::wifi_ui::read_credentials(&nvs_for_wifi_get);
        let body = serde_json::json!({
            "connected": nic_for_wifi_get.kind() == zenclaw_agent::net::NicKind::Wifi && nic_for_wifi_get.link_up(),
            "ssid": creds.as_ref().map(|(s, _)| s.clone()).or_else(|| nic_for_wifi_get.ssid()),
            "rssi": nic_for_wifi_get.rssi(),
            "driver": zenclaw_agent::net::wifi_ui::driver_label(),
        }).to_string();
        let mut resp = req.into_response(200, None, CORS_HEADERS)?;
        resp.write_all(body.as_bytes())?;
        Ok(())
    }).unwrap();

    // --- PUT /api/wifi (save credentials + restart) ---
    let nvs_for_wifi_put = nvs.clone();
    server.fn_handler::<anyhow::Error, _>("/api/wifi", Method::Put, move |mut req| {
        let mut buf = [0u8; 1024];
        let mut body = Vec::new();
        loop {
            let n = req.read(&mut buf)?;
            if n == 0 { break; }
            body.extend_from_slice(&buf[..n]);
        }
        let parsed: serde_json::Value = serde_json::from_slice(&body).unwrap_or_default();
        let new_ssid = parsed.get("ssid").and_then(|s| s.as_str()).unwrap_or("");
        let new_pass = parsed.get("password").and_then(|s| s.as_str());
        if new_ssid.is_empty() {
            let err = serde_json::json!({"error": "ssid required"}).to_string();
            let mut resp = req.into_response(400, None, CORS_HEADERS)?;
            resp.write_all(err.as_bytes())?;
            return Ok(());
        }
        zenclaw_agent::net::wifi_ui::write_credentials(&nvs_for_wifi_put, new_ssid, new_pass)
            .map_err(|e| anyhow::anyhow!("write_credentials: {}", e))?;
        log::info!("WiFi credentials saved, restarting...");
        let resp_body = serde_json::json!({"ok": true, "restart": true}).to_string();
        let mut resp = req.into_response(200, None, CORS_HEADERS)?;
        resp.write_all(resp_body.as_bytes())?;
        std::thread::spawn(|| {
            std::thread::sleep(std::time::Duration::from_secs(1));
            unsafe { esp_idf_svc::sys::esp_restart(); }
        });
        Ok(())
    }).unwrap();

    // --- GET /api/files (list directory) ---
    server.fn_handler::<anyhow::Error, _>("/api/files", Method::Get, |req| {
        let uri = req.uri().to_string();
        let raw = get_query_param(&uri, "path").unwrap_or_default();
        let path = match jail_filesystem_path(&raw) {
            Ok(p) => p,
            Err(msg) => {
                let err = serde_json::json!({"error": msg}).to_string();
                let mut resp = req.into_response(400, None, CORS_HEADERS)?;
                resp.write_all(err.as_bytes())?;
                return Ok(());
            }
        };
        let mut entries = Vec::new();
        if let Ok(dir) = std::fs::read_dir(&path) {
            for entry in dir.flatten() {
                let name = entry.file_name().to_string_lossy().to_string();
                let full = format!("{}/{}", path.trim_end_matches('/'), name);
                let meta = entry.metadata().ok();
                let is_dir = meta.as_ref().map(|m| m.is_dir()).unwrap_or(false);
                let size = if is_dir { None } else { meta.map(|m| m.len()) };
                entries.push(serde_json::json!({"name": name, "path": full, "is_dir": is_dir, "size": size}));
            }
        }
        let body = serde_json::json!({"path": path, "entries": entries}).to_string();
        let mut resp = req.into_response(200, None, CORS_HEADERS)?;
        resp.write_all(body.as_bytes())?;
        Ok(())
    }).unwrap();

    // --- DELETE /api/files ---
    server.fn_handler::<anyhow::Error, _>("/api/files", Method::Delete, |req| {
        let uri = req.uri().to_string();
        let raw = get_query_param(&uri, "path").unwrap_or_default();
        let path = match jail_filesystem_path(&raw) {
            Ok(p) if is_mount_root(&p) => {
                let err = serde_json::json!({"error": format!("refusing to delete mount root {}", p)}).to_string();
                let mut resp = req.into_response(400, None, CORS_HEADERS)?;
                resp.write_all(err.as_bytes())?;
                return Ok(());
            }
            Ok(p) => p,
            Err(msg) => {
                let err = serde_json::json!({"error": msg}).to_string();
                let mut resp = req.into_response(400, None, CORS_HEADERS)?;
                resp.write_all(err.as_bytes())?;
                return Ok(());
            }
        };
        let result = if std::fs::metadata(&path).map(|m| m.is_dir()).unwrap_or(false) {
            std::fs::remove_dir_all(&path)
        } else {
            std::fs::remove_file(&path)
        };
        match result {
            Ok(()) => {
                let body = serde_json::json!({"deleted": path}).to_string();
                let mut resp = req.into_response(200, None, CORS_HEADERS)?;
                resp.write_all(body.as_bytes())?;
            }
            Err(e) => {
                let err = serde_json::json!({"error": e.to_string()}).to_string();
                let mut resp = req.into_response(500, None, CORS_HEADERS)?;
                resp.write_all(err.as_bytes())?;
            }
        }
        Ok(())
    }).unwrap();

    // --- GET /api/files/read ---
    server.fn_handler::<anyhow::Error, _>("/api/files/read", Method::Get, |req| {
        let uri = req.uri().to_string();
        let raw = get_query_param(&uri, "path").unwrap_or_default();
        let path = match jail_filesystem_path(&raw) {
            Ok(p) => p,
            Err(msg) => {
                let err = serde_json::json!({"error": msg}).to_string();
                let mut resp = req.into_response(400, None, CORS_HEADERS)?;
                resp.write_all(err.as_bytes())?;
                return Ok(());
            }
        };
        match std::fs::read_to_string(&path) {
            Ok(content) => {
                let body = serde_json::json!({"path": path, "content": content}).to_string();
                let mut resp = req.into_response(200, None, CORS_HEADERS)?;
                resp.write_all(body.as_bytes())?;
            }
            Err(e) => {
                let err = serde_json::json!({"error": e.to_string()}).to_string();
                let mut resp = req.into_response(404, None, CORS_HEADERS)?;
                resp.write_all(err.as_bytes())?;
            }
        }
        Ok(())
    }).unwrap();

    // --- PUT /api/files/write ---
    server.fn_handler::<anyhow::Error, _>("/api/files/write", Method::Put, |mut req| {
        let mut buf = [0u8; 4096];
        let mut body = Vec::new();
        loop {
            let n = req.read(&mut buf)?;
            if n == 0 { break; }
            body.extend_from_slice(&buf[..n]);
        }
        let parsed: serde_json::Value = serde_json::from_slice(&body).unwrap_or_default();
        let raw = parsed.get("path").and_then(|p| p.as_str()).unwrap_or("");
        let content = parsed.get("content").and_then(|c| c.as_str()).unwrap_or("");
        let path = match jail_filesystem_path(raw) {
            Ok(p) if is_mount_root(&p) => {
                let err = serde_json::json!({"error": "path required"}).to_string();
                let mut resp = req.into_response(400, None, CORS_HEADERS)?;
                resp.write_all(err.as_bytes())?;
                return Ok(());
            }
            Ok(p) => p,
            Err(msg) => {
                let err = serde_json::json!({"error": msg}).to_string();
                let mut resp = req.into_response(400, None, CORS_HEADERS)?;
                resp.write_all(err.as_bytes())?;
                return Ok(());
            }
        };
        if let Some(parent) = std::path::Path::new(&path).parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        match std::fs::write(&path, content.as_bytes()) {
            Ok(()) => {
                let body = serde_json::json!({"path": path, "size": content.len()}).to_string();
                let mut resp = req.into_response(200, None, CORS_HEADERS)?;
                resp.write_all(body.as_bytes())?;
            }
            Err(e) => {
                let err = serde_json::json!({"error": e.to_string()}).to_string();
                let mut resp = req.into_response(500, None, CORS_HEADERS)?;
                resp.write_all(err.as_bytes())?;
            }
        }
        Ok(())
    }).unwrap();

    // --- POST /api/files/mkdir ---
    server.fn_handler::<anyhow::Error, _>("/api/files/mkdir", Method::Post, |mut req| {
        let mut buf = [0u8; 1024];
        let mut body = Vec::new();
        loop {
            let n = req.read(&mut buf)?;
            if n == 0 { break; }
            body.extend_from_slice(&buf[..n]);
        }
        let parsed: serde_json::Value = serde_json::from_slice(&body).unwrap_or_default();
        let raw = parsed.get("path").and_then(|p| p.as_str()).unwrap_or("");
        let path = match jail_filesystem_path(raw) {
            Ok(p) if is_mount_root(&p) => {
                let err = serde_json::json!({"error": "path required"}).to_string();
                let mut resp = req.into_response(400, None, CORS_HEADERS)?;
                resp.write_all(err.as_bytes())?;
                return Ok(());
            }
            Ok(p) => p,
            Err(msg) => {
                let err = serde_json::json!({"error": msg}).to_string();
                let mut resp = req.into_response(400, None, CORS_HEADERS)?;
                resp.write_all(err.as_bytes())?;
                return Ok(());
            }
        };
        match std::fs::create_dir_all(&path) {
            Ok(()) => {
                let body = serde_json::json!({"path": path}).to_string();
                let mut resp = req.into_response(200, None, CORS_HEADERS)?;
                resp.write_all(body.as_bytes())?;
            }
            Err(e) => {
                let err = serde_json::json!({"error": e.to_string()}).to_string();
                let mut resp = req.into_response(500, None, CORS_HEADERS)?;
                resp.write_all(err.as_bytes())?;
            }
        }
        Ok(())
    }).unwrap();

    // --- POST /api/files/upload (binary stream to file) ---
    server.fn_handler::<anyhow::Error, _>("/api/files/upload", Method::Post, |mut req| {
        let uri = req.uri().to_string();
        let raw = get_query_param(&uri, "path").unwrap_or_default();
        let path = match jail_filesystem_path(&raw) {
            Ok(p) if is_mount_root(&p) => {
                let err = serde_json::json!({"error": "path required"}).to_string();
                let mut resp = req.into_response(400, None, CORS_HEADERS)?;
                resp.write_all(err.as_bytes())?;
                return Ok(());
            }
            Ok(p) => p,
            Err(msg) => {
                let err = serde_json::json!({"error": msg}).to_string();
                let mut resp = req.into_response(400, None, CORS_HEADERS)?;
                resp.write_all(err.as_bytes())?;
                return Ok(());
            }
        };
        if let Some(parent) = std::path::Path::new(&path).parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let mut file = std::fs::File::create(&path)?;
        let mut buf = [0u8; 4096];
        let mut total = 0usize;
        loop {
            let n = req.read(&mut buf)?;
            if n == 0 { break; }
            std::io::Write::write_all(&mut file, &buf[..n])?;
            total += n;
        }
        let resp_body = serde_json::json!({"path": path, "size": total}).to_string();
        let mut resp = req.into_response(200, None, CORS_HEADERS)?;
        resp.write_all(resp_body.as_bytes())?;
        Ok(())
    }).unwrap();

    // --- POST /api/restart ---
    server.fn_handler::<anyhow::Error, _>("/api/restart", Method::Post, |req| {
        let body = r#"{"ok":true}"#;
        let mut resp = req.into_response(200, None, CORS_HEADERS)?;
        resp.write_all(body.as_bytes())?;
        std::thread::spawn(|| {
            std::thread::sleep(std::time::Duration::from_millis(500));
            unsafe { esp_idf_svc::sys::esp_restart(); }
        });
        Ok(())
    }).unwrap();

    // --- /api/sessions (GET) — list all sessions sorted by last_activity_ms desc ---
    let gw_sessions_list = gateway.clone();
    server.fn_handler::<anyhow::Error, _>("/api/sessions", Method::Get, move |req| {
        let mut sessions = gw_sessions_list.sessions.list_with_meta();
        sessions.sort_by(|a, b| b.last_activity_ms.cmp(&a.last_activity_ms));
        let body = serde_json::to_vec(&sessions)?;
        let mut resp = req.into_response(200, Some("OK"), CORS_HEADERS)?;
        resp.write_all(&body)?;
        Ok(())
    }).unwrap();

    // --- /api/sessions (POST) — create a new session ---
    let gw_sessions_create = gateway.clone();
    server.fn_handler::<anyhow::Error, _>("/api/sessions", Method::Post, move |req| {
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        let chat_id = format!("chat-{}", now_ms);
        let meta = zenclaw_agent::core::sessions::meta::SessionMeta::synthesize_default(
            &chat_id, now_ms, None,
        );
        if let Err(e) = gw_sessions_create.sessions.set_meta(&chat_id, &meta) {
            log::warn!("api_sessions_create: set_meta failed for {}: {}", chat_id, e);
            let mut resp = req.into_response(500, Some("Internal Server Error"), CORS_HEADERS)?;
            resp.write_all(serde_json::json!({"error": format!("set_meta: {}", e)}).to_string().as_bytes())?;
            return Ok(());
        }
        let body = serde_json::to_vec(&serde_json::json!({"chatId": chat_id, "meta": meta}))?;
        log::info!("Session created: {}", chat_id);
        let mut resp = req.into_response(201, Some("Created"), CORS_HEADERS)?;
        resp.write_all(&body)?;
        Ok(())
    }).unwrap();

    // --- /api/sessions/* (PATCH) — rename a session ---
    // Wildcard URI matching is enabled via Configuration::uri_match_wildcard = true above.
    // ESP-IDF httpd binds httpd_uri_match_wildcard, which matches /api/sessions/*
    // against any suffix; we strip the prefix and split on '?' to extract the chat_id.
    let gw_sessions_patch = gateway.clone();
    server.fn_handler::<anyhow::Error, _>("/api/sessions/*", Method::Patch, move |mut req| {
        let uri = req.uri().to_string();
        let chat_id = uri
            .strip_prefix("/api/sessions/")
            .and_then(|s| s.split('?').next())
            .map(|s| s.to_string())
            .unwrap_or_default();
        if chat_id.is_empty() {
            let mut resp = req.into_response(400, Some("Bad Request"), CORS_HEADERS)?;
            resp.write_all(serde_json::json!({"error": "missing chat id"}).to_string().as_bytes())?;
            return Ok(());
        }

        let mut body = Vec::new();
        let mut buf = [0u8; 1024];
        loop {
            let n = req.read(&mut buf)?;
            if n == 0 { break; }
            body.extend_from_slice(&buf[..n]);
        }

        #[derive(serde::Deserialize)]
        struct PatchBody { title: String }

        let parsed: PatchBody = match serde_json::from_slice(&body) {
            Ok(p) => p,
            Err(e) => {
                let mut resp = req.into_response(400, Some("Bad Request"), CORS_HEADERS)?;
                resp.write_all(serde_json::json!({"error": format!("invalid body: {}", e)}).to_string().as_bytes())?;
                return Ok(());
            }
        };

        match gw_sessions_patch.sessions.rename(&chat_id, &parsed.title) {
            Ok(meta) => {
                log::info!("Session renamed: {} -> {}", chat_id, parsed.title);
                let body = serde_json::to_vec(&meta)?;
                let mut resp = req.into_response(200, Some("OK"), CORS_HEADERS)?;
                resp.write_all(&body)?;
                Ok(())
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                let mut resp = req.into_response(404, Some("Not Found"), CORS_HEADERS)?;
                resp.write_all(serde_json::json!({"error": format!("not found: {}", chat_id)}).to_string().as_bytes())?;
                Ok(())
            }
            Err(e) if e.kind() == std::io::ErrorKind::InvalidInput => {
                let mut resp = req.into_response(400, Some("Bad Request"), CORS_HEADERS)?;
                resp.write_all(serde_json::json!({"error": e.to_string()}).to_string().as_bytes())?;
                Ok(())
            }
            Err(e) => {
                let mut resp = req.into_response(500, Some("Internal Server Error"), CORS_HEADERS)?;
                resp.write_all(serde_json::json!({"error": e.to_string()}).to_string().as_bytes())?;
                Ok(())
            }
        }
    }).unwrap();

    // --- /api/sessions/* (DELETE) — delete a session ---
    let gw_sessions_delete = gateway.clone();
    server.fn_handler::<anyhow::Error, _>("/api/sessions/*", Method::Delete, move |req| {
        let uri = req.uri().to_string();
        let chat_id = uri
            .strip_prefix("/api/sessions/")
            .and_then(|s| s.split('?').next())
            .map(|s| s.to_string())
            .unwrap_or_default();
        if chat_id.is_empty() {
            let mut resp = req.into_response(400, Some("Bad Request"), CORS_HEADERS)?;
            resp.write_all(serde_json::json!({"error": "missing chat id"}).to_string().as_bytes())?;
            return Ok(());
        }

        let result = match gw_sessions_delete.cloud_store.as_ref() {
            Some(s) => gw_sessions_delete.sessions.delete_with_store(
                &chat_id,
                s.as_ref() as &dyn zenclaw_agent::core::cloud::client::ObjectStore,
            ),
            None => gw_sessions_delete.sessions.delete(&chat_id),
        };
        match result {
            Ok(()) => {
                log::info!("Session deleted: {}", chat_id);
                let _resp = req.into_response(204, Some("No Content"), CORS_HEADERS)?;
                Ok(())
            }
            Err(e) => {
                let mut resp = req.into_response(500, Some("Internal Server Error"), CORS_HEADERS)?;
                resp.write_all(serde_json::json!({"error": e.to_string()}).to_string().as_bytes())?;
                Ok(())
            }
        }
    }).unwrap();

    // --- WS /ws/stats (live stats stream) ---
    //
    // Pushes the same payload as `/api/status` every 10 seconds. The
    // shared `build_status_payload` ensures GET and WS never diverge —
    // the web client uses a single `setStatus` (full replace) handler
    // regardless of transport. Desktop builds use the same cadence.
    // See docs/superpowers/specs/2026-05-03-stats-transport-model.md.
    {
        use embedded_svc::ws::FrameType;
        let gw_for_ws = gateway.clone();
        let nic_for_ws = nic.clone();
        let th = temp_handle;
        server.ws_handler::<_, anyhow::Error>("/ws/stats", None, move |ws: &mut esp_idf_svc::http::server::ws::EspHttpWsConnection| {
            if ws.is_new() {
                let sender = ws.create_detached_sender()?;
                let gw_clone = gw_for_ws.clone();
                let nic_clone = nic_for_ws.clone();
                std::thread::Builder::new()
                    .name("ws-stats".into())
                    .stack_size(8192)
                    .spawn(move || {
                        let mut sender = sender;
                        loop {
                            if sender.is_closed() { break; }
                            let payload = build_status_payload(&gw_clone, &nic_clone, th);
                            if sender.send(FrameType::Text(false), payload.to_string().as_bytes()).is_err() {
                                break;
                            }
                            std::thread::sleep(std::time::Duration::from_secs(10));
                        }
                    }).ok();
                return Ok(());
            }
            if ws.is_closed() { return Ok(()); }
            let _ = ws.recv(&mut [0u8; 64]);
            Ok(())
        }).unwrap();
    }

    // --- WS /ws/chat (streaming chat) ---
    //
    // Inbound frames are typed `ChatEvent`s from the browser:
    //   - `user_message` — start a turn, stream typed events back.
    //   - `cancel`       — abort the active turn for this chat_id.
    //
    // The chat itself runs on the long-lived `agent_thread` (already 32KB
    // stack). The WS handler only spawns a small forwarder that owns the
    // detached WS sender and drains a `mpsc::Receiver<ChatEvent>`, writing
    // each event as a JSON text frame. This avoids spawning a fresh 32KB
    // worker stack per turn — internal SRAM is tight on ESP32-S3.
    {
        use embedded_svc::ws::FrameType;
        use zenclaw_agent::core::chat_events::ChatEvent;
        let gw_ws = gateway.clone();
        let chat_tx_ws = chat_tx.clone();
        server.ws_handler::<_, anyhow::Error>("/ws/chat", None, move |ws: &mut esp_idf_svc::http::server::ws::EspHttpWsConnection| {
            if ws.is_new() { return Ok(()); }
            if ws.is_closed() { return Ok(()); }
            // Single-call recv with a sized buffer. The two-call peek-then-fill
            // pattern does not behave reliably under esp-idf v5.4 on ESP32-P4 —
            // the second httpd_ws_recv_frame returns success but leaves the
            // buffer zeroed, causing the JSON parse to silently fail.
            let mut buf = vec![0u8; 4096];
            let (_ft, len) = ws.recv(&mut buf)?;
            if len == 0 { return Ok(()); }
            buf.truncate(len);
            // esp-idf v5.4's httpd_ws_recv_frame can report a length that
            // overshoots the actual payload by a handful of bytes; the tail
            // of `buf` is zero-padding from the initial allocation. A strict
            // typed deserialize rejects those trailing nulls ("trailing
            // characters at line 1 column N"). Trim them before parsing.
            while buf.last() == Some(&0) {
                buf.pop();
            }

            let evt: ChatEvent = match serde_json::from_slice(&buf) {
                Ok(v) => v,
                Err(e) => {
                    log::warn!("WS chat: invalid frame ({}B): {}", buf.len(), e);
                    return Ok(());
                }
            };

            match evt {
                ChatEvent::UserMessage { chat_id, text } => {
                    if text.is_empty() {
                        let mut sender = ws.create_detached_sender()?;
                        let err = ChatEvent::Error { error: "Empty message".to_string() };
                        if let Ok(json) = serde_json::to_string(&err) {
                            let _ = sender.send(FrameType::Text(false), json.as_bytes());
                        }
                        return Ok(());
                    }
                    log::info!("WS chat: chat_id={} msg_len={}", chat_id, text.len());

                    // Internal SRAM is too tight to spawn a 32KB worker on
                    // top of a forwarder thread (ENOMEM). Route the chat
                    // through the long-lived agent_thread (already 32KB) and
                    // only spawn a small forwarder that owns the WS sender
                    // and drains the event channel until the chat completes.
                    let sender = ws.create_detached_sender()?;
                    let (events_tx, events_rx) =
                        std::sync::mpsc::channel::<ChatEvent>();
                    {
                        let tx = chat_tx_ws.lock().unwrap();
                        if let Err(e) = tx.send(ChatRequest {
                            chat_id,
                            message: text,
                            reply_tx: None,
                            events_tx: Some(events_tx),
                        }) {
                            log::error!("WS chat: enqueue: {}", e);
                            return Ok(());
                        }
                    }

                    std::thread::Builder::new()
                        .name("ws-chat-fwd".into())
                        .stack_size(8192)
                        .spawn(move || {
                            let mut sender = sender;
                            while let Ok(evt) = events_rx.recv() {
                                let json = match serde_json::to_string(&evt) {
                                    Ok(s) => s,
                                    Err(e) => {
                                        log::warn!("WS chat: serialize event: {}", e);
                                        continue;
                                    }
                                };
                                if sender
                                    .send(FrameType::Text(false), json.as_bytes())
                                    .is_err()
                                {
                                    // Browser disconnected — agent thread keeps
                                    // running so the turn still lands in session
                                    // JSONL; just stop forwarding.
                                    break;
                                }
                            }
                        })
                        .ok();
                }
                ChatEvent::Cancel { chat_id } => {
                    log::info!("WS chat: cancel chat_id={}", chat_id);
                    let gw = gw_ws.clone();
                    std::thread::Builder::new()
                        .name("ws-chat-cancel".into())
                        .stack_size(8192)
                        .spawn(move || {
                            esp_idf_svc::hal::task::block_on(gw.cancel_chat(&chat_id));
                        })
                        .ok();
                }
                _ => {
                    log::warn!("WS chat: unexpected inbound event type");
                }
            }
            Ok(())
        }).unwrap();
    }

    // --- WS /ws/logs (stream log entries as JSON) ---
    {
        use embedded_svc::ws::FrameType;
        server.ws_handler::<_, anyhow::Error>("/ws/logs", None, |ws: &mut esp_idf_svc::http::server::ws::EspHttpWsConnection| {
            if ws.is_new() {
                let sender = ws.create_detached_sender()?;
                std::thread::Builder::new()
                    .name("ws-logs".into())
                    .stack_size(4096)
                    .spawn(move || {
                        let mut sender = sender;
                        let mut epoch = log_ring::epoch();
                        loop {
                            if sender.is_closed() { break; }
                            let (entries, new_epoch) = log_ring::drain_since(epoch);
                            epoch = new_epoch;
                            for json in entries {
                                if sender.send(FrameType::Text(false), json.as_bytes()).is_err() {
                                    return;
                                }
                            }
                            std::thread::sleep(std::time::Duration::from_millis(500));
                        }
                    }).ok();
                return Ok(());
            }
            if ws.is_closed() { return Ok(()); }
            let _ = ws.recv(&mut [0u8; 64]);
            Ok(())
        }).unwrap();
    }

    log::info!("HTTP server on :80 — http://{}/ or http://{}.local/", ip_str, hostname);

    // Leak the server so it stays alive
    std::mem::forget(server);
}

/// A chat request sent from the httpd handler to the agent thread.
#[cfg(feature = "esp32")]
struct ChatRequest {
    chat_id: String,
    message: String,
    /// REST callers set this to receive the final reply string. WS callers
    /// pass `None` and rely on `events_tx` for the full event stream.
    reply_tx: Option<std::sync::mpsc::Sender<Result<String, String>>>,
    /// WS callers attach a sender to receive each typed event (thinking,
    /// tool_call_*, assistant_text, done, error). REST callers pass `None`.
    events_tx: Option<std::sync::mpsc::Sender<zenclaw_agent::core::chat_events::ChatEvent>>,
}

/// Unified agent thread — handles both Telegram polling and HTTP chat requests.
/// Single 32KB stack thread avoids the OOM from spawning a third thread.
#[cfg(feature = "esp32")]
fn agent_thread(
    chat_rx: std::sync::mpsc::Receiver<ChatRequest>,
    gateway: std::sync::Arc<zenclaw_agent::core::gateway::Gateway>,
    http: std::sync::Arc<dyn zenclaw_agent::platform::http_client::HttpClient>,
    mut tg: Option<(
        zenclaw_agent::core::channels::telegram::Poller,
        zenclaw_agent::core::channels::telegram::TelegramChannel,
        Option<Vec<String>>,
    )>,
) {
    use zenclaw_agent::core::channels::Channel;

    if tg.is_some() {
        log::info!("Agent thread: Telegram + HTTP chat");
    } else {
        log::info!("Agent thread: HTTP chat only");
    }

    // Register the BotFather menu (single source of truth in commands::menu()).
    // Non-fatal: rate limit / no network — log and continue.
    if let Some((poller, _channel, _allowed)) = tg.as_ref() {
        if let Err(e) = esp_idf_svc::hal::task::block_on(
            poller.set_my_commands(&*http, zenclaw_agent::core::commands::menu()),
        ) {
            log::warn!("setMyCommands failed (non-fatal): {}", e);
        }
    }

    loop {
        // --- Process any pending HTTP chat requests (non-blocking) ---
        while let Ok(req) = chat_rx.try_recv() {
            run_chat_request(gateway.as_ref(), req);
        }

        // --- Telegram poll (if enabled) ---
        if let Some((poller, channel, allowed)) = tg.as_mut() {
            let messages = match esp_idf_svc::hal::task::block_on(poller.poll_once(&*http, 10)) {
                Ok(m) => m,
                Err(e) => {
                    log::error!("Telegram poll: {}", e);
                    std::thread::sleep(std::time::Duration::from_secs(5));
                    continue;
                }
            };

            for msg in messages {
                if let Some(ids) = allowed.as_ref() {
                    if !ids.contains(&msg.chat_id) {
                        log::warn!(
                            "Telegram message from disallowed chat: {}",
                            msg.chat_id
                        );
                        continue;
                    }
                }

                log::info!(
                    "Telegram msg from {}: {}B",
                    msg.chat_id,
                    msg.text.len()
                );

                if let Err(e) =
                    esp_idf_svc::hal::task::block_on(channel.send_typing(&msg.chat_id))
                {
                    log::warn!("Telegram send_typing: {}", e);
                }

                zenclaw_agent::led_status::set(zenclaw_agent::led_status::State::Thinking);
                let reply = match esp_idf_svc::hal::task::block_on(
                    gateway.chat(&msg.chat_id, &msg.text, "telegram"),
                ) {
                    Ok(r) => r,
                    Err(e) => format!("Error: {}", e),
                };
                zenclaw_agent::led_status::set(zenclaw_agent::led_status::State::Idle);

                if let Err(e) =
                    esp_idf_svc::hal::task::block_on(channel.deliver(&msg.chat_id, &reply))
                {
                    log::error!("Telegram deliver: {}", e);
                } else {
                    log::info!("Telegram reply sent to {}", msg.chat_id);
                }
            }
        } else {
            // No Telegram — block-wait for HTTP chat requests so we don't busy-loop.
            if let Ok(req) = chat_rx.recv_timeout(std::time::Duration::from_secs(1)) {
                run_chat_request(gateway.as_ref(), req);
            }
        }
    }
}

/// Runs one chat request on the agent thread, dispatching whichever signals
/// the caller asked for. REST callers populate `reply_tx` and read the
/// final string back; WS callers populate `events_tx` and the event stream
/// arrives in real time as the agent loop runs.
#[cfg(feature = "esp32")]
fn run_chat_request(
    gateway: &zenclaw_agent::core::gateway::Gateway,
    req: ChatRequest,
) {
    log::info!(
        "HTTP chat: chat_id={} msg_len={} streaming={}",
        req.chat_id,
        req.message.len(),
        req.events_tx.is_some(),
    );
    zenclaw_agent::led_status::set(zenclaw_agent::led_status::State::Thinking);
    let result = esp_idf_svc::hal::task::block_on(gateway.chat_with_events(
        &req.chat_id,
        &req.message,
        "api",
        req.events_tx.as_ref(),
    ));
    zenclaw_agent::led_status::set(zenclaw_agent::led_status::State::Idle);
    if let Some(reply_tx) = req.reply_tx {
        let _ = reply_tx.send(result.map_err(|e| e.to_string()));
    }
    // events_tx drops here; the WS forwarder's recv() will return Err next.
}

#[cfg(feature = "desktop")]
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    zenclaw_agent::desktop::run().await
}

#[cfg(test)]
mod hostname_tests {
    use super::format_mac_suffix;

    #[test]
    fn format_mac_suffix_uses_lower_three_bytes_lowercase_hex() {
        let mac = [0xAA, 0xBB, 0xCC, 0xDD, 0xEE, 0xFF];
        assert_eq!(format_mac_suffix(&mac), "zenclaw-ddeeff");
    }

    #[test]
    fn format_mac_suffix_zero_pads_each_byte() {
        let mac = [0x00, 0x00, 0x00, 0x01, 0x02, 0x03];
        assert_eq!(format_mac_suffix(&mac), "zenclaw-010203");
    }
}
