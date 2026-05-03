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
}
