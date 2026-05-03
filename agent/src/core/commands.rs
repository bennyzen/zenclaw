//! Slash-command parser, executors, and Telegram menu list.
//!
//! Single source of truth for the four user-issued commands the agent
//! recognizes today: `/new`, `/clear`, `/status`, `/help`. The same
//! `menu()` const slice is consumed by both `parse()` (dispatch) and
//! `Poller::set_my_commands` (Telegram menu registration on boot) —
//! drift between the two is impossible by construction.
//!
//! Hook point: `Gateway::chat_with_events` calls `parse()` before
//! auto-compaction. Recognized commands skip the LLM entirely.

/// User-issued slash commands recognized by the agent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command {
    /// Alias for `Clear`.
    New,
    /// Wipe the current session for this `chat_id`. Preserves `SessionState`
    /// (model_override etc.) — only the conversation history goes.
    Clear,
    /// Render a markdown table of live device facts.
    Status,
    /// Static list of available commands.
    Help,
}

/// Network link description for `/status`.
#[derive(Debug, Clone)]
pub enum LinkKind {
    Wifi { ssid: String, rssi: Option<i32> },
    Ethernet,
    Desktop,
}

/// Live device facts assembled by `Gateway::runtime_facts(chat_id)` and
/// passed to `execute()`. Stable fields (`agent_name`, `platform`,
/// session size, model) come from `Gateway` directly; live fields come
/// from the `HostFacts` trait so platform-specific reads stay out of
/// `commands.rs` itself.
#[derive(Debug, Clone)]
pub struct RuntimeFacts {
    pub hostname: String,
    pub ip: Option<String>,
    pub link: LinkKind,
    pub free_internal_heap: Option<u32>,
    pub free_psram: Option<u32>,
    pub uptime_secs: u64,
    pub agent_name: String,
    pub platform: &'static str,
    pub session_bytes: u64,
    pub session_entries: usize,
    pub model: String,
}

/// Bridge from `Gateway` to platform-specific runtime reads.
///
/// `Esp32HostFacts` (in `main.rs`) reads heap/RSSI from `esp_idf_svc`.
/// `DesktopHostFacts` (in `desktop/host_facts.rs`) returns desktop-shaped
/// values (heap = `None`, link = `Desktop`).
pub trait HostFacts: Send + Sync {
    fn hostname(&self) -> String;
    fn ip(&self) -> Option<String>;
    fn link(&self) -> LinkKind;
    fn free_internal_heap(&self) -> Option<u32>;
    fn free_psram(&self) -> Option<u32>;
    fn uptime_secs(&self) -> u64;
}

/// Detect the build platform. Replaces the broken `cfg!(target_os)`
/// ladder in `session_tools.rs::do_status` which reported `unknown` on
/// every ESP32 build.
pub fn detect_platform() -> &'static str {
    if cfg!(target_os = "espidf") {
        if cfg!(target_arch = "xtensa") {
            "esp32-s3"
        } else if cfg!(target_arch = "riscv32") {
            "esp32-p4"
        } else {
            "espidf"
        }
    } else if cfg!(target_os = "linux") {
        "linux"
    } else if cfg!(target_os = "macos") {
        "macos"
    } else if cfg!(target_os = "windows") {
        "windows"
    } else {
        "unknown"
    }
}

/// Single source of truth for the BotFather menu and the parser.
///
/// Order matters — this list is what users see in Telegram's `/` menu,
/// so the most-used command (`/new`) goes first.
pub fn menu() -> &'static [(&'static str, &'static str)] {
    &[
        ("new",    "Start a fresh chat (alias for /clear)"),
        ("clear",  "Wipe the current chat history"),
        ("status", "Show device status (heap, link, model)"),
        ("help",   "List available commands"),
    ]
}

/// Execute a parsed slash command. Returns the user-visible reply.
///
/// `async` for forward-compat — v1 ops are sync but `/restart`, `/model`
/// (deferred to v2) will need NVS / HTTP I/O. Async now avoids breaking
/// callers later.
///
/// Note: `Clear` and `New` need access to `SessionManager`, which is on
/// `Gateway`. The signature in subsequent tasks will grow to include
/// `&SessionManager` + cloud handles. We start with the simplest shape
/// and extend.
pub async fn execute(
    cmd: Command,
    chat_id: &str,
    facts: &RuntimeFacts,
    sessions: &crate::core::sessions::SessionManager,
    cloud_store: Option<&dyn crate::core::cloud::client::ObjectStore>,
) -> String {
    match cmd {
        Command::Help   => render_help(),
        Command::Status => render_status(facts),
        Command::New | Command::Clear => clear_session(chat_id, sessions, cloud_store),
    }
}

fn clear_session(
    chat_id: &str,
    sessions: &crate::core::sessions::SessionManager,
    store: Option<&dyn crate::core::cloud::client::ObjectStore>,
) -> String {
    let result = match store {
        Some(s) => sessions.clear_with_store(chat_id, s),
        None    => sessions.clear(chat_id),
    };
    match result {
        Ok(()) => "Session cleared.".to_string(),
        Err(e) => format!("Failed to clear session: {}", e),
    }
}

fn render_help() -> String {
    let mut s = String::from("**Available commands:**\n\n");
    for (name, desc) in menu() {
        s.push_str(&format!("- `/{}` — {}\n", name, desc));
    }
    s
}

fn render_status(f: &RuntimeFacts) -> String {
    fn or_dash<T: std::fmt::Display>(v: Option<T>) -> String {
        v.map(|x| x.to_string()).unwrap_or_else(|| "—".to_string())
    }
    fn fmt_kb(bytes: Option<u32>) -> String {
        match bytes {
            Some(b) if b >= 1_000_000 => format!("{} MB", b / 1_000_000),
            Some(b) => format!("{} KB", b / 1000),
            None => "—".to_string(),
        }
    }
    let link = match &f.link {
        LinkKind::Wifi { ssid, rssi } => format!("WiFi {} ({} dBm)", ssid, or_dash(*rssi)),
        LinkKind::Ethernet => "Ethernet".to_string(),
        LinkKind::Desktop  => "Desktop".to_string(),
    };
    format!(
        "**{} Status**\n\n\
         | Field | Value |\n\
         |---|---|\n\
         | Hostname | `{}` |\n\
         | IP | {} |\n\
         | Link | {} |\n\
         | Platform | `{}` |\n\
         | Free internal heap | {} |\n\
         | Free PSRAM | {} |\n\
         | Uptime | {}s |\n\
         | Model | `{}` |\n\
         | Session | {} bytes, {} entries |\n",
        f.agent_name,
        f.hostname,
        or_dash(f.ip.as_deref()),
        link,
        f.platform,
        fmt_kb(f.free_internal_heap),
        fmt_kb(f.free_psram),
        f.uptime_secs,
        f.model,
        f.session_bytes,
        f.session_entries,
    )
}

/// Parse a user message into a recognized command.
///
/// Matches only when the message *starts* with `/<name>`. Trailing
/// arguments are ignored (no v1 command takes args). The Telegram
/// group-chat suffix `@<botname>` after the command name is stripped
/// before lookup.
pub fn parse(text: &str) -> Option<Command> {
    let rest = text.strip_prefix('/')?;

    // First whitespace OR newline ends the command token.
    let token_end = rest
        .find(|c: char| c.is_whitespace())
        .unwrap_or(rest.len());
    let token = &rest[..token_end];
    if token.is_empty() {
        return None;
    }

    // Strip Telegram group-chat suffix: "/status@zenclaw_bot".
    let name = token.split('@').next().unwrap_or(token);

    match name {
        "new"    => Some(Command::New),
        "clear"  => Some(Command::Clear),
        "status" => Some(Command::Status),
        "help"   => Some(Command::Help),
        _        => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn menu_lists_four_commands_with_descriptions() {
        let m = menu();
        assert_eq!(m.len(), 4);
        let names: Vec<&str> = m.iter().map(|(n, _)| *n).collect();
        assert_eq!(names, vec!["new", "clear", "status", "help"]);
        for (_, desc) in m {
            assert!(!desc.is_empty(), "every command needs a description");
        }
    }

    #[test]
    fn parse_recognizes_all_four_commands() {
        assert_eq!(parse("/new"), Some(Command::New));
        assert_eq!(parse("/clear"), Some(Command::Clear));
        assert_eq!(parse("/status"), Some(Command::Status));
        assert_eq!(parse("/help"), Some(Command::Help));
    }

    #[test]
    fn parse_strips_telegram_botname_suffix() {
        assert_eq!(parse("/status@zenclaw_bot"), Some(Command::Status));
        assert_eq!(parse("/clear@anything"), Some(Command::Clear));
    }

    #[test]
    fn parse_returns_none_for_unknown_commands() {
        assert_eq!(parse("/foo"), None);
        assert_eq!(parse("/"), None);
        assert_eq!(parse("hello"), None);
        assert_eq!(parse(""), None);
    }

    #[test]
    fn parse_ignores_trailing_args() {
        assert_eq!(parse("/clear extra trailing words"), Some(Command::Clear));
        assert_eq!(parse("/status\nmore stuff"), Some(Command::Status));
    }

    #[test]
    fn parse_only_matches_at_start() {
        assert_eq!(parse("not a command /clear"), None);
        assert_eq!(parse(" /clear"), None); // leading space — not at start
    }

    /// Drift guard: every command in `menu()` must round-trip through `parse()`.
    #[test]
    fn menu_entries_all_parse() {
        for (name, _) in menu() {
            let with_slash = format!("/{}", name);
            assert!(
                parse(&with_slash).is_some(),
                "menu lists `/{}` but parse() does not recognize it",
                name,
            );
        }
    }

    pub(super) struct FakeHostFacts {
        pub hostname: String,
        pub ip: Option<String>,
        pub link: LinkKind,
        pub heap: Option<u32>,
        pub psram: Option<u32>,
        pub uptime: u64,
    }

    impl FakeHostFacts {
        pub fn new() -> Self {
            Self {
                hostname: "test-host".to_string(),
                ip: Some("10.0.0.1".to_string()),
                link: LinkKind::Wifi { ssid: "test".to_string(), rssi: Some(-55) },
                heap: Some(120_000),
                psram: Some(7_500_000),
                uptime: 42,
            }
        }
    }

    impl HostFacts for FakeHostFacts {
        fn hostname(&self) -> String { self.hostname.clone() }
        fn ip(&self) -> Option<String> { self.ip.clone() }
        fn link(&self) -> LinkKind { self.link.clone() }
        fn free_internal_heap(&self) -> Option<u32> { self.heap }
        fn free_psram(&self) -> Option<u32> { self.psram }
        fn uptime_secs(&self) -> u64 { self.uptime }
    }

    #[test]
    fn detect_platform_returns_known_string_on_test_host() {
        let p = detect_platform();
        // Test host is always linux/macos/windows — never the espidf fallback.
        assert!(
            matches!(p, "linux" | "macos" | "windows"),
            "expected host platform, got {:?}",
            p,
        );
    }

    #[tokio::test]
    async fn execute_help_lists_all_commands_with_descriptions() {
        let facts = make_fake_runtime_facts();
        let (_tmp, sessions) = make_test_sessions();
        let out = execute(Command::Help, "chat-test", &facts, &sessions, None).await;
        for (name, desc) in menu() {
            assert!(out.contains(&format!("/{}", name)),
                "expected /{} in /help output, got: {}", name, out);
            assert!(out.contains(desc),
                "expected description {:?} in /help output", desc);
        }
    }

    fn make_test_sessions() -> (tempfile::TempDir, crate::core::sessions::SessionManager) {
        let dir = tempfile::tempdir().unwrap();
        let sub = dir.path().join("sessions");
        std::fs::create_dir_all(&sub).unwrap();
        let mgr = crate::core::sessions::SessionManager::new(sub.to_str().unwrap());
        (dir, mgr)
    }

    #[tokio::test]
    async fn execute_status_renders_full_facts() {
        let facts = make_fake_runtime_facts();
        let (_tmp, sessions) = make_test_sessions();
        let out = execute(Command::Status, "chat-test", &facts, &sessions, None).await;
        // Header
        assert!(out.contains("TestAgent"), "agent_name missing: {}", out);
        // Identity rows
        assert!(out.contains("test-host"));
        assert!(out.contains("10.0.0.1"));
        // Platform fix — the bug we set out to fix.
        assert!(out.contains("test"));
        // Link
        assert!(out.contains("test") && out.contains("-55"),
            "WiFi SSID and RSSI missing: {}", out);
        // Heap (formatted in KB or MB)
        assert!(out.contains("120"), "heap missing: {}", out);
        assert!(out.contains("7"), "psram missing: {}", out);
        // Model
        assert!(out.contains("test-model"));
    }

    #[tokio::test]
    async fn execute_status_renders_em_dash_for_missing_fields() {
        let mut facts = make_fake_runtime_facts();
        facts.ip = None;
        facts.free_internal_heap = None;
        facts.free_psram = None;
        facts.link = LinkKind::Ethernet;
        let (_tmp, sessions) = make_test_sessions();
        let out = execute(Command::Status, "chat-test", &facts, &sessions, None).await;
        // Em-dash placeholder (—) appears for unknown fields rather than
        // dropping the row entirely.
        assert!(out.contains("—"), "expected em-dash for missing fields: {}", out);
        assert!(out.contains("Ethernet"), "Ethernet link missing: {}", out);
    }

    fn make_fake_runtime_facts() -> RuntimeFacts {
        let h = FakeHostFacts::new();
        RuntimeFacts {
            hostname: h.hostname(),
            ip: h.ip(),
            link: h.link(),
            free_internal_heap: h.free_internal_heap(),
            free_psram: h.free_psram(),
            uptime_secs: h.uptime_secs(),
            agent_name: "TestAgent".to_string(),
            platform: "test",
            session_bytes: 0,
            session_entries: 0,
            model: "test-model".to_string(),
        }
    }

    #[tokio::test]
    async fn execute_clear_deletes_session_file() {
        let (_tmp, sessions) = make_test_sessions();
        let path = format!("{}/abc.jsonl", sessions.sessions_dir());
        std::fs::write(&path, "{}\n").unwrap();

        let facts = make_fake_runtime_facts();
        let out = execute(Command::Clear, "abc", &facts, &sessions, None).await;

        assert!(out.contains("cleared"), "out was: {}", out);
        assert!(!std::path::Path::new(&path).exists(),
            "session file should have been deleted");
    }

    #[tokio::test]
    async fn execute_clear_preserves_model_override() {
        let (_tmp, sessions_owned) = make_test_sessions();
        // SessionManager::set_state is `&mut self`, so we need ownership.
        let mut sessions = sessions_owned;
        sessions.set_state("abc", crate::core::sessions::SessionState {
            turn_count: 0,
            model_override: Some("gpt-4".to_string()),
            last_channel: None,
        });
        let path = format!("{}/abc.jsonl", sessions.sessions_dir());
        std::fs::write(&path, "{}\n").unwrap();

        let facts = make_fake_runtime_facts();
        let _out = execute(Command::Clear, "abc", &facts, &sessions, None).await;

        let st = sessions.get_state("abc");
        assert_eq!(st.model_override, Some("gpt-4".to_string()),
            "model_override must survive /clear");
    }

    #[tokio::test]
    async fn execute_new_is_alias_for_clear() {
        let (_tmp, sessions) = make_test_sessions();
        let path = format!("{}/abc.jsonl", sessions.sessions_dir());
        std::fs::write(&path, "{}\n").unwrap();

        let facts = make_fake_runtime_facts();
        let out = execute(Command::New, "abc", &facts, &sessions, None).await;

        assert!(out.contains("cleared"));
        assert!(!std::path::Path::new(&path).exists());
    }
}
