use std::sync::Arc;
use tracing::info;

use crate::config::Config;

/// Background task runner for heartbeat, cron, and subagent reaping.
pub struct BackgroundRunner {
    config: Arc<Config>,
}

impl BackgroundRunner {
    pub fn new(config: Arc<Config>) -> Self {
        Self { config }
    }

    /// Start the background loop. Runs until cancelled.
    pub async fn run(&self, cancel: tokio_util::sync::CancellationToken) {
        let heartbeat_enabled = self.config.heartbeat.enabled;
        let heartbeat_secs = self.config.heartbeat.every_secs;

        info!(
            heartbeat_enabled,
            heartbeat_secs, "Background runner started"
        );

        let mut interval = tokio::time::interval(std::time::Duration::from_secs(1));
        let mut heartbeat_counter: u64 = 0;

        loop {
            tokio::select! {
                _ = cancel.cancelled() => {
                    info!("Background runner stopping");
                    break;
                }
                _ = interval.tick() => {
                    heartbeat_counter += 1;

                    // TODO: check cron jobs every 60s
                    // TODO: reap finished subagents

                    if heartbeat_enabled && heartbeat_counter >= heartbeat_secs {
                        heartbeat_counter = 0;
                        info!("Heartbeat tick (stub)");
                        // TODO: run heartbeat prompt through gateway
                    }
                }
            }
        }
    }
}
