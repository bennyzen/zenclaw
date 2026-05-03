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
}
