pub mod agent_loop;
pub mod cloud;
pub mod compaction;
pub mod cron;
pub mod gateway;
pub mod memory;
pub mod prompt;
pub mod sessions;
pub mod tool_loop;
pub mod tools;
pub mod types;
pub mod workspace;

pub mod runner;

// These depend on genai/reqwest/teloxide — desktop only
#[cfg(feature = "desktop")]
pub mod background;
#[cfg(feature = "desktop")]
pub mod channels;
#[cfg(feature = "desktop")]
pub mod subagents;
#[cfg(feature = "desktop")]
pub mod telegram;
