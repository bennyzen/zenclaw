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
pub async fn execute(cmd: Command, facts: &RuntimeFacts) -> String {
    match cmd {
        Command::Help => render_help(),
        // Other arms wired in later tasks.
        _ => format!("(command {:?} not yet implemented)", cmd),
    }
}

fn render_help() -> String {
    let mut s = String::from("**Available commands:**\n\n");
    for (name, desc) in menu() {
        s.push_str(&format!("- `/{}` — {}\n", name, desc));
    }
    s
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
        let out = execute(Command::Help, &facts).await;
        for (name, desc) in menu() {
            assert!(out.contains(&format!("/{}", name)),
                "expected /{} in /help output, got: {}", name, out);
            assert!(out.contains(desc),
                "expected description {:?} in /help output", desc);
        }
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
}
