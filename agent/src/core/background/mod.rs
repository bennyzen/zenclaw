/// Background task runner for heartbeat and scheduled jobs.
pub struct BackgroundRunner {
    enabled: bool,
    interval_secs: u64,
}

impl BackgroundRunner {
    pub fn new(enabled: bool, interval_secs: u64) -> Self {
        Self {
            enabled,
            interval_secs,
        }
    }

    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    pub fn interval_secs(&self) -> u64 {
        self.interval_secs
    }

    // TODO: implement run loop
}
