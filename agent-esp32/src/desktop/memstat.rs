//! Linux memory instrumentation for the desktop runtime.
//!
//! Gives `/api/status` a real `memory.*` reading so the desktop workbench
//! exposes the same observability surface as ESP32 (which serves
//! `esp_idf_svc::sys::heap_caps_get_free_size`). All values in kibibytes.

use std::fs;

#[derive(Debug, Clone, Copy)]
pub struct MemStats {
    /// Resident set size of this process.
    pub rss_kb: u64,
    /// Peak RSS since process start (best-effort; 0 if unavailable).
    pub rss_peak_kb: u64,
    /// System-wide MemAvailable — the analog to ESP32 free_heap.
    pub system_available_kb: u64,
    /// System-wide MemTotal.
    pub system_total_kb: u64,
}

impl MemStats {
    pub fn read() -> Option<Self> {
        let status = fs::read_to_string("/proc/self/status").ok()?;
        let meminfo = fs::read_to_string("/proc/meminfo").ok()?;
        Some(Self {
            rss_kb: parse_kb(&status, "VmRSS:")?,
            rss_peak_kb: parse_kb(&status, "VmPeak:").unwrap_or(0),
            system_available_kb: parse_kb(&meminfo, "MemAvailable:")?,
            system_total_kb: parse_kb(&meminfo, "MemTotal:")?,
        })
    }
}

fn parse_kb(text: &str, prefix: &str) -> Option<u64> {
    text.lines()
        .find_map(|line| line.strip_prefix(prefix))
        .and_then(|rest| rest.split_whitespace().next())
        .and_then(|num| num.parse().ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_kb_extracts_first_number() {
        let sample = "VmRSS:\t   42360 kB\nVmPeak:\t   88888 kB\n";
        assert_eq!(parse_kb(sample, "VmRSS:"), Some(42360));
        assert_eq!(parse_kb(sample, "VmPeak:"), Some(88888));
        assert_eq!(parse_kb(sample, "VmNope:"), None);
    }

    #[test]
    fn read_returns_some_on_linux() {
        // /proc is always present on the targets we ship desktop builds for.
        let stats = MemStats::read().expect("/proc unreadable");
        assert!(stats.rss_kb > 0);
        assert!(stats.system_total_kb > stats.system_available_kb);
    }
}
