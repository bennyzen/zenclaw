use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;
use tracing::info;

/// Maximum concurrent subagents per parent session.
const MAX_CHILDREN_PER_AGENT: usize = 5;
/// Maximum spawn depth (subagent of subagent of...).
const MAX_SPAWN_DEPTH: usize = 3;
/// Default run timeout in seconds.
const DEFAULT_RUN_TIMEOUT_S: u64 = 300;

/// Status of a subagent run.
#[derive(Debug, Clone, PartialEq)]
pub enum RunStatus {
    Running,
    Completed,
    Failed(String),
    Cancelled,
}

/// A tracked subagent run.
#[derive(Debug, Clone)]
pub struct SubagentRun {
    pub id: String,
    pub parent_chat_id: String,
    pub prompt: String,
    pub status: RunStatus,
    pub result: Option<String>,
    pub started_at_ms: u64,
    pub ended_at_ms: Option<u64>,
    pub depth: usize,
}

/// Registry tracking all active and recent subagent runs.
pub struct SubagentRegistry {
    runs: HashMap<String, SubagentRun>,
    next_id: u64,
}

impl SubagentRegistry {
    pub fn new() -> Self {
        Self {
            runs: HashMap::new(),
            next_id: 1,
        }
    }

    /// Spawn a new subagent. Returns the run ID or an error.
    pub fn register(
        &mut self,
        parent_chat_id: &str,
        prompt: &str,
        depth: usize,
    ) -> Result<String, String> {
        // Check spawn depth
        if depth >= MAX_SPAWN_DEPTH {
            return Err(format!(
                "Max spawn depth ({}) reached. Cannot spawn deeper.",
                MAX_SPAWN_DEPTH
            ));
        }

        // Check max children
        let active_count = self
            .runs
            .values()
            .filter(|r| r.parent_chat_id == parent_chat_id && r.status == RunStatus::Running)
            .count();
        if active_count >= MAX_CHILDREN_PER_AGENT {
            return Err(format!(
                "Max concurrent subagents ({}) reached for this session.",
                MAX_CHILDREN_PER_AGENT
            ));
        }

        let id = format!("sub_{}", self.next_id);
        self.next_id += 1;

        let now_ms = epoch_ms();
        let run = SubagentRun {
            id: id.clone(),
            parent_chat_id: parent_chat_id.to_string(),
            prompt: prompt.to_string(),
            status: RunStatus::Running,
            result: None,
            started_at_ms: now_ms,
            ended_at_ms: None,
            depth,
        };

        info!(id = %id, parent = %parent_chat_id, "Subagent registered");
        self.runs.insert(id.clone(), run);
        Ok(id)
    }

    /// Mark a run as completed with a result.
    pub fn complete(&mut self, id: &str, result: String) {
        if let Some(run) = self.runs.get_mut(id) {
            run.status = RunStatus::Completed;
            run.result = Some(result);
            run.ended_at_ms = Some(epoch_ms());
            info!(id = %id, "Subagent completed");
        }
    }

    /// Mark a run as failed.
    pub fn fail(&mut self, id: &str, error: String) {
        if let Some(run) = self.runs.get_mut(id) {
            run.status = RunStatus::Failed(error);
            run.ended_at_ms = Some(epoch_ms());
            info!(id = %id, "Subagent failed");
        }
    }

    /// Cancel a run.
    pub fn cancel(&mut self, id: &str) -> bool {
        if let Some(run) = self.runs.get_mut(id) {
            if run.status == RunStatus::Running {
                run.status = RunStatus::Cancelled;
                run.ended_at_ms = Some(epoch_ms());
                info!(id = %id, "Subagent cancelled");
                return true;
            }
        }
        false
    }

    /// List runs for a parent session.
    pub fn list_for_parent(&self, parent_chat_id: &str) -> Vec<&SubagentRun> {
        self.runs
            .values()
            .filter(|r| r.parent_chat_id == parent_chat_id)
            .collect()
    }

    /// Get a specific run.
    pub fn get(&self, id: &str) -> Option<&SubagentRun> {
        self.runs.get(id)
    }

    /// Count active runs for a parent session.
    pub fn active_count(&self, parent_chat_id: &str) -> usize {
        self.runs
            .values()
            .filter(|r| r.parent_chat_id == parent_chat_id && r.status == RunStatus::Running)
            .count()
    }

    /// Reap completed runs older than the given age in ms.
    pub fn reap_old(&mut self, max_age_ms: u64) {
        let cutoff = epoch_ms().saturating_sub(max_age_ms);
        self.runs.retain(|_, run| {
            if run.status == RunStatus::Running {
                return true; // keep running
            }
            match run.ended_at_ms {
                Some(ended) => ended > cutoff,
                None => true,
            }
        });
    }
}

impl Default for SubagentRegistry {
    fn default() -> Self {
        Self::new()
    }
}

fn epoch_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_register_and_complete() {
        let mut reg = SubagentRegistry::new();
        let id = reg.register("chat1", "do something", 0).unwrap();
        assert_eq!(reg.active_count("chat1"), 1);

        reg.complete(&id, "done".to_string());
        assert_eq!(reg.active_count("chat1"), 0);

        let run = reg.get(&id).unwrap();
        assert!(matches!(run.status, RunStatus::Completed));
        assert_eq!(run.result.as_deref(), Some("done"));
    }

    #[test]
    fn test_max_depth() {
        let mut reg = SubagentRegistry::new();
        let result = reg.register("chat1", "too deep", MAX_SPAWN_DEPTH);
        assert!(result.is_err());
    }

    #[test]
    fn test_max_children() {
        let mut reg = SubagentRegistry::new();
        for i in 0..MAX_CHILDREN_PER_AGENT {
            reg.register("chat1", &format!("task {}", i), 0).unwrap();
        }
        let result = reg.register("chat1", "one too many", 0);
        assert!(result.is_err());
    }

    #[test]
    fn test_cancel() {
        let mut reg = SubagentRegistry::new();
        let id = reg.register("chat1", "cancellable", 0).unwrap();
        assert!(reg.cancel(&id));
        assert_eq!(reg.active_count("chat1"), 0);
        assert!(!reg.cancel(&id)); // can't cancel twice
    }

    #[test]
    fn test_list_for_parent() {
        let mut reg = SubagentRegistry::new();
        reg.register("chat1", "task a", 0).unwrap();
        reg.register("chat1", "task b", 0).unwrap();
        reg.register("chat2", "task c", 0).unwrap();

        assert_eq!(reg.list_for_parent("chat1").len(), 2);
        assert_eq!(reg.list_for_parent("chat2").len(), 1);
    }
}
