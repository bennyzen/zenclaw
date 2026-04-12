use std::sync::Arc;
use tracing::info;

use crate::config::Config;
use crate::core::cron::{CronService, CronStore};

/// Background task runner for heartbeat, cron, and subagent reaping.
pub struct BackgroundRunner {
    config: Arc<Config>,
    data_dir: String,
}

impl BackgroundRunner {
    pub fn new(config: Arc<Config>, data_dir: String) -> Self {
        Self { config, data_dir }
    }

    fn cron_jobs_path(&self) -> String {
        format!("{}/cron/jobs.json", self.data_dir)
    }

    /// Check for due cron jobs and log them.
    fn check_cron_jobs(&self) {
        let store = CronStore::new(self.cron_jobs_path());
        let service = CronService::new(store);
        let due = service.get_due_jobs();

        if due.is_empty() {
            return;
        }

        for job in &due {
            info!(
                job_id = %job.id,
                job_name = %job.name,
                "CRON: job due (execution via gateway not yet wired)"
            );
            // TODO: dispatch job through gateway for actual execution
        }
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
        let mut cron_counter: u64 = 0;

        loop {
            tokio::select! {
                _ = cancel.cancelled() => {
                    info!("Background runner stopping");
                    break;
                }
                _ = interval.tick() => {
                    heartbeat_counter += 1;
                    cron_counter += 1;

                    // Check cron jobs every 60 seconds
                    if cron_counter >= 60 {
                        cron_counter = 0;
                        self.check_cron_jobs();
                    }

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
