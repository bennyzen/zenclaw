//! Flash-backed snapshot of the [`CloudCache`].
//!
//! Boot-time fallback when S3 is unreachable: a tail-only restore
//! against the network is preferred (see `cloud::boot`), but if even
//! that fails the agent should still come up with *some* prior state.
//! The snapshot is that floor.
//!
//! Written periodically by a timer thread (lifecycle in `main.rs`,
//! T9) and re-read on boot. Atomic via tmp + rename: a partial write
//! never replaces a previous good snapshot.
//!
//! Format: tiny length-prefixed binary, NOT JSON.  serde_json encodes
//! `Vec<u8>` as a JSON array of integers (4-5x bloat), which on an
//! 8 MB on-flash partition is unacceptable when the cache itself is
//! 1-2 MB. The format below is ~12 bytes overhead per entry plus the
//! raw key + value bytes.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::Path;

use chrono::Utc;

use crate::core::cloud::cache::CloudCache;

/// Default on-flash location. Caller can override for tests.
pub const SNAPSHOT_PATH: &str = "data/.snapshot.bin";

const MAGIC: u32 = 0x5A434C44; // "ZCLD"
const VERSION: u32 = 1;

/// Decoded form of the on-flash file.
#[derive(Debug)]
pub struct Snapshot {
    pub written_at: i64,
    pub entries: HashMap<String, Vec<u8>>,
}

/// Write the cache to `path` atomically. Caller is responsible for
/// ensuring the parent directory exists.
pub fn write_to(cache: &CloudCache, path: &str) -> std::io::Result<()> {
    let entries = cache.snapshot();
    let bytes = encode(Utc::now().timestamp(), &entries);
    let tmp = format!("{}.tmp", path);
    {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(&bytes)?;
        f.sync_all()?;
    }
    std::fs::rename(&tmp, path)?;
    Ok(())
}

/// Default-path convenience wrapper used by the timer thread.
pub fn write_snapshot(cache: &CloudCache) -> std::io::Result<()> {
    write_to(cache, SNAPSHOT_PATH)
}

/// Read the snapshot at `path`. Returns `Ok(None)` when the file
/// doesn't exist (fresh device); errors only on IO failure or magic
/// mismatch (treated as corruption).
pub fn read_from(path: &str) -> std::io::Result<Option<Snapshot>> {
    if !Path::new(path).exists() {
        return Ok(None);
    }
    let mut f = std::fs::File::open(path)?;
    let mut bytes = Vec::new();
    f.read_to_end(&mut bytes)?;
    decode(&bytes)
        .map(Some)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
}

pub fn read_snapshot() -> std::io::Result<Option<Snapshot>> {
    read_from(SNAPSHOT_PATH)
}

fn encode(written_at: i64, entries: &HashMap<String, Vec<u8>>) -> Vec<u8> {
    let mut out = Vec::with_capacity(
        16 + entries
            .iter()
            .map(|(k, v)| 8 + k.len() + v.len())
            .sum::<usize>(),
    );
    out.extend_from_slice(&MAGIC.to_le_bytes());
    out.extend_from_slice(&VERSION.to_le_bytes());
    out.extend_from_slice(&written_at.to_le_bytes());
    out.extend_from_slice(&(entries.len() as u32).to_le_bytes());
    // Sort by key for deterministic output (helps tests + dedup tooling).
    let mut sorted: Vec<(&String, &Vec<u8>)> = entries.iter().collect();
    sorted.sort_by(|a, b| a.0.cmp(b.0));
    for (k, v) in sorted {
        out.extend_from_slice(&(k.len() as u32).to_le_bytes());
        out.extend_from_slice(k.as_bytes());
        out.extend_from_slice(&(v.len() as u32).to_le_bytes());
        out.extend_from_slice(v);
    }
    out
}

fn decode(bytes: &[u8]) -> Result<Snapshot, String> {
    if bytes.len() < 20 {
        return Err(format!("snapshot too short: {} bytes", bytes.len()));
    }
    let mut p = 0;
    let magic = read_u32(bytes, &mut p)?;
    if magic != MAGIC {
        return Err(format!("bad magic: 0x{:08x}", magic));
    }
    let version = read_u32(bytes, &mut p)?;
    if version != VERSION {
        return Err(format!("unsupported version: {}", version));
    }
    let written_at = read_i64(bytes, &mut p)?;
    let count = read_u32(bytes, &mut p)? as usize;

    let mut entries = HashMap::with_capacity(count);
    for _ in 0..count {
        let klen = read_u32(bytes, &mut p)? as usize;
        let key = std::str::from_utf8(read_slice(bytes, &mut p, klen)?)
            .map_err(|e| format!("non-utf8 key: {}", e))?
            .to_string();
        let vlen = read_u32(bytes, &mut p)? as usize;
        let val = read_slice(bytes, &mut p, vlen)?.to_vec();
        entries.insert(key, val);
    }
    Ok(Snapshot {
        written_at,
        entries,
    })
}

fn read_u32(bytes: &[u8], p: &mut usize) -> Result<u32, String> {
    let s = read_slice(bytes, p, 4)?;
    Ok(u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
}
fn read_i64(bytes: &[u8], p: &mut usize) -> Result<i64, String> {
    let s = read_slice(bytes, p, 8)?;
    let mut buf = [0u8; 8];
    buf.copy_from_slice(s);
    Ok(i64::from_le_bytes(buf))
}
fn read_slice<'a>(bytes: &'a [u8], p: &mut usize, n: usize) -> Result<&'a [u8], String> {
    if *p + n > bytes.len() {
        return Err(format!(
            "truncated at offset {} (need {} of {} remaining)",
            p,
            n,
            bytes.len() - *p
        ));
    }
    let out = &bytes[*p..*p + n];
    *p += n;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn tmp_path(suffix: &str) -> String {
        let id = uuid::Uuid::new_v4().to_string();
        let dir = format!("/tmp/zenclaw_snap_{}", id);
        fs::create_dir_all(&dir).unwrap();
        format!("{}/{}", dir, suffix)
    }

    #[test]
    fn round_trip_preserves_all_entries() {
        let cache = CloudCache::new();
        cache.put("sys/MEMORY.md", b"## [m1] note\n".to_vec());
        cache.put("sys/sessions/web/log-00.jsonl", b"line1\nline2\n".to_vec());
        cache.put("sys/cron.json", b"{\"jobs\":[]}".to_vec());

        let path = tmp_path("snap.bin");
        write_to(&cache, &path).unwrap();
        let snap = read_from(&path).unwrap().unwrap();

        assert_eq!(snap.entries.len(), 3);
        assert_eq!(snap.entries["sys/MEMORY.md"], b"## [m1] note\n".to_vec());
        assert_eq!(
            snap.entries["sys/sessions/web/log-00.jsonl"],
            b"line1\nline2\n".to_vec()
        );
        assert!(snap.written_at > 0);

        let _ = fs::remove_dir_all(std::path::Path::new(&path).parent().unwrap());
    }

    #[test]
    fn read_returns_none_for_missing_file() {
        let path = tmp_path("nonexistent.bin");
        // Don't actually create the file.
        let snap = read_from(&path).unwrap();
        assert!(snap.is_none());
        let _ = fs::remove_dir_all(std::path::Path::new(&path).parent().unwrap());
    }

    #[test]
    fn read_rejects_bad_magic() {
        let path = tmp_path("garbage.bin");
        fs::write(&path, b"not a valid snapshot at all whatsoever").unwrap();
        let err = read_from(&path).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
        let _ = fs::remove_dir_all(std::path::Path::new(&path).parent().unwrap());
    }

    #[test]
    fn write_uses_atomic_rename_via_tmp() {
        // We can't easily verify atomicity without a deliberately
        // failing FS, but we can verify the .tmp file is gone after a
        // successful write — proves the rename step ran.
        let path = tmp_path("snap.bin");
        let cache = CloudCache::new();
        cache.put("k", b"v".to_vec());
        write_to(&cache, &path).unwrap();
        assert!(!std::path::Path::new(&format!("{}.tmp", path)).exists());
        assert!(std::path::Path::new(&path).exists());
        let _ = fs::remove_dir_all(std::path::Path::new(&path).parent().unwrap());
    }

    #[test]
    fn restore_from_snapshot_repopulates_cache() {
        // End-to-end: write cache → snapshot → fresh cache → restore.
        let original = CloudCache::new();
        original.put("a", b"alpha".to_vec());
        original.put("b", b"beta".to_vec());

        let path = tmp_path("snap.bin");
        write_to(&original, &path).unwrap();

        let restored_snap = read_from(&path).unwrap().unwrap();
        let fresh = CloudCache::new();
        fresh.restore_from(restored_snap.entries);

        assert_eq!(fresh.get("a"), Some(b"alpha".to_vec()));
        assert_eq!(fresh.get("b"), Some(b"beta".to_vec()));
        let _ = fs::remove_dir_all(std::path::Path::new(&path).parent().unwrap());
    }

    #[test]
    fn empty_cache_serializes_to_header_only() {
        let cache = CloudCache::new();
        let bytes = encode(42, &cache.snapshot());
        // 4 (magic) + 4 (version) + 8 (ts) + 4 (count=0) = 20
        assert_eq!(bytes.len(), 20);
        let snap = decode(&bytes).unwrap();
        assert_eq!(snap.written_at, 42);
        assert!(snap.entries.is_empty());
    }
}
