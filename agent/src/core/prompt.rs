use crate::config::Config;
use crate::core::types::ToolDefinition;
use crate::core::workspace::ContextFile;

const ZENCLAW_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Tool descriptions shown in the system prompt tooling section.
/// Maps tool name -> human-readable description.
const TOOL_DESCRIPTIONS: &[(&str, &str)] = &[
    ("file", "File operations: read, write, edit, delete, list_dir"),
    ("memory", "Persistent memory: save/search/list/get/edit/delete entries that outlive this chat"),
    ("cron", "Scheduled tasks: add/list/remove/run/update"),
    ("web", "Web access: fetch a URL or search the web (Brave)"),
    ("hub_search", "Search TinyHub for skills, extensions, and tools"),
    ("message_send", "Send a cross-channel message"),
    ("session", "Session management: status (time/uptime/memory/model), list, history"),
    ("gateway", "Gateway management: status/reload"),
    ("subagents", "Spawn, list, or cancel background sub-agents"),
    ("storage", "Cloud storage (S3): read/write/delete/list/info/grep/analyze"),
];

/// Build the full system prompt from config, tools, context files, and runtime info.
pub fn build_system_prompt(
    config: &Config,
    tools: &[ToolDefinition],
    context_files: &[ContextFile],
    channel: Option<&str>,
    chat_id: Option<&str>,
) -> String {
    let mut sections: Vec<String> = Vec::new();

    sections.push(build_identity_section(config));
    sections.push(String::new());
    sections.push(build_platform_section());
    sections.push(String::new());
    sections.push(build_time_section());
    sections.push(String::new());
    sections.push(build_tooling_section(tools));
    sections.push(String::new());

    if let Some(ch) = channel {
        sections.push(format!("Current channel: **{}**", ch));
        sections.push(String::new());
    }

    if !context_files.is_empty() {
        sections.push(build_context_section(context_files));
        sections.push(String::new());
    }

    sections.push(build_runtime_line(config, channel, chat_id));

    sections.join("\n")
}

fn get_hostname() -> String {
    #[cfg(feature = "desktop")]
    {
        hostname::get()
            .ok()
            .and_then(|h| h.into_string().ok())
            .unwrap_or_default()
    }
    #[cfg(not(feature = "desktop"))]
    {
        String::new()
    }
}

fn build_identity_section(config: &Config) -> String {
    let hostname = get_hostname();

    let location = if hostname.is_empty() {
        " on an embedded device".to_string()
    } else {
        format!(" on {}", hostname)
    };

    format!(
        "You are {}, running{}. Read SOUL.md for who you are.",
        config.agent_name, location
    )
}

fn build_platform_section() -> String {
    let mut lines = vec!["## Platform".to_string()];

    lines.push(format!(
        "{} — {} (Rust {})",
        std::env::consts::ARCH,
        std::env::consts::OS,
        env!("CARGO_PKG_VERSION"),
    ));

    lines.push(String::new());
    lines.push("Storage: data/ (persistent)".to_string());

    lines.join("\n")
}

fn build_time_section() -> String {
    let mut lines = vec!["## Current Date & Time".to_string()];

    // Timezone detection — best-effort from env
    let tz = std::env::var("TZ").unwrap_or_else(|_| "UTC".to_string());
    lines.push(format!("Time zone: {}", tz));

    lines.push(
        "If you need the current date, time, day of week, or uptime, run \
         session(action=\"status\"). These values change constantly — always re-call the tool, \
         never reuse values from earlier in the conversation."
            .to_string(),
    );

    lines.join("\n")
}

fn build_tooling_section(tools: &[ToolDefinition]) -> String {
    let mut lines = vec![
        "## Tooling".to_string(),
        "These are your built-in tools — always available.".to_string(),
        "Tool names are case-sensitive. Call tools exactly as listed. \
         Many tools use an action parameter to select the operation."
            .to_string(),
    ];

    if tools.is_empty() {
        lines.push("No tools are currently registered.".to_string());
    } else {
        let desc_map: std::collections::HashMap<&str, &str> =
            TOOL_DESCRIPTIONS.iter().copied().collect();

        let mut names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
        names.sort();

        for name in names {
            if let Some(desc) = desc_map.get(name) {
                lines.push(format!("- {}: {}", name, desc));
            } else {
                lines.push(format!("- {}", name));
            }
        }
    }

    lines.push(String::new());
    lines.push(
        "Use tools proactively when they can help. \
         Prefer reading files over guessing their contents."
            .to_string(),
    );
    lines.push(
        "Memory is fetch-on-demand: nothing is auto-loaded. Before responding when the user might \
         have told you something relevant before (preferences, prior decisions, project context), \
         call memory(action=search) or memory(action=list). When the user states a preference, fact \
         about themselves, or a decision worth remembering, call memory(action=save) — never just say \
         \"I'll remember\". When a memory save/edit/delete result reports >=70% capacity, surface this \
         to the user and propose a specific compaction plan; never groom memory unilaterally — wait \
         for explicit approval."
            .to_string(),
    );
    lines.push(
        "Multi-step requests in ONE turn: when a user asks for work that takes several \
         actions (e.g. \"find every X, do Y to each, then Z\"), execute the FULL chain \
         of tool calls in this turn — keep emitting tool_calls as long as work remains. \
         Do NOT narrate next steps in plain text (\"Now let me read each one...\") and \
         then stop without calling them — that defers work the user asked you to \
         complete now. Only emit a final text response when every action is actually \
         done or you genuinely cannot proceed."
            .to_string(),
    );
    lines.push(
        "Your device state changes between turns — settings get configured, \
         connections come online, files get written. If a tool call fails, retry it \
         before reporting the error; do not assume the fault persists."
            .to_string(),
    );
    lines.push(
        "If a task is more complex or takes longer, spawn a sub-agent with subagents. \
         Completion is push-based: it will auto-announce when done."
            .to_string(),
    );
    lines.push(
        "Vision: Photos sent in Telegram are automatically visible to you. \
         When a user sends a photo, you can see it directly — describe, analyze, \
         or respond to it without needing any tool call."
            .to_string(),
    );

    lines.join("\n")
}

fn build_context_section(context_files: &[ContextFile]) -> String {
    let mut lines = vec![
        "# Project Context".to_string(),
        String::new(),
        "The following project context files have been loaded:".to_string(),
        String::new(),
    ];

    for f in context_files {
        lines.push(f.content.clone());
        lines.push(String::new());
    }

    lines.join("\n")
}

fn build_runtime_line(config: &Config, channel: Option<&str>, chat_id: Option<&str>) -> String {
    let mut parts = vec![format!("agent={}", config.agent_name)];

    let h = get_hostname();
    if !h.is_empty() {
        parts.push(format!("host={}", h));
    }

    // Resolve model from default provider
    let model = resolve_default_model(config);
    parts.push(format!("model={}", model));

    let provider_name = &config.providers.default;
    parts.push(format!("provider={}", provider_name));

    if let Some(ch) = channel {
        parts.push(format!("channel={}", ch));
    }
    if let Some(cid) = chat_id {
        parts.push(format!("session={}", cid));
    }

    parts.push(format!("version=ZenClaw-{}", ZENCLAW_VERSION));
    parts.push(format!(
        "platform=Rust-{}",
        std::env::consts::ARCH,
    ));

    let tz = std::env::var("TZ").unwrap_or_else(|_| "UTC".to_string());
    parts.push(format!("timezone={}", tz));

    format!("Runtime: {}", parts.join(" | "))
}

/// Resolve the model name from the default provider config.
fn resolve_default_model(config: &Config) -> String {
    config
        .providers
        .entries
        .get(&config.providers.default)
        .and_then(|e| e.model.clone())
        .unwrap_or_else(|| "unknown".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_config() -> Config {
        serde_json::from_str(
            r#"{
                "agent_name": "TestBot",
                "providers": {
                    "default": "google",
                    "google": {
                        "model": "gemini-2.5-flash",
                        "api_key": "test-key"
                    }
                }
            }"#,
        )
        .unwrap()
    }

    #[test]
    fn build_prompt_has_identity() {
        let config = test_config();
        let prompt = build_system_prompt(&config, &[], &[], None, None);
        assert!(prompt.contains("You are TestBot"));
        assert!(prompt.contains("SOUL.md"));
    }

    #[test]
    fn build_prompt_has_platform() {
        let config = test_config();
        let prompt = build_system_prompt(&config, &[], &[], None, None);
        assert!(prompt.contains("## Platform"));
    }

    #[test]
    fn build_prompt_has_tooling() {
        let config = test_config();
        let tools = vec![ToolDefinition {
            name: "file".to_string(),
            description: "File ops".to_string(),
            parameters: serde_json::json!({}),
        }];
        let prompt = build_system_prompt(&config, &tools, &[], None, None);
        assert!(prompt.contains("## Tooling"));
        assert!(prompt.contains("- file:"));
    }

    #[test]
    fn build_prompt_no_tools() {
        let config = test_config();
        let prompt = build_system_prompt(&config, &[], &[], None, None);
        assert!(prompt.contains("No tools are currently registered."));
    }

    #[test]
    fn build_prompt_includes_channel() {
        let config = test_config();
        let prompt = build_system_prompt(&config, &[], &[], Some("telegram"), None);
        assert!(prompt.contains("Current channel: **telegram**"));
    }

    #[test]
    fn build_prompt_includes_context_files() {
        let config = test_config();
        let files = vec![ContextFile {
            name: "SOUL.md".to_string(),
            path: "data/SOUL.md".to_string(),
            content: "I am a helpful AI.".to_string(),
        }];
        let prompt = build_system_prompt(&config, &[], &files, None, None);
        assert!(prompt.contains("Project Context"));
        assert!(prompt.contains("I am a helpful AI."));
    }

    #[test]
    fn build_prompt_runtime_line() {
        let config = test_config();
        let prompt = build_system_prompt(&config, &[], &[], Some("cli"), Some("test-session"));
        assert!(prompt.contains("Runtime:"));
        assert!(prompt.contains("agent=TestBot"));
        assert!(prompt.contains("model=gemini-2.5-flash"));
        assert!(prompt.contains("channel=cli"));
        assert!(prompt.contains("session=test-session"));
        assert!(prompt.contains("version=ZenClaw-"));
        assert!(prompt.contains("platform=Rust-"));
    }
}
