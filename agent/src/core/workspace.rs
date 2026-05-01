use std::fs;
use std::path::Path;

/// A loaded context file (SOUL.md, AGENTS.md, memory, etc.)
#[derive(Debug, Clone)]
pub struct ContextFile {
    pub name: String,
    pub path: String,
    pub content: String,
}

/// Files loaded directly from the data directory root (always attempted).
const FLASH_FILENAMES: &[&str] = &["AGENTS.md", "TOOLS.md"];

/// Identity files — agent persona / bootstrap. First match wins.
const SOUL_FILENAMES: &[&str] = &[
    "SOUL.md",
    "IDENTITY.md",
    "USER.md",
    "HEARTBEAT.md",
    "BOOTSTRAP.md",
];

/// Optional files — included only if present and non-empty.
const OPTIONAL_FILENAMES: &[&str] = &["MEMORY.md"];

/// Maximum number of recent memory files to include in context.
const MAX_MEMORIES: usize = 3;

/// Load all workspace context files from the data directory.
///
/// 1. Flash files (AGENTS.md, TOOLS.md) — startup behavior guidance
/// 2. Soul files (SOUL.md, etc.) — agent identity
/// 3. Optional files (MEMORY.md) — only if present and non-empty
/// 4. Recent memory files from `{data_dir}/memory/` (newest first, up to 3)
pub fn load_bootstrap_files(data_dir: &str) -> Vec<ContextFile> {
    let mut results = Vec::new();

    for filename in FLASH_FILENAMES {
        if let Some(cf) = resolve_file(data_dir, filename) {
            results.push(cf);
        }
    }

    for filename in SOUL_FILENAMES {
        if let Some(cf) = resolve_file(data_dir, filename) {
            results.push(cf);
        }
    }

    // Optional files — only if non-empty
    for filename in OPTIONAL_FILENAMES {
        if let Some(cf) = resolve_file(data_dir, filename) {
            if !cf.content.is_empty() {
                results.push(cf);
            }
        }
    }

    // Recent memory files — newest first (filenames starting with digits)
    let memory_dir = format!("{}/memory", data_dir);
    if let Ok(mut entries) = fs::read_dir(&memory_dir) {
        let mut memory_files: Vec<String> = entries
            .by_ref()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().to_string())
            .filter(|name| name.ends_with(".md") && name.starts_with(|c: char| c.is_ascii_digit()))
            .collect();

        memory_files.sort_by(|a, b| b.cmp(a)); // newest first
        memory_files.truncate(MAX_MEMORIES);

        for mem_filename in memory_files {
            let mem_path = format!("{}/{}", memory_dir, mem_filename);
            if let Some(content) = read_file_trimmed(&mem_path) {
                if !content.is_empty() {
                    results.push(ContextFile {
                        name: format!("memory/{}", mem_filename),
                        path: mem_path,
                        content,
                    });
                }
            }
        }
    }

    results
}

fn resolve_file(data_dir: &str, filename: &str) -> Option<ContextFile> {
    let path = format!("{}/{}", data_dir, filename);
    read_file_trimmed(&path).map(|content| ContextFile {
        name: filename.to_string(),
        path,
        content,
    })
}

fn read_file_trimmed(path: &str) -> Option<String> {
    fs::read_to_string(path).ok().map(|s| s.trim().to_string())
}

/// Default SOUL.md content shipped with the firmware. Written to the data
/// directory on first boot if no SOUL/IDENTITY/USER file already exists.
const DEFAULT_SOUL_MD: &str = "You are ZenClaw, an AI agent running on an ESP32 embedded device.\nYou are helpful, concise, and resourceful.\n";

/// Default AGENTS.md content. Written to the data directory on first boot
/// if no AGENTS.md is already present.
const DEFAULT_AGENTS_MD: &str = "# Startup Checklist\n- Greet the user\n- Be ready to help\n";

/// Seed default bootstrap files into the data directory if missing.
/// Idempotent — each file is only written when no user-supplied version
/// already exists, so content edited via the file manager is preserved.
pub fn seed_defaults(data_dir: &str) {
    let any_soul = SOUL_FILENAMES
        .iter()
        .any(|f| Path::new(&format!("{}/{}", data_dir, f)).exists());
    if !any_soul {
        let path = format!("{}/SOUL.md", data_dir);
        let _ = fs::write(&path, DEFAULT_SOUL_MD);
    }

    let agents_path = format!("{}/AGENTS.md", data_dir);
    if !Path::new(&agents_path).exists() {
        let _ = fs::write(&agents_path, DEFAULT_AGENTS_MD);
    }
}

/// Resolve a user-supplied path against the data directory.
/// Absolute paths are returned as-is; relative paths are prefixed with data_dir.
pub fn resolve_path(data_dir: &str, path: &str) -> String {
    if path.starts_with('/') {
        path.to_string()
    } else {
        format!("{}/{}", data_dir, path)
    }
}

/// Ensure the parent directory of `path` exists.
pub fn ensure_parent_dir(path: &str) {
    if let Some(parent) = Path::new(path).parent() {
        let _ = fs::create_dir_all(parent);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn resolve_path_absolute() {
        assert_eq!(resolve_path("data", "/tmp/foo.txt"), "/tmp/foo.txt");
    }

    #[test]
    fn resolve_path_relative() {
        assert_eq!(resolve_path("/sd/data", "sessions/abc.jsonl"), "/sd/data/sessions/abc.jsonl");
    }

    #[test]
    fn load_bootstrap_finds_soul_md() {
        let tmp = std::env::temp_dir().join(format!("zenclaw_ws_test_{}", std::process::id()));
        let data_dir = tmp.to_str().unwrap();
        fs::create_dir_all(data_dir).unwrap();
        fs::write(format!("{}/SOUL.md", data_dir), "I am ZenClaw.").unwrap();

        let files = load_bootstrap_files(data_dir);
        let soul = files.iter().find(|f| f.name == "SOUL.md");
        assert!(soul.is_some());
        assert_eq!(soul.unwrap().content, "I am ZenClaw.");

        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn load_bootstrap_skips_missing_files() {
        let tmp = std::env::temp_dir().join(format!("zenclaw_ws_empty_{}", std::process::id()));
        let data_dir = tmp.to_str().unwrap();
        fs::create_dir_all(data_dir).unwrap();

        let files = load_bootstrap_files(data_dir);
        // No soul/flash files exist — should be empty (no panics)
        assert!(files.iter().all(|f| !f.content.is_empty()));

        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn seed_defaults_writes_missing_files() {
        let tmp = std::env::temp_dir().join(format!("zenclaw_ws_seed_{}", std::process::id()));
        let data_dir = tmp.to_str().unwrap();
        fs::create_dir_all(data_dir).unwrap();

        seed_defaults(data_dir);

        let soul = fs::read_to_string(format!("{}/SOUL.md", data_dir)).unwrap();
        assert!(soul.contains("ZenClaw"));
        let agents = fs::read_to_string(format!("{}/AGENTS.md", data_dir)).unwrap();
        assert!(agents.contains("Startup Checklist"));

        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn seed_defaults_preserves_existing_content() {
        let tmp = std::env::temp_dir().join(format!("zenclaw_ws_seed_existing_{}", std::process::id()));
        let data_dir = tmp.to_str().unwrap();
        fs::create_dir_all(data_dir).unwrap();
        fs::write(format!("{}/SOUL.md", data_dir), "user soul").unwrap();
        fs::write(format!("{}/AGENTS.md", data_dir), "user agents").unwrap();

        seed_defaults(data_dir);

        assert_eq!(fs::read_to_string(format!("{}/SOUL.md", data_dir)).unwrap(), "user soul");
        assert_eq!(fs::read_to_string(format!("{}/AGENTS.md", data_dir)).unwrap(), "user agents");

        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn load_bootstrap_loads_memory_files() {
        let tmp = std::env::temp_dir().join(format!("zenclaw_ws_mem_{}", std::process::id()));
        let mem_dir = format!("{}/memory", tmp.display());
        fs::create_dir_all(&mem_dir).unwrap();

        fs::write(format!("{}/001_first.md", mem_dir), "memory one").unwrap();
        fs::write(format!("{}/002_second.md", mem_dir), "memory two").unwrap();
        fs::write(format!("{}/not_a_memory.md", mem_dir), "ignored").unwrap();

        let files = load_bootstrap_files(tmp.to_str().unwrap());
        let mem_files: Vec<_> = files.iter().filter(|f| f.name.starts_with("memory/")).collect();
        assert_eq!(mem_files.len(), 2);
        // Newest first
        assert!(mem_files[0].name.contains("002"));

        let _ = fs::remove_dir_all(&tmp);
    }
}
