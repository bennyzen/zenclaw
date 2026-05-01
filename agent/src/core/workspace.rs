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

/// Load workspace context files from the data directory.
///
/// 1. Flash files (AGENTS.md, TOOLS.md) — startup behavior guidance
/// 2. Soul files (SOUL.md, etc.) — agent identity
///
/// MEMORY.md is intentionally NOT loaded here. It used to be dumped verbatim
/// into the system prompt on every turn — a token-cost and attention-pollution
/// disaster. Memory is retrievable on demand via memory_search / memory_list /
/// memory_get tools.
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
    fn load_bootstrap_does_not_inject_memory_md() {
        let tmp = std::env::temp_dir().join(format!("zenclaw_ws_no_mem_{}", std::process::id()));
        let data_dir = tmp.to_str().unwrap();
        fs::create_dir_all(data_dir).unwrap();
        fs::write(format!("{}/MEMORY.md", data_dir), "huge memory dump").unwrap();
        fs::write(format!("{}/SOUL.md", data_dir), "I am ZenClaw.").unwrap();

        let files = load_bootstrap_files(data_dir);
        assert!(files.iter().all(|f| f.name != "MEMORY.md"),
            "MEMORY.md must NOT be auto-injected; it bloats the system prompt");
        // SOUL.md still comes through.
        assert!(files.iter().any(|f| f.name == "SOUL.md"));

        let _ = fs::remove_dir_all(&tmp);
    }
}
