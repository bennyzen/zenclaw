use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

const HISTORY_SIZE: usize = 30;
const WARNING_THRESHOLD: usize = 10;
const CRITICAL_THRESHOLD: usize = 20;
const GLOBAL_CIRCUIT_BREAKER: usize = 30;

/// Severity of a loop detection result.
#[derive(Debug, Clone, PartialEq)]
pub enum LoopLevel {
    Warning,
    Critical,
}

/// Result of a loop detection check.
#[derive(Debug, Clone)]
pub struct LoopCheckResult {
    pub level: LoopLevel,
    pub detector: &'static str,
    pub count: usize,
    pub message: String,
}

#[derive(Debug, Clone)]
struct CallRecord {
    tool_name: String,
    args_hash: u64,
    result_hash: Option<u64>,
}

/// Circuit breaker for stuck tool-use loops.
/// Tracks a sliding window of tool calls and detects:
/// - No-progress streaks (same args + same result repeated)
/// - Ping-pong patterns (alternating between two calls)
/// - Generic repeats (same tool + args called too many times)
pub struct LoopDetector {
    history: Vec<CallRecord>,
}

impl LoopDetector {
    pub fn new() -> Self {
        Self {
            history: Vec::with_capacity(HISTORY_SIZE),
        }
    }

    pub fn reset(&mut self) {
        self.history.clear();
    }

    /// Check if the upcoming tool call would trigger a loop detection.
    /// Returns None if the call is safe, or a LoopCheckResult if it should be
    /// warned about or blocked.
    pub fn check(&self, tool_name: &str, args: &serde_json::Value) -> Option<LoopCheckResult> {
        let current_hash = stable_hash(tool_name, args);
        let no_progress = self.get_no_progress_streak(tool_name, current_hash);

        // Global circuit breaker
        if no_progress >= GLOBAL_CIRCUIT_BREAKER {
            return Some(LoopCheckResult {
                level: LoopLevel::Critical,
                detector: "global_circuit_breaker",
                count: no_progress,
                message: format!(
                    "CRITICAL: {} has repeated identical no-progress outcomes {} times. \
                     Execution blocked by circuit breaker.",
                    tool_name, no_progress
                ),
            });
        }

        // Ping-pong detection
        let (ping_pong_count, ping_pong_no_progress) =
            self.get_ping_pong_streak(current_hash);

        if ping_pong_count >= CRITICAL_THRESHOLD && ping_pong_no_progress {
            return Some(LoopCheckResult {
                level: LoopLevel::Critical,
                detector: "ping_pong",
                count: ping_pong_count,
                message: format!(
                    "CRITICAL: Alternating between repeated tool-call patterns \
                     ({} consecutive calls) with no progress. Execution blocked.",
                    ping_pong_count
                ),
            });
        }

        if ping_pong_count >= WARNING_THRESHOLD {
            return Some(LoopCheckResult {
                level: LoopLevel::Warning,
                detector: "ping_pong",
                count: ping_pong_count,
                message: format!(
                    "WARNING: Alternating between repeated tool-call patterns \
                     ({} consecutive calls). Stop retrying and report the task as failed.",
                    ping_pong_count
                ),
            });
        }

        // Generic repeat detection
        let generic_count = self
            .history
            .iter()
            .filter(|e| e.tool_name == tool_name && e.args_hash == current_hash)
            .count();

        if generic_count >= CRITICAL_THRESHOLD {
            return Some(LoopCheckResult {
                level: LoopLevel::Critical,
                detector: "generic_repeat",
                count: generic_count,
                message: format!(
                    "CRITICAL: Called {} {} times with identical arguments. \
                     Execution blocked — take a different approach.",
                    tool_name, generic_count
                ),
            });
        }

        if generic_count >= WARNING_THRESHOLD {
            return Some(LoopCheckResult {
                level: LoopLevel::Warning,
                detector: "generic_repeat",
                count: generic_count,
                message: format!(
                    "WARNING: Called {} {} times with identical arguments. \
                     If not making progress, stop retrying.",
                    tool_name, generic_count
                ),
            });
        }

        None
    }

    /// Record a tool call (before execution, result_hash is None).
    pub fn record_call(&mut self, tool_name: &str, args: &serde_json::Value) {
        self.history.push(CallRecord {
            tool_name: tool_name.to_string(),
            args_hash: stable_hash(tool_name, args),
            result_hash: None,
        });
        if self.history.len() > HISTORY_SIZE {
            self.history.remove(0);
        }
    }

    /// Record the outcome of a tool call.
    pub fn record_outcome(&mut self, tool_name: &str, args: &serde_json::Value, result: &str) {
        let rh = result_hash(result);
        let ah = stable_hash(tool_name, args);

        // Find the most recent matching call without a result
        for entry in self.history.iter_mut().rev() {
            if entry.tool_name == tool_name && entry.args_hash == ah && entry.result_hash.is_none()
            {
                entry.result_hash = Some(rh);
                return;
            }
        }

        // No matching call — append
        self.history.push(CallRecord {
            tool_name: tool_name.to_string(),
            args_hash: ah,
            result_hash: Some(rh),
        });
        if self.history.len() > HISTORY_SIZE {
            self.history.remove(0);
        }
    }

    fn get_no_progress_streak(&self, tool_name: &str, args_hash: u64) -> usize {
        let mut count = 0;
        let mut last_rh = None;

        for entry in self.history.iter().rev() {
            if entry.tool_name != tool_name || entry.args_hash != args_hash {
                break;
            }
            match (entry.result_hash, last_rh) {
                (None, _) => break,
                (Some(rh), None) => {
                    last_rh = Some(rh);
                    count = 1;
                }
                (Some(rh), Some(prev)) => {
                    if rh != prev {
                        break;
                    }
                    count += 1;
                }
            }
        }
        count
    }

    fn get_ping_pong_streak(&self, current_hash: u64) -> (usize, bool) {
        if self.history.is_empty() {
            return (0, false);
        }

        let last = &self.history[self.history.len() - 1];

        // Find the paired hash
        let paired_hash = self
            .history
            .iter()
            .rev()
            .skip(1)
            .find(|e| e.args_hash != last.args_hash)
            .map(|e| e.args_hash);

        let paired_hash = match paired_hash {
            Some(h) => h,
            None => return (0, false),
        };

        if current_hash != paired_hash {
            return (0, false);
        }

        // Count alternating streak
        let mut streak = 0;
        for (i, entry) in self.history.iter().rev().enumerate() {
            let expected = if i % 2 == 0 {
                last.args_hash
            } else {
                paired_hash
            };
            if entry.args_hash != expected {
                break;
            }
            streak += 1;
        }

        if streak < 2 {
            return (0, false);
        }

        // Check no-progress across the window
        let start_idx = self.history.len().saturating_sub(streak);
        let mut result_a = None;
        let mut result_b = None;
        let mut all_no_progress = true;

        for entry in &self.history[start_idx..] {
            match entry.result_hash {
                None => {
                    all_no_progress = false;
                }
                Some(rh) => {
                    if entry.args_hash == last.args_hash {
                        match result_a {
                            None => result_a = Some(rh),
                            Some(prev) if prev != rh => all_no_progress = false,
                            _ => {}
                        }
                    } else {
                        match result_b {
                            None => result_b = Some(rh),
                            Some(prev) if prev != rh => all_no_progress = false,
                            _ => {}
                        }
                    }
                }
            }
        }

        if result_a.is_none() || result_b.is_none() {
            all_no_progress = false;
        }

        (streak + 1, all_no_progress)
    }
}

impl Default for LoopDetector {
    fn default() -> Self {
        Self::new()
    }
}

fn stable_hash(tool_name: &str, params: &serde_json::Value) -> u64 {
    let serialized = serde_json::to_string(params).unwrap_or_default();
    let mut hasher = DefaultHasher::new();
    format!("{}:{}", tool_name, serialized).hash(&mut hasher);
    hasher.finish()
}

fn result_hash(result: &str) -> u64 {
    let truncated = &result[..result.len().min(500)];
    let mut hasher = DefaultHasher::new();
    truncated.hash(&mut hasher);
    hasher.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_no_detection_on_few_calls() {
        let mut det = LoopDetector::new();
        let args = serde_json::json!({"action": "read"});

        det.record_call("file", &args);
        det.record_outcome("file", &args, "content");

        assert!(det.check("file", &args).is_none());
    }

    #[test]
    fn test_warning_on_repeat() {
        let mut det = LoopDetector::new();
        let args = serde_json::json!({"action": "read"});

        for _ in 0..WARNING_THRESHOLD {
            det.record_call("file", &args);
            det.record_outcome("file", &args, "same result");
        }

        let result = det.check("file", &args);
        assert!(result.is_some());
        assert_eq!(result.unwrap().level, LoopLevel::Warning);
    }

    #[test]
    fn test_critical_on_many_repeats() {
        let mut det = LoopDetector::new();
        let args = serde_json::json!({"action": "read"});

        for _ in 0..CRITICAL_THRESHOLD {
            det.record_call("file", &args);
            det.record_outcome("file", &args, "same result");
        }

        let result = det.check("file", &args);
        assert!(result.is_some());
        assert_eq!(result.unwrap().level, LoopLevel::Critical);
    }

    #[test]
    fn test_different_args_no_detection() {
        let mut det = LoopDetector::new();

        for i in 0..25 {
            let args = serde_json::json!({"action": "read", "path": format!("file_{}", i)});
            det.record_call("file", &args);
            det.record_outcome("file", &args, "content");
        }

        let args = serde_json::json!({"action": "read", "path": "file_new"});
        assert!(det.check("file", &args).is_none());
    }

    #[test]
    fn test_reset_clears_history() {
        let mut det = LoopDetector::new();
        let args = serde_json::json!({"action": "read"});

        for _ in 0..WARNING_THRESHOLD {
            det.record_call("file", &args);
            det.record_outcome("file", &args, "same result");
        }

        det.reset();
        assert!(det.check("file", &args).is_none());
    }
}
