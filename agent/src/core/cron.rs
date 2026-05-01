use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};
use tracing::{info, warn};

/// Backoff schedule for consecutive errors (milliseconds).
const ERROR_BACKOFF_SCHEDULE_MS: [u64; 5] = [30_000, 60_000, 300_000, 900_000, 3_600_000];

/// Auto-disable a job after this many consecutive errors.
const MAX_CONSECUTIVE_ERRORS: u32 = 10;

fn epoch_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn error_backoff_ms(consecutive_errors: u32) -> u64 {
    let idx =
        (consecutive_errors.saturating_sub(1) as usize).min(ERROR_BACKOFF_SCHEDULE_MS.len() - 1);
    ERROR_BACKOFF_SCHEDULE_MS[idx]
}

fn generate_id() -> String {
    format!("job-{}", epoch_ms() % 1_000_000)
}

// ---------------------------------------------------------------------------
// CronSchedule
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum CronSchedule {
    /// One-shot: fires once at `at_ms` epoch milliseconds.
    At {
        #[serde(rename = "atMs")]
        at_ms: u64,
    },
    /// Recurring: fires every `every_ms` milliseconds, anchored at `anchor_ms`.
    Every {
        #[serde(rename = "everyMs")]
        every_ms: u64,
        #[serde(rename = "anchorMs", skip_serializing_if = "Option::is_none")]
        anchor_ms: Option<u64>,
    },
}

impl CronSchedule {
    pub fn at(at_ms: u64) -> Self {
        CronSchedule::At { at_ms }
    }

    pub fn every(every_ms: u64, anchor_ms: Option<u64>) -> Self {
        CronSchedule::Every {
            every_ms,
            anchor_ms,
        }
    }

    /// Compute the next run time in epoch ms, or `None` if the job will never fire again.
    pub fn compute_next_run(&self, now_ms: u64) -> Option<u64> {
        match self {
            CronSchedule::At { at_ms } => {
                if *at_ms > now_ms {
                    Some(*at_ms)
                } else {
                    None
                }
            }
            CronSchedule::Every {
                every_ms,
                anchor_ms,
            } => {
                let interval = (*every_ms).max(1);
                let anchor = anchor_ms.unwrap_or(now_ms);
                if now_ms < anchor {
                    return Some(anchor);
                }
                let elapsed = now_ms - anchor;
                let periods = ((elapsed + interval - 1) / interval).max(1);
                Some(anchor + periods * interval)
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Payload & Delivery
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum CronPayload {
    AgentTurn { text: String },
    SystemEvent { text: String },
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct CronDelivery {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub channel: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub chat_id: Option<String>,
}

// ---------------------------------------------------------------------------
// Job state
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct CronJobState {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_run_at_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_run_at_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_status: Option<String>,
    #[serde(default)]
    pub consecutive_errors: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub running_at_ms: Option<u64>,
}

// ---------------------------------------------------------------------------
// CronJob
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CronJob {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub delete_after_run: bool,
    pub schedule: CronSchedule,
    pub payload: CronPayload,
    #[serde(default)]
    pub delivery: CronDelivery,
    #[serde(default)]
    pub state: CronJobState,
    #[serde(default)]
    pub created_at_ms: u64,
    #[serde(default)]
    pub updated_at_ms: u64,
}

fn default_true() -> bool {
    true
}

impl CronJob {
    /// Returns true if the job is enabled, not currently running, and past its next_run_at_ms.
    pub fn is_due(&self, now_ms: u64) -> bool {
        if !self.enabled {
            return false;
        }
        if self.state.running_at_ms.is_some() {
            return false;
        }
        match self.state.next_run_at_ms {
            Some(next) => next <= now_ms,
            None => false,
        }
    }

    /// Recompute and store next_run_at_ms from the schedule.
    pub fn compute_next_run(&mut self, now_ms: u64) -> Option<u64> {
        let next = self.schedule.compute_next_run(now_ms);
        self.state.next_run_at_ms = next;
        next
    }
}

// ---------------------------------------------------------------------------
// CronStore — file-backed job persistence
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize, Deserialize)]
struct StoreFile {
    version: u32,
    jobs: Vec<CronJob>,
}

pub struct CronStore {
    path: String,
    jobs: HashMap<String, CronJob>,
}

impl CronStore {
    pub fn new(path: String) -> Self {
        let mut store = Self {
            path,
            jobs: HashMap::new(),
        };
        store.load();
        store
    }

    fn load(&mut self) {
        let data = match std::fs::read_to_string(&self.path) {
            Ok(d) => d,
            Err(_) => {
                self.jobs = HashMap::new();
                return;
            }
        };
        match serde_json::from_str::<StoreFile>(&data) {
            Ok(sf) => {
                self.jobs = sf.jobs.into_iter().map(|j| (j.id.clone(), j)).collect();
            }
            Err(e) => {
                warn!("Failed to parse cron jobs file: {}", e);
                self.jobs = HashMap::new();
            }
        }
    }

    fn save(&self) {
        // Ensure parent directory exists
        if let Some(parent) = std::path::Path::new(&self.path).parent() {
            let _ = std::fs::create_dir_all(parent);
        }

        let sf = StoreFile {
            version: 1,
            jobs: self.jobs.values().cloned().collect(),
        };
        match serde_json::to_string_pretty(&sf) {
            Ok(data) => {
                if let Err(e) = std::fs::write(&self.path, data) {
                    warn!("Failed to write cron jobs file: {}", e);
                }
            }
            Err(e) => warn!("Failed to serialize cron jobs: {}", e),
        }
    }

    pub fn add(&mut self, job: CronJob) -> CronJob {
        self.jobs.insert(job.id.clone(), job.clone());
        self.save();
        job
    }

    pub fn remove(&mut self, job_id: &str) -> bool {
        if self.jobs.remove(job_id).is_some() {
            self.save();
            true
        } else {
            false
        }
    }

    pub fn get(&self, job_id: &str) -> Option<&CronJob> {
        self.jobs.get(job_id)
    }

    pub fn get_mut(&mut self, job_id: &str) -> Option<&mut CronJob> {
        self.jobs.get_mut(job_id)
    }

    pub fn list(&self) -> Vec<&CronJob> {
        self.jobs.values().collect()
    }

    pub fn update(&mut self, job: CronJob) -> bool {
        if self.jobs.contains_key(&job.id) {
            self.jobs.insert(job.id.clone(), job);
            self.save();
            true
        } else {
            false
        }
    }
}

// ---------------------------------------------------------------------------
// CronService — high-level operations over CronStore
// ---------------------------------------------------------------------------

pub struct CronService {
    store: CronStore,
}

impl CronService {
    pub fn new(store: CronStore) -> Self {
        Self { store }
    }

    pub fn add_job(
        &mut self,
        name: String,
        schedule: CronSchedule,
        payload: CronPayload,
        delivery: CronDelivery,
        delete_after_run: bool,
    ) -> CronJob {
        let now = epoch_ms();
        let mut job = CronJob {
            id: generate_id(),
            name,
            description: String::new(),
            enabled: true,
            delete_after_run,
            schedule,
            payload,
            delivery,
            state: CronJobState::default(),
            created_at_ms: now,
            updated_at_ms: now,
        };
        job.compute_next_run(now);
        self.store.add(job)
    }

    pub fn remove_job(&mut self, job_id: &str) -> bool {
        self.store.remove(job_id)
    }

    pub fn list_jobs(&self) -> Vec<&CronJob> {
        self.store.list()
    }

    pub fn get_job(&self, job_id: &str) -> Option<&CronJob> {
        self.store.get(job_id)
    }

    pub fn get_due_jobs(&self) -> Vec<CronJob> {
        let now = epoch_ms();
        self.store
            .list()
            .into_iter()
            .filter(|j| j.is_due(now))
            .cloned()
            .collect()
    }

    /// Apply the result of running a job (ok or error), handle backoff/auto-disable.
    pub fn apply_job_result(&mut self, job_id: &str, success: bool, _error_msg: Option<&str>) {
        let now = epoch_ms();

        // Read fields we need before mutable borrow
        let (delete_after_run, job_exists) = match self.store.get(job_id) {
            Some(job) => (job.delete_after_run, true),
            None => return,
        };

        if !job_exists {
            return;
        }

        // Now do the mutable work
        let job = self.store.get_mut(job_id).unwrap();

        if success {
            job.state.last_status = Some("ok".to_string());
            job.state.consecutive_errors = 0;
        } else {
            job.state.last_status = Some("error".to_string());
            job.state.consecutive_errors += 1;
        }

        job.state.last_run_at_ms = Some(now);
        job.state.running_at_ms = None;

        // Auto-disable after too many errors
        if job.state.consecutive_errors >= MAX_CONSECUTIVE_ERRORS {
            job.enabled = false;
            info!(
                "CRON: auto-disabled {} after {} errors",
                job.name, job.state.consecutive_errors
            );
        }

        if delete_after_run && success {
            self.store.remove(job_id);
        } else {
            // Recompute next run
            let job = self.store.get_mut(job_id).unwrap();
            job.compute_next_run(now);

            // Apply error backoff
            if job.state.consecutive_errors > 0 {
                let backoff = error_backoff_ms(job.state.consecutive_errors);
                if let Some(next) = job.state.next_run_at_ms {
                    job.state.next_run_at_ms = Some(next.max(now + backoff));
                }
                info!(
                    "CRON: backoff job={} errors={} delay={}ms",
                    job.name, job.state.consecutive_errors, backoff
                );
            }

            job.updated_at_ms = now;
            let updated = job.clone();
            self.store.update(updated);
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn compute_next_run_at_future() {
        let sched = CronSchedule::at(5000);
        assert_eq!(sched.compute_next_run(3000), Some(5000));
    }

    #[test]
    fn compute_next_run_at_past() {
        let sched = CronSchedule::at(1000);
        assert_eq!(sched.compute_next_run(2000), None);
    }

    #[test]
    fn compute_next_run_every_before_anchor() {
        let sched = CronSchedule::every(60_000, Some(10_000));
        assert_eq!(sched.compute_next_run(5_000), Some(10_000));
    }

    #[test]
    fn compute_next_run_every_at_anchor() {
        let sched = CronSchedule::every(60_000, Some(10_000));
        // At exactly the anchor, next run should be anchor + interval
        assert_eq!(sched.compute_next_run(10_000), Some(70_000));
    }

    #[test]
    fn compute_next_run_every_after_anchor() {
        let sched = CronSchedule::every(60_000, Some(10_000));
        // 25s after anchor, next should be anchor + 60s
        assert_eq!(sched.compute_next_run(35_000), Some(70_000));
    }

    #[test]
    fn compute_next_run_every_multiple_periods() {
        let sched = CronSchedule::every(60_000, Some(10_000));
        // Well past two periods
        assert_eq!(sched.compute_next_run(150_000), Some(190_000));
    }

    #[test]
    fn compute_next_run_every_no_anchor() {
        let sched = CronSchedule::every(60_000, None);
        // With no anchor, anchor defaults to now_ms, so next = now + 60s
        assert_eq!(sched.compute_next_run(100_000), Some(160_000));
    }

    #[test]
    fn job_is_due() {
        let job = CronJob {
            id: "test".to_string(),
            name: "test".to_string(),
            description: String::new(),
            enabled: true,
            delete_after_run: false,
            schedule: CronSchedule::every(60_000, None),
            payload: CronPayload::AgentTurn {
                text: "hello".to_string(),
            },
            delivery: CronDelivery::default(),
            state: CronJobState {
                next_run_at_ms: Some(1000),
                ..Default::default()
            },
            created_at_ms: 0,
            updated_at_ms: 0,
        };

        assert!(job.is_due(1000));
        assert!(job.is_due(2000));
        assert!(!job.is_due(999));
    }

    #[test]
    fn job_not_due_when_disabled() {
        let job = CronJob {
            id: "test".to_string(),
            name: "test".to_string(),
            description: String::new(),
            enabled: false,
            delete_after_run: false,
            schedule: CronSchedule::every(60_000, None),
            payload: CronPayload::AgentTurn {
                text: "hello".to_string(),
            },
            delivery: CronDelivery::default(),
            state: CronJobState {
                next_run_at_ms: Some(1000),
                ..Default::default()
            },
            created_at_ms: 0,
            updated_at_ms: 0,
        };

        assert!(!job.is_due(2000));
    }

    #[test]
    fn job_not_due_when_running() {
        let job = CronJob {
            id: "test".to_string(),
            name: "test".to_string(),
            description: String::new(),
            enabled: true,
            delete_after_run: false,
            schedule: CronSchedule::every(60_000, None),
            payload: CronPayload::AgentTurn {
                text: "hello".to_string(),
            },
            delivery: CronDelivery::default(),
            state: CronJobState {
                next_run_at_ms: Some(1000),
                running_at_ms: Some(500),
                ..Default::default()
            },
            created_at_ms: 0,
            updated_at_ms: 0,
        };

        assert!(!job.is_due(2000));
    }

    #[test]
    fn error_backoff_schedule() {
        assert_eq!(error_backoff_ms(1), 30_000);
        assert_eq!(error_backoff_ms(2), 60_000);
        assert_eq!(error_backoff_ms(3), 300_000);
        assert_eq!(error_backoff_ms(4), 900_000);
        assert_eq!(error_backoff_ms(5), 3_600_000);
        // Clamps to last entry
        assert_eq!(error_backoff_ms(100), 3_600_000);
    }

    #[test]
    fn store_load_save_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cron").join("jobs.json");
        let path_str = path.to_string_lossy().to_string();

        // Create store and add a job
        let mut store = CronStore::new(path_str.clone());
        let job = CronJob {
            id: "test-1".to_string(),
            name: "Test Job".to_string(),
            description: "A test".to_string(),
            enabled: true,
            delete_after_run: false,
            schedule: CronSchedule::every(60_000, Some(0)),
            payload: CronPayload::AgentTurn {
                text: "do something".to_string(),
            },
            delivery: CronDelivery {
                channel: Some("cli".to_string()),
                chat_id: None,
            },
            state: CronJobState::default(),
            created_at_ms: 1000,
            updated_at_ms: 1000,
        };
        store.add(job);

        assert_eq!(store.list().len(), 1);
        assert_eq!(store.get("test-1").unwrap().name, "Test Job");

        // Reload from disk
        let store2 = CronStore::new(path_str);
        assert_eq!(store2.list().len(), 1);
        assert_eq!(store2.get("test-1").unwrap().name, "Test Job");
    }

    #[test]
    fn store_remove() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("jobs.json");
        let path_str = path.to_string_lossy().to_string();

        let mut store = CronStore::new(path_str);
        let job = CronJob {
            id: "rm-1".to_string(),
            name: "Remove Me".to_string(),
            description: String::new(),
            enabled: true,
            delete_after_run: false,
            schedule: CronSchedule::at(99999),
            payload: CronPayload::SystemEvent {
                text: "test".to_string(),
            },
            delivery: CronDelivery::default(),
            state: CronJobState::default(),
            created_at_ms: 0,
            updated_at_ms: 0,
        };
        store.add(job);
        assert_eq!(store.list().len(), 1);

        assert!(store.remove("rm-1"));
        assert_eq!(store.list().len(), 0);
        assert!(!store.remove("rm-1")); // already gone
    }

    #[test]
    fn store_loads_empty_on_missing_file() {
        let store = CronStore::new("/nonexistent/path/jobs.json".to_string());
        assert_eq!(store.list().len(), 0);
    }

    #[test]
    fn store_loads_empty_on_invalid_json() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("jobs.json");

        let mut f = std::fs::File::create(&path).unwrap();
        f.write_all(b"not valid json!!!").unwrap();

        let store = CronStore::new(path.to_string_lossy().to_string());
        assert_eq!(store.list().len(), 0);
    }

    #[test]
    fn schedule_serde_roundtrip() {
        let at = CronSchedule::at(12345);
        let json = serde_json::to_string(&at).unwrap();
        let back: CronSchedule = serde_json::from_str(&json).unwrap();
        assert_eq!(back.compute_next_run(10000), Some(12345));

        let every = CronSchedule::every(60_000, Some(5000));
        let json = serde_json::to_string(&every).unwrap();
        let back: CronSchedule = serde_json::from_str(&json).unwrap();
        assert_eq!(back.compute_next_run(3000), Some(5000));
    }
}
