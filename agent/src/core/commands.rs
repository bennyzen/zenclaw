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
