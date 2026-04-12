use async_trait::async_trait;
use serde_json::json;

use crate::core::cron::{CronDelivery, CronPayload, CronSchedule, CronService, CronStore};
use crate::core::tools::{Tool, ToolContext, ToolResult};
use crate::core::types::ToolDefinition;

pub struct CronTool;

impl CronTool {
    fn build_store_path(data_dir: &str) -> String {
        format!("{}/cron/jobs.json", data_dir)
    }

    fn build_service(ctx: &ToolContext) -> CronService {
        let path = Self::build_store_path(&ctx.data_dir);
        let store = CronStore::new(path);
        CronService::new(store)
    }

    fn do_add(args: &serde_json::Value, ctx: &ToolContext) -> ToolResult {
        let name = args["name"].as_str().unwrap_or("Unnamed").to_string();
        let schedule_kind = args["schedule_kind"].as_str().unwrap_or("at");
        let prompt = match args["prompt"].as_str() {
            Some(p) if !p.is_empty() => p.to_string(),
            _ => return ToolResult::Error("prompt is required".to_string()),
        };

        let now_ms = epoch_ms();

        let schedule = match schedule_kind {
            "at" => {
                let at_ms = args["at_ms"].as_u64().unwrap_or_else(|| {
                    let minutes = args["at_minutes"].as_f64().unwrap_or(1.0);
                    now_ms + (minutes * 60_000.0) as u64
                });
                CronSchedule::at(at_ms)
            }
            "every" => {
                let interval_ms = args["interval_ms"].as_u64().unwrap_or_else(|| {
                    let minutes = args["every_minutes"].as_f64().unwrap_or(60.0);
                    (minutes * 60_000.0) as u64
                });
                CronSchedule::every(interval_ms, Some(now_ms))
            }
            other => {
                return ToolResult::Error(format!("Unknown schedule_kind '{}'", other));
            }
        };

        let delete_after_run = match schedule_kind {
            "every" => false,
            _ => args["delete_after_run"].as_bool().unwrap_or(true),
        };

        let channel = args["channel"].as_str().map(String::from);
        let chat_id = args["chat_id"].as_str().map(String::from);

        let payload = CronPayload::AgentTurn { text: prompt };
        let delivery = CronDelivery { channel, chat_id };

        let mut service = Self::build_service(ctx);
        let job = service.add_job(name.clone(), schedule, payload, delivery, delete_after_run);

        let next_info = match job.state.next_run_at_ms {
            Some(next) if next > now_ms => {
                let mins = (next - now_ms) as f64 / 60_000.0;
                format!(", next run in {:.1} minutes", mins)
            }
            _ => String::new(),
        };

        ToolResult::Text(format!(
            "Created job '{}' (ID: {}){}",
            name, job.id, next_info
        ))
    }

    fn do_list(ctx: &ToolContext) -> ToolResult {
        let service = Self::build_service(ctx);
        let jobs = service.list_jobs();

        if jobs.is_empty() {
            return ToolResult::Text("No scheduled jobs".to_string());
        }

        let now_ms = epoch_ms();
        let items: Vec<serde_json::Value> = jobs
            .iter()
            .map(|j| {
                let next_in = j.state.next_run_at_ms.map(|next| {
                    if next > now_ms {
                        format!("{:.1}m", (next - now_ms) as f64 / 60_000.0)
                    } else {
                        "due".to_string()
                    }
                });

                json!({
                    "id": j.id,
                    "name": j.name,
                    "enabled": j.enabled,
                    "schedule": j.schedule,
                    "next_in": next_in,
                    "last_status": j.state.last_status,
                    "consecutive_errors": j.state.consecutive_errors,
                    "running": j.state.running_at_ms.is_some(),
                })
            })
            .collect();

        ToolResult::Json(json!(items))
    }

    fn do_remove(args: &serde_json::Value, ctx: &ToolContext) -> ToolResult {
        let job_id = match args["job_id"].as_str() {
            Some(id) => id,
            None => return ToolResult::Error("job_id is required".to_string()),
        };

        let mut service = Self::build_service(ctx);
        if service.remove_job(job_id) {
            ToolResult::Text(format!("Removed job {}", job_id))
        } else {
            ToolResult::Error(format!("Job {} not found", job_id))
        }
    }

    fn do_run(args: &serde_json::Value, ctx: &ToolContext) -> ToolResult {
        let job_id = match args["job_id"].as_str() {
            Some(id) => id,
            None => return ToolResult::Error("job_id is required".to_string()),
        };

        let service = Self::build_service(ctx);
        match service.get_job(job_id) {
            Some(job) => ToolResult::Text(format!(
                "Triggered job '{}' ({}) — will execute on next background tick",
                job.name, job.id
            )),
            None => ToolResult::Error(format!("Job {} not found", job_id)),
        }
    }

    fn do_update(args: &serde_json::Value, ctx: &ToolContext) -> ToolResult {
        let job_id = match args["job_id"].as_str() {
            Some(id) => id,
            None => return ToolResult::Error("job_id is required".to_string()),
        };

        let service = Self::build_service(ctx);
        let job = match service.get_job(job_id) {
            Some(j) => j.clone(),
            None => return ToolResult::Error(format!("Job {} not found", job_id)),
        };

        let mut updated = job;
        let now_ms = epoch_ms();

        if let Some(name) = args["name"].as_str() {
            updated.name = name.to_string();
        }

        if let Some(enabled) = args["enabled"].as_bool() {
            updated.enabled = enabled;
            // Reset error count when re-enabling
            if enabled {
                updated.state.consecutive_errors = 0;
            }
        }

        if let Some(prompt) = args["prompt"].as_str() {
            updated.payload = CronPayload::AgentTurn {
                text: prompt.to_string(),
            };
        }

        // Schedule update
        if let Some(kind) = args["schedule_kind"].as_str() {
            updated.schedule = match kind {
                "at" => {
                    let at_ms = args["at_ms"].as_u64().unwrap_or_else(|| {
                        let minutes = args["at_minutes"].as_f64().unwrap_or(1.0);
                        now_ms + (minutes * 60_000.0) as u64
                    });
                    CronSchedule::at(at_ms)
                }
                "every" => {
                    let interval_ms = args["interval_ms"].as_u64().unwrap_or_else(|| {
                        let minutes = args["every_minutes"].as_f64().unwrap_or(60.0);
                        (minutes * 60_000.0) as u64
                    });
                    CronSchedule::every(interval_ms, Some(now_ms))
                }
                other => {
                    return ToolResult::Error(format!("Unknown schedule_kind '{}'", other));
                }
            };
        }

        updated.updated_at_ms = now_ms;
        updated.compute_next_run(now_ms);

        let name = updated.name.clone();
        let id = updated.id.clone();

        // Write back through the store
        let path = Self::build_store_path(&ctx.data_dir);
        let mut store = CronStore::new(path);
        if store.update(updated) {
            ToolResult::Text(format!("Updated job '{}' ({})", name, id))
        } else {
            ToolResult::Error(format!("Failed to update job {}", id))
        }
    }
}

fn epoch_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

#[async_trait]
impl Tool for CronTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "cron".to_string(),
            description: "Scheduled tasks. Actions: add, list, remove, run, update.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "action": {
                        "type": "string",
                        "enum": ["add", "list", "remove", "run", "update"],
                        "description": "Operation to perform"
                    },
                    "name": {
                        "type": "string",
                        "description": "Job name (add/update)"
                    },
                    "schedule_kind": {
                        "type": "string",
                        "enum": ["at", "every"],
                        "description": "'at' for one-shot, 'every' for recurring (add/update)"
                    },
                    "interval_ms": {
                        "type": "integer",
                        "description": "Interval in milliseconds for 'every' schedule (add/update)"
                    },
                    "at_ms": {
                        "type": "integer",
                        "description": "Epoch ms for 'at' schedule (add/update)"
                    },
                    "at_minutes": {
                        "type": "number",
                        "description": "Minutes from now for 'at' schedule (add/update)"
                    },
                    "every_minutes": {
                        "type": "number",
                        "description": "Interval in minutes for 'every' schedule (add/update)"
                    },
                    "prompt": {
                        "type": "string",
                        "description": "Prompt text for the job payload (add/update)"
                    },
                    "channel": {
                        "type": "string",
                        "description": "Delivery channel (add)"
                    },
                    "chat_id": {
                        "type": "string",
                        "description": "Delivery chat_id (add)"
                    },
                    "delete_after_run": {
                        "type": "boolean",
                        "description": "Delete one-shot jobs after run (add, default true)"
                    },
                    "enabled": {
                        "type": "boolean",
                        "description": "Enable/disable (update)"
                    },
                    "job_id": {
                        "type": "string",
                        "description": "Job ID (remove/run/update)"
                    }
                },
                "required": ["action"]
            }),
        }
    }

    async fn execute(&self, args: serde_json::Value, ctx: &ToolContext) -> ToolResult {
        let action = args["action"].as_str().unwrap_or("");
        match action {
            "add" => Self::do_add(&args, ctx),
            "list" => Self::do_list(ctx),
            "remove" => Self::do_remove(&args, ctx),
            "run" => Self::do_run(&args, ctx),
            "update" => Self::do_update(&args, ctx),
            other => ToolResult::Error(format!(
                "Unknown action '{}'. Use: add, list, remove, run, update",
                other
            )),
        }
    }
}
