//! `HostFacts` impl for the desktop build.
//!
//! Heap fields are `None` (no equivalent of `esp_get_free_heap_size`
//! that's worth wiring up for desktop dev). Link is always `Desktop`.
//! Hostname comes from the `hostname` crate (already a desktop dep).

use crate::core::commands::{HostFacts, LinkKind};
use std::time::Instant;

pub struct DesktopHostFacts {
    started: Instant,
    hostname: String,
}

impl DesktopHostFacts {
    pub fn new() -> Self {
        Self {
            started: Instant::now(),
            hostname: hostname::get()
                .ok()
                .and_then(|s| s.into_string().ok())
                .unwrap_or_else(|| "desktop".to_string()),
        }
    }
}

impl Default for DesktopHostFacts {
    fn default() -> Self {
        Self::new()
    }
}

impl HostFacts for DesktopHostFacts {
    fn hostname(&self) -> String { self.hostname.clone() }
    fn ip(&self) -> Option<String> { None } // desktop dev — not exposing local IP
    fn link(&self) -> LinkKind { LinkKind::Desktop }
    fn free_internal_heap(&self) -> Option<u32> { None }
    fn free_psram(&self) -> Option<u32> { None }
    fn uptime_secs(&self) -> u64 { self.started.elapsed().as_secs() }
}
