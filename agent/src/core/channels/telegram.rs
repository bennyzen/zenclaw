//! Telegram bot integration — Poller (long-poll receiver) and TelegramChannel
//! (sender, impl Channel). Both go through `&dyn HttpClient` so they work
//! identically on ESP32 and desktop.
//!
//! Filled in commit B (telegram unification).
