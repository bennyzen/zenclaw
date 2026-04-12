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

#[cfg(feature = "esp32")]
fn main() {
    esp_idf_svc::sys::link_patches();
    log_ring::init();
    log::info!("=== ZenClaw ESP32 boot ===");

    // --- WiFi ---
    use esp_idf_svc::hal::prelude::Peripherals;
    use esp_idf_svc::eventloop::EspSystemEventLoop;
    use esp_idf_svc::nvs::EspDefaultNvsPartition;
    use esp_idf_svc::wifi::{AuthMethod, ClientConfiguration, Configuration, EspWifi};

    let peripherals = Peripherals::take().unwrap();
    let sysloop = EspSystemEventLoop::take().unwrap();
    let nvs = EspDefaultNvsPartition::take().unwrap();

    let nvs_handle = esp_idf_svc::nvs::EspNvs::new(nvs.clone(), "wifi", true).unwrap();
    let ssid = nvs_get_string(&nvs_handle, "ssid").unwrap_or_default();
    let password = nvs_get_string(&nvs_handle, "password").unwrap_or_default();
    log::info!("NVS: ssid='{}' (len={})", ssid, ssid.len());
    drop(nvs_handle);

    if ssid.is_empty() {
        log::error!("No WiFi SSID in NVS — halting");
        loop { std::thread::sleep(std::time::Duration::from_secs(60)); }
    }

    let mut wifi = EspWifi::new(peripherals.modem, sysloop.clone(), Some(nvs.clone())).unwrap();
    let mut ssid_h: heapless::String<32> = heapless::String::new();
    ssid_h.push_str(&ssid).unwrap();
    let mut pass_h: heapless::String<64> = heapless::String::new();
    pass_h.push_str(&password).unwrap();

    wifi.set_configuration(&Configuration::Client(ClientConfiguration {
        ssid: ssid_h,
        password: pass_h,
        auth_method: AuthMethod::WPA2Personal,
        ..Default::default()
    })).unwrap();
    wifi.start().unwrap();
    wifi.connect().unwrap();
    log::info!("WiFi connecting...");

    let mut ip_str = String::new();
    for i in 0..30 {
        std::thread::sleep(std::time::Duration::from_millis(500));
        if wifi.is_connected().unwrap_or(false) {
            let netif = wifi.sta_netif();
            if let Ok(info) = netif.get_ip_info() {
                if !info.ip.is_unspecified() {
                    ip_str = format!("{}", info.ip);
                    log::info!("Got IP: {} (after {}ms)", ip_str, i * 500);
                    break;
                }
            }
        }
    }
    if ip_str.is_empty() {
        log::error!("WiFi: no IP after 15s — halting");
        loop { std::thread::sleep(std::time::Duration::from_secs(60)); }
    }

    // --- mDNS ---
    #[cfg(any(esp_idf_comp_mdns_enabled, esp_idf_comp_espressif__mdns_enabled))]
    {
        let mut mdns = esp_idf_svc::mdns::EspMdns::take().unwrap();
        mdns.set_hostname("zenclaw").unwrap();
        mdns.set_instance_name("ZenClaw Agent").unwrap();
        mdns.add_service(None, "_http", "_tcp", 80, &[]).unwrap();
        log::info!("mDNS: zenclaw.local");
        std::mem::forget(mdns);
    }
    #[cfg(not(any(esp_idf_comp_mdns_enabled, esp_idf_comp_espressif__mdns_enabled)))]
    log::warn!("mDNS: not available (needs cargo clean && cargo build)");

    // --- Load config ---
    let config = load_config(&nvs);
    log::info!("Config: agent={}, provider={}", config.agent_name, config.providers.default);

    // --- Mount SPIFFS ---
    let spiffs_conf = esp_idf_svc::sys::esp_vfs_spiffs_conf_t {
        base_path: b"/data\0".as_ptr() as *const core::ffi::c_char,
        partition_label: b"storage\0".as_ptr() as *const core::ffi::c_char,
        max_files: 8,
        format_if_mount_failed: true,
    };
    let ret = unsafe { esp_idf_svc::sys::esp_vfs_spiffs_register(&spiffs_conf) };
    if ret != 0 {
        log::error!("SPIFFS mount failed: err={}", ret);
    } else {
        let mut total: usize = 0;
        let mut used: usize = 0;
        unsafe {
            esp_idf_svc::sys::esp_spiffs_info(
                b"storage\0".as_ptr() as *const core::ffi::c_char,
                &mut total,
                &mut used,
            );
        }
        log::info!("SPIFFS mounted at /data ({}KB total, {}KB used)", total / 1024, used / 1024);
    }

    // --- USB mass storage (optional) ---
    #[cfg(feature = "usb_storage")]
    zenclaw_agent::usb_storage::init();

    // --- Create gateway ---
    let data_dir = "/data";
    let _ = std::fs::create_dir_all(format!("{}/sessions", data_dir));
    let _ = std::fs::create_dir_all(format!("{}/memory", data_dir));

    let config_for_tg = config.clone();
    let config_arc = std::sync::Arc::new(config.clone());
    let runner = Box::new(zenclaw_agent::esp32::runner::EspRunner::new(config_arc));
    let gateway = zenclaw_agent::core::gateway::Gateway::new(config, data_dir, runner);
    let gateway = std::sync::Arc::new(gateway);

    // --- Start HTTP server ---
    start_http_server(gateway.clone(), &ip_str, nvs);

    // --- Start Telegram poller if enabled ---
    if let Some(ref tg) = config_for_tg.channels.telegram {
        if tg.enabled && !tg.bot_token.is_empty() {
            let gw = gateway.clone();
            let bot_token = tg.bot_token.clone();
            std::thread::Builder::new()
                .name("telegram".into())
                .stack_size(32768)
                .spawn(move || telegram_poll_loop(&bot_token, gw))
                .ok();
            log::info!("Telegram poller started");
        }
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
    let mut handle = esp_idf_svc::nvs::EspNvs::new(nvs.clone(), "config", true)
        .map_err(|e| format!("NVS open: {}", e))?;
    handle.set_blob("json", json.as_bytes())
        .map_err(|e| format!("NVS write: {}", e))?;
    Ok(())
}

#[cfg(feature = "esp32")]
fn get_wifi_info() -> (Option<i32>, Option<String>) {
    let mut ap_info: esp_idf_svc::sys::wifi_ap_record_t = unsafe { std::mem::zeroed() };
    let ret = unsafe { esp_idf_svc::sys::esp_wifi_sta_get_ap_info(&mut ap_info) };
    if ret == 0 {
        let rssi = Some(ap_info.rssi as i32);
        let ssid = ap_info.ssid.iter()
            .take_while(|&&b| b != 0)
            .map(|&b| b as char)
            .collect::<String>();
        (rssi, if ssid.is_empty() { None } else { Some(ssid) })
    } else {
        (None, None)
    }
}

#[cfg(feature = "esp32")]
fn read_temp(handle_val: usize) -> Option<f64> {
    let handle = handle_val as esp_idf_svc::sys::temperature_sensor_handle_t;
    let mut celsius: f32 = 0.0;
    let ret = unsafe { esp_idf_svc::sys::temperature_sensor_get_celsius(handle, &mut celsius) };
    if ret == 0 { Some(((celsius as f64) * 10.0).round() / 10.0) } else { None }
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
    ip_str: &str,
    nvs: esp_idf_svc::nvs::EspDefaultNvsPartition,
) {
    use esp_idf_svc::http::server::{Configuration as HttpConfig, EspHttpServer};
    use esp_idf_svc::http::Method;

    let mut server = EspHttpServer::new(&HttpConfig {
        http_port: 80,
        stack_size: 16384,
        max_uri_handlers: 32,
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
        "/api/files/upload", "/api/restart",
    ] {
        server.fn_handler::<anyhow::Error, _>(path, Method::Options, |req| {
            let mut resp = req.into_response(204, None, &[
                ("Access-Control-Allow-Origin", "*"),
                ("Access-Control-Allow-Methods", "GET, POST, PUT, DELETE, OPTIONS"),
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
<p class="sub">ESP32-S3 &middot; v{ver}</p>
<div class="stat"><span class="label">IP</span><span>{ip}</span></div>
<div class="stat"><span class="label">Heap free</span><span>{heap}KB</span></div>
<div class="stat"><span class="label">API</span><a href="/api/status">/api/status</a></div>
<div class="stat"><span class="label">Chat</span><span>POST /api/chat</span></div>
</div></body></html>"#,
            name = name,
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
    let gw = gateway.clone();
    let ip_for_status = ip_str.to_string();
    let th = temp_handle;
    server.fn_handler::<anyhow::Error, _>("/api/status", Method::Get, move |req| {
        let heap_free = unsafe { esp_idf_svc::sys::esp_get_free_heap_size() } as usize;
        let heap_total = unsafe { esp_idf_svc::sys::heap_caps_get_total_size(4096) }; // MALLOC_CAP_DEFAULT
        let uptime_us = unsafe { esp_idf_svc::sys::esp_timer_get_time() };
        let mut spiffs_total: usize = 0;
        let mut spiffs_used: usize = 0;
        unsafe {
            esp_idf_svc::sys::esp_spiffs_info(
                b"storage\0".as_ptr() as *const core::ffi::c_char,
                &mut spiffs_total,
                &mut spiffs_used,
            );
        }
        let body = serde_json::json!({
            "agent_name": gw.config.agent_name,
            "version": env!("CARGO_PKG_VERSION"),
            "platform": "esp32s3",
            "memory": {
                "free_kb": heap_free / 1024,
                "total_kb": heap_total / 1024,
                "used_kb": heap_total.saturating_sub(heap_free) / 1024,
            },
            "temperature_c": read_temp(th),
            "wifi": {
                "connected": true,
                "ip": ip_for_status,
                "rssi": get_wifi_info().0,
            },
            "storage": {
                "total_kb": spiffs_total / 1024,
                "free_kb": (spiffs_total - spiffs_used) / 1024,
            },
            "channels": {
                "telegram": {
                    "configured": gw.config.channels.telegram.is_some(),
                    "enabled": gw.config.channels.telegram.as_ref().map_or(false, |t| t.enabled),
                    "has_token": gw.config.channels.telegram.as_ref().map_or(false, |t| !t.bot_token.is_empty()),
                },
            },
            "provider": gw.config.providers.default,
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
        let body_str = body.to_string();
        let mut resp = req.into_response(200, None, CORS_HEADERS)?;
        resp.write_all(body_str.as_bytes())?;
        Ok(())
    }).unwrap();

    // --- /api/chat (POST) ---
    let gw = gateway.clone();
    server.fn_handler::<anyhow::Error, _>("/api/chat", Method::Post, move |mut req| {
        // Read request body
        let mut buf = [0u8; 4096];
        let mut body = Vec::new();
        loop {
            let n = req.read(&mut buf)?;
            if n == 0 { break; }
            body.extend_from_slice(&buf[..n]);
        }

        let parsed: serde_json::Value = serde_json::from_slice(&body)
            .unwrap_or_default();
        let message = parsed.get("message")
            .and_then(|m| m.as_str())
            .unwrap_or("");
        let chat_id = parsed.get("chat_id")
            .and_then(|c| c.as_str())
            .unwrap_or("web");

        if message.is_empty() {
            let err = serde_json::json!({"error": "JSON body with message required"}).to_string();
            let mut resp = req.into_response(400, None, &[
                ("Content-Type", "application/json"),
            ])?;
            resp.write_all(err.as_bytes())?;
            return Ok(());
        }

        log::info!("Chat: chat_id={} msg_len={}", chat_id, message.len());

        // Run gateway.chat() — blocking via ESP-IDF's FreeRTOS-backed executor
        let result = esp_idf_svc::hal::task::block_on(gw.chat(chat_id, message, "api"));

        let resp_body = match result {
            Ok(reply) => serde_json::json!({"reply": reply}),
            Err(e) => {
                log::error!("Chat error: {}", e);
                serde_json::json!({"error": e.to_string()})
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
        // Simple: return empty for now, sessions work via JSONL on flash
        let uri = req.uri();
        let chat_id = uri.split("chat_id=")
            .nth(1)
            .and_then(|s| s.split('&').next())
            .unwrap_or("web");

        let branch = gw.sessions.get_branch(chat_id).unwrap_or_default();
        let mut messages: Vec<serde_json::Value> = Vec::new();
        for entry in &branch {
            if let zenclaw_agent::core::sessions::SessionEntry::Message { role, content, .. } = entry {
                let role_str = match role {
                    zenclaw_agent::core::types::Role::User => "user",
                    zenclaw_agent::core::types::Role::Assistant => "assistant",
                    _ => continue,
                };
                if content.is_empty() { continue; }
                messages.push(serde_json::json!({"role": role_str, "content": content}));
            }
        }
        // Keep last 50
        if messages.len() > 50 {
            messages = messages.split_off(messages.len() - 50);
        }
        let body = serde_json::json!({"messages": messages}).to_string();
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
                save_config_nvs(&nvs_w, &json).map_err(|e| anyhow::anyhow!(e))?;
                log::info!("Config saved to NVS ({}B), restarting...", json.len());
                let resp_body = serde_json::json!({"ok": true}).to_string();
                let mut resp = req.into_response(200, None, CORS_HEADERS)?;
                resp.write_all(resp_body.as_bytes())?;
                // Restart so gateway picks up new config
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

    // --- /api/wifi (GET) ---
    let ip = ip_str.to_string();
    server.fn_handler::<anyhow::Error, _>("/api/wifi", Method::Get, move |req| {
        let (rssi, ssid) = get_wifi_info();
        let body = serde_json::json!({
            "ssid": ssid,
            "connected": true,
            "ip": ip,
            "rssi": rssi,
            "hostname": "zenclaw"
        }).to_string();
        let mut resp = req.into_response(200, None, CORS_HEADERS)?;
        resp.write_all(body.as_bytes())?;
        Ok(())
    }).unwrap();

    // --- PUT /api/wifi (save credentials + restart) ---
    let nvs_wifi = nvs.clone();
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
        let new_pass = parsed.get("password").and_then(|s| s.as_str()).unwrap_or("");
        if new_ssid.is_empty() {
            let err = serde_json::json!({"error": "ssid required"}).to_string();
            let mut resp = req.into_response(400, None, CORS_HEADERS)?;
            resp.write_all(err.as_bytes())?;
            return Ok(());
        }
        match esp_idf_svc::nvs::EspNvs::new(nvs_wifi.clone(), "wifi", true) {
            Ok(mut handle) => {
                let _ = handle.set_str("ssid", new_ssid);
                let _ = handle.set_str("password", new_pass);
                log::info!("WiFi credentials saved, restarting...");
            }
            Err(e) => {
                let err = serde_json::json!({"error": format!("NVS: {}", e)}).to_string();
                let mut resp = req.into_response(500, None, CORS_HEADERS)?;
                resp.write_all(err.as_bytes())?;
                return Ok(());
            }
        }
        let resp_body = serde_json::json!({"ok": true, "connected": false, "ip": null}).to_string();
        let mut resp = req.into_response(200, None, CORS_HEADERS)?;
        resp.write_all(resp_body.as_bytes())?;
        std::thread::spawn(|| {
            std::thread::sleep(std::time::Duration::from_millis(500));
            unsafe { esp_idf_svc::sys::esp_restart(); }
        });
        Ok(())
    }).unwrap();

    // --- GET /api/files (list directory) ---
    server.fn_handler::<anyhow::Error, _>("/api/files", Method::Get, |req| {
        let uri = req.uri().to_string();
        let path = get_query_param(&uri, "path").unwrap_or_else(|| "/".to_string());
        let mut entries = Vec::new();
        if path == "/" {
            entries.push(serde_json::json!({"name": "data", "path": "/data", "is_dir": true, "size": null}));
        } else if let Ok(dir) = std::fs::read_dir(&path) {
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
        let path = get_query_param(&uri, "path").unwrap_or_default();
        if path.is_empty() {
            let err = serde_json::json!({"error": "path required"}).to_string();
            let mut resp = req.into_response(400, None, CORS_HEADERS)?;
            resp.write_all(err.as_bytes())?;
            return Ok(());
        }
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
        let path = get_query_param(&uri, "path").unwrap_or_default();
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
        let path = parsed.get("path").and_then(|p| p.as_str()).unwrap_or("");
        let content = parsed.get("content").and_then(|c| c.as_str()).unwrap_or("");
        if path.is_empty() {
            let err = serde_json::json!({"error": "path required"}).to_string();
            let mut resp = req.into_response(400, None, CORS_HEADERS)?;
            resp.write_all(err.as_bytes())?;
            return Ok(());
        }
        if let Some(parent) = std::path::Path::new(path).parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        match std::fs::write(path, content.as_bytes()) {
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
        let path = parsed.get("path").and_then(|p| p.as_str()).unwrap_or("");
        if path.is_empty() {
            let err = serde_json::json!({"error": "path required"}).to_string();
            let mut resp = req.into_response(400, None, CORS_HEADERS)?;
            resp.write_all(err.as_bytes())?;
            return Ok(());
        }
        let _ = std::fs::create_dir_all(path);
        let resp_body = serde_json::json!({"path": path}).to_string();
        let mut resp = req.into_response(200, None, CORS_HEADERS)?;
        resp.write_all(resp_body.as_bytes())?;
        Ok(())
    }).unwrap();

    // --- POST /api/files/upload (binary stream to file) ---
    server.fn_handler::<anyhow::Error, _>("/api/files/upload", Method::Post, |mut req| {
        let uri = req.uri().to_string();
        let path = get_query_param(&uri, "path").unwrap_or_default();
        if path.is_empty() {
            let err = serde_json::json!({"error": "path required"}).to_string();
            let mut resp = req.into_response(400, None, CORS_HEADERS)?;
            resp.write_all(err.as_bytes())?;
            return Ok(());
        }
        if let Some(parent) = std::path::Path::new(&*path).parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let mut file = std::fs::File::create(&*path)?;
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

    // --- WS /ws/stats (live stats stream) ---
    {
        use embedded_svc::ws::FrameType;
        let ip_for_ws = ip_str.to_string();
        let th = temp_handle;
        server.ws_handler::<_, anyhow::Error>("/ws/stats", move |ws| {
            if ws.is_new() {
                let sender = ws.create_detached_sender()?;
                let ip = ip_for_ws.clone();
                std::thread::Builder::new()
                    .name("ws-stats".into())
                    .stack_size(8192)
                    .spawn(move || {
                        let mut sender = sender;
                        loop {
                            if sender.is_closed() { break; }
                            let heap_free = unsafe { esp_idf_svc::sys::esp_get_free_heap_size() } as usize;
                            let heap_total = unsafe { esp_idf_svc::sys::heap_caps_get_total_size(4096) }; // MALLOC_CAP_DEFAULT
                            let uptime_us = unsafe { esp_idf_svc::sys::esp_timer_get_time() };
                            let mut total: usize = 0;
                            let mut used: usize = 0;
                            unsafe {
                                esp_idf_svc::sys::esp_spiffs_info(
                                    b"storage\0".as_ptr() as *const core::ffi::c_char,
                                    &mut total,
                                    &mut used,
                                );
                            }
                            let (rssi, _) = get_wifi_info();
                            let stats = serde_json::json!({
                                "memory": {
                                    "free_kb": heap_free / 1024,
                                    "total_kb": heap_total / 1024,
                                    "used_kb": heap_total.saturating_sub(heap_free) / 1024,
                                },
                                "temperature_c": read_temp(th),
                                "wifi": {"connected": true, "ip": ip, "rssi": rssi},
                                "storage": {
                                    "total_kb": total / 1024,
                                    "free_kb": total.saturating_sub(used) / 1024,
                                },
                                "uptime_s": uptime_us / 1_000_000
                            });
                            if sender.send(FrameType::Text(false), stats.to_string().as_bytes()).is_err() {
                                break;
                            }
                            std::thread::sleep(std::time::Duration::from_secs(5));
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
    {
        use embedded_svc::ws::FrameType;
        let gw_ws = gateway.clone();
        server.ws_handler::<_, anyhow::Error>("/ws/chat", move |ws| {
            if ws.is_new() { return Ok(()); }
            if ws.is_closed() { return Ok(()); }
            let (_ft, len) = ws.recv(&mut [])?;
            if len == 0 { return Ok(()); }
            let mut buf = vec![0u8; len];
            ws.recv(&mut buf)?;
            let sender = ws.create_detached_sender()?;
            let gw = gw_ws.clone();
            std::thread::Builder::new()
                .name("ws-chat".into())
                .stack_size(32768)
                .spawn(move || {
                    let mut sender = sender;
                    let parsed: serde_json::Value = serde_json::from_slice(&buf).unwrap_or_default();
                    let message = parsed.get("message").and_then(|m| m.as_str()).unwrap_or("");
                    let chat_id = parsed.get("chat_id").and_then(|c| c.as_str()).unwrap_or("web");
                    if message.is_empty() {
                        let _ = sender.send(FrameType::Text(false), br#"{"type":"error","error":"no message"}"#);
                        return;
                    }
                    log::info!("WS chat: chat_id={} msg_len={}", chat_id, message.len());
                    match esp_idf_svc::hal::task::block_on(gw.chat(chat_id, message, "api")) {
                        Ok(reply) => {
                            let msg = serde_json::json!({"type": "done", "text": reply});
                            let _ = sender.send(FrameType::Text(false), msg.to_string().as_bytes());
                        }
                        Err(e) => {
                            log::error!("WS chat error: {}", e);
                            let msg = serde_json::json!({"type": "error", "error": e.to_string()});
                            let _ = sender.send(FrameType::Text(false), msg.to_string().as_bytes());
                        }
                    }
                }).ok();
            Ok(())
        }).unwrap();
    }

    // --- WS /ws/logs (stream log entries as JSON) ---
    {
        use embedded_svc::ws::FrameType;
        server.ws_handler::<_, anyhow::Error>("/ws/logs", |ws| {
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

    log::info!("HTTP server on :80 — http://{}/ or http://zenclaw.local/", ip_str);

    // Leak the server so it stays alive
    std::mem::forget(server);
}

// ---------------------------------------------------------------------------
// Telegram poller (ESP32 — blocking HTTP via esp-idf-svc)
// ---------------------------------------------------------------------------

#[cfg(feature = "esp32")]
fn tg_api(token: &str, method: &str) -> String {
    format!("https://api.telegram.org/bot{}/{}", token, method)
}

#[cfg(feature = "esp32")]
fn log_heap(tag: &str) {
    let free = unsafe { esp_idf_svc::sys::esp_get_free_heap_size() };
    let largest = unsafe { esp_idf_svc::sys::heap_caps_get_largest_free_block(4096) }; // MALLOC_CAP_DEFAULT
    log::info!("HEAP[{}]: free={}KB largest_block={}KB", tag, free / 1024, largest / 1024);
}

fn tg_http_get(url: &str) -> Result<String, String> {
    use esp_idf_svc::http::client::{Configuration as HttpConfig, EspHttpConnection};
    use esp_idf_svc::http::Method;

    log_heap("tg_get:pre");
    let config = HttpConfig {
        buffer_size: Some(1024),
        buffer_size_tx: Some(1024),
        timeout: Some(std::time::Duration::from_secs(30)),
        crt_bundle_attach: Some(esp_idf_svc::sys::esp_crt_bundle_attach),
        ..Default::default()
    };
    let mut conn = EspHttpConnection::new(&config).map_err(|e| format!("HTTP: {}", e))?;
    conn.initiate_request(Method::Get, url, &[]).map_err(|e| format!("req: {}", e))?;
    conn.initiate_response().map_err(|e| format!("resp: {}", e))?;
    let mut buf = [0u8; 2048];
    let mut body = Vec::new();
    loop {
        let n = conn.read(&mut buf).map_err(|e| format!("read: {}", e))?;
        if n == 0 { break; }
        body.extend_from_slice(&buf[..n]);
    }
    drop(conn);
    log_heap("tg_get:post");
    String::from_utf8(body).map_err(|e| format!("utf8: {}", e))
}

#[cfg(feature = "esp32")]
fn tg_http_post(url: &str, json_body: &str) -> Result<String, String> {
    use esp_idf_svc::http::client::{Configuration as HttpConfig, EspHttpConnection};
    use esp_idf_svc::http::Method;

    log_heap("tg_post:pre");
    let config = HttpConfig {
        buffer_size: Some(1024),
        buffer_size_tx: Some(1024),
        timeout: Some(std::time::Duration::from_secs(15)),
        crt_bundle_attach: Some(esp_idf_svc::sys::esp_crt_bundle_attach),
        ..Default::default()
    };
    let mut conn = EspHttpConnection::new(&config).map_err(|e| format!("HTTP: {}", e))?;
    let len = json_body.len().to_string();
    conn.initiate_request(Method::Post, url, &[
        ("Content-Type", "application/json"),
        ("Content-Length", &len),
    ]).map_err(|e| format!("req: {}", e))?;
    conn.write_all(json_body.as_bytes()).map_err(|e| format!("write: {}", e))?;
    conn.initiate_response().map_err(|e| format!("resp: {}", e))?;
    let mut buf = [0u8; 2048];
    let mut body = Vec::new();
    loop {
        let n = conn.read(&mut buf).map_err(|e| format!("read: {}", e))?;
        if n == 0 { break; }
        body.extend_from_slice(&buf[..n]);
    }
    drop(conn);
    log_heap("tg_post:post");
    String::from_utf8(body).map_err(|e| format!("utf8: {}", e))
}

#[cfg(feature = "esp32")]
fn telegram_poll_loop(
    token: &str,
    gateway: std::sync::Arc<zenclaw_agent::core::gateway::Gateway>,
) {
    let mut offset: i64 = 0;
    log::info!("Telegram poll loop running");

    loop {
        // Step 1: Poll — one TLS connection, then fully drop it
        let incoming = {
            let url = format!("{}?offset={}&timeout=15", tg_api(token, "getUpdates"), offset);
            let body = match tg_http_get(&url) {
                Ok(b) => b,
                Err(e) => {
                    log::error!("Telegram poll: {}", e);
                    std::thread::sleep(std::time::Duration::from_secs(5));
                    continue;
                }
            };
            // Parse and extract only (chat_id, text) pairs — drop raw JSON immediately
            let data: serde_json::Value = match serde_json::from_str(&body) {
                Ok(v) => v,
                Err(e) => {
                    log::error!("Telegram parse: {}", e);
                    std::thread::sleep(std::time::Duration::from_secs(5));
                    continue;
                }
            };
            drop(body); // Free raw string before extracting

            let mut msgs: Vec<(String, String)> = Vec::new();
            if let Some(updates) = data.get("result").and_then(|r| r.as_array()) {
                for update in updates {
                    if let Some(uid) = update.get("update_id").and_then(|v| v.as_i64()) {
                        if uid >= offset { offset = uid + 1; }
                    }
                    if let Some(msg) = update.get("message") {
                        let chat_id = msg.get("chat").and_then(|c| c.get("id"))
                            .and_then(|id| id.as_i64()).map(|id| id.to_string());
                        let text = msg.get("text").and_then(|t| t.as_str()).map(String::from);
                        if let (Some(cid), Some(txt)) = (chat_id, text) {
                            msgs.push((cid, txt));
                        }
                    }
                }
            }
            msgs
            // `data` dropped here — all JSON freed before any outbound calls
        };

        // Step 2: Process each message — one TLS connection at a time
        for (chat_id, text) in incoming {
            log::info!("Telegram msg from {}: {}B", chat_id, text.len());

            // Typing indicator — one connection, fully closed before next
            let typing = format!(r#"{{"chat_id":"{}","action":"typing"}}"#, chat_id);
            let _ = tg_http_post(&tg_api(token, "sendChatAction"), &typing);
            drop(typing);

            // LLM call — one connection inside gateway.chat()
            let reply = esp_idf_svc::hal::task::block_on(
                gateway.chat(&chat_id, &text, "telegram")
            );
            drop(text); // Free input before building reply payload

            let reply_text = match reply {
                Ok(r) => r,
                Err(e) => format!("Error: {}", e),
            };

            // Send reply — one connection
            let send = format!(
                r#"{{"chat_id":"{}","text":"{}","parse_mode":"Markdown"}}"#,
                chat_id,
                reply_text.replace('\\', "\\\\").replace('"', "\\\"").replace('\n', "\\n"),
            );
            drop(reply_text);
            match tg_http_post(&tg_api(token, "sendMessage"), &send) {
                Ok(_) => log::info!("Telegram reply sent to {}", chat_id),
                Err(e) => log::error!("Telegram send: {}", e),
            }
        }
    }
}

#[cfg(feature = "desktop")]
fn main() { unimplemented!() }
