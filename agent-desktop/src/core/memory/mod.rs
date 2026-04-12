use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;

/// A single memory chunk with its embedding.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryEntry {
    pub id: String,
    pub text: String,
    #[serde(default)]
    pub embedding: Vec<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<serde_json::Value>,
    /// Source file this chunk came from.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
}

/// Search result from memory.
#[derive(Debug, Clone)]
pub struct MemorySearchResult {
    pub entry: MemoryEntry,
    pub score: f32,
}

/// Trait for vector-based memory stores.
#[async_trait]
pub trait VectorStore: Send + Sync {
    async fn add(&mut self, entry: MemoryEntry) -> Result<(), MemoryError>;
    async fn search(
        &self,
        query_embedding: &[f32],
        top_k: usize,
    ) -> Result<Vec<MemorySearchResult>, MemoryError>;
    async fn get(&self, id: &str) -> Result<Option<MemoryEntry>, MemoryError>;
    async fn reindex(&mut self) -> Result<(), MemoryError>;
    fn len(&self) -> usize;
    fn is_empty(&self) -> bool {
        self.len() == 0
    }
    fn save_to_disk(&self, path: &str) -> Result<(), MemoryError>;
    fn load_from_disk(&mut self, path: &str) -> Result<(), MemoryError>;
}

#[derive(Debug, thiserror::Error)]
pub enum MemoryError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("Memory error: {0}")]
    Other(String),
}

// --- Brute-force vector store ---

/// Brute-force in-memory vector store. Cosine similarity scan over all entries.
/// Suitable for <1000 chunks (instant even on ESP32).
pub struct BruteForceStore {
    entries: Vec<MemoryEntry>,
    /// Dedup: content hash -> entry id
    seen_hashes: HashMap<u64, String>,
}

impl BruteForceStore {
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
            seen_hashes: HashMap::new(),
        }
    }
}

impl Default for BruteForceStore {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl VectorStore for BruteForceStore {
    async fn add(&mut self, entry: MemoryEntry) -> Result<(), MemoryError> {
        let hash = djb2_hash(&entry.text);
        if self.seen_hashes.contains_key(&hash) {
            return Ok(()); // duplicate — skip
        }
        self.seen_hashes.insert(hash, entry.id.clone());
        self.entries.push(entry);
        Ok(())
    }

    async fn search(
        &self,
        query_embedding: &[f32],
        top_k: usize,
    ) -> Result<Vec<MemorySearchResult>, MemoryError> {
        if query_embedding.is_empty() || self.entries.is_empty() {
            return Ok(Vec::new());
        }

        let mut scored: Vec<MemorySearchResult> = self
            .entries
            .iter()
            .filter(|e| !e.embedding.is_empty())
            .map(|e| MemorySearchResult {
                score: cosine_similarity(query_embedding, &e.embedding),
                entry: e.clone(),
            })
            .collect();

        scored.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
        scored.truncate(top_k);
        Ok(scored)
    }

    async fn get(&self, id: &str) -> Result<Option<MemoryEntry>, MemoryError> {
        Ok(self.entries.iter().find(|e| e.id == id).cloned())
    }

    async fn reindex(&mut self) -> Result<(), MemoryError> {
        // Rebuild dedup index
        self.seen_hashes.clear();
        for entry in &self.entries {
            self.seen_hashes
                .insert(djb2_hash(&entry.text), entry.id.clone());
        }
        Ok(())
    }

    fn len(&self) -> usize {
        self.entries.len()
    }

    fn save_to_disk(&self, path: &str) -> Result<(), MemoryError> {
        let index = IndexFile {
            chunks: self
                .entries
                .iter()
                .map(|e| IndexChunk {
                    id: e.id.clone(),
                    text: e.text.clone(),
                    embedding: e.embedding.clone(),
                    source: e.source.clone(),
                })
                .collect(),
        };
        let json = serde_json::to_string(&index)?;
        fs::write(path, json)?;
        Ok(())
    }

    fn load_from_disk(&mut self, path: &str) -> Result<(), MemoryError> {
        let data = match fs::read_to_string(path) {
            Ok(d) => d,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(e) => return Err(MemoryError::Io(e)),
        };
        let index: IndexFile = serde_json::from_str(&data)?;
        self.entries.clear();
        self.seen_hashes.clear();
        for chunk in index.chunks {
            let entry = MemoryEntry {
                id: chunk.id,
                text: chunk.text,
                embedding: chunk.embedding,
                metadata: None,
                source: chunk.source,
            };
            let hash = djb2_hash(&entry.text);
            self.seen_hashes.insert(hash, entry.id.clone());
            self.entries.push(entry);
        }
        Ok(())
    }
}

/// On-disk index format — compatible with MicroPython's index.json
#[derive(Serialize, Deserialize)]
struct IndexFile {
    chunks: Vec<IndexChunk>,
}

#[derive(Serialize, Deserialize)]
struct IndexChunk {
    id: String,
    text: String,
    #[serde(default)]
    embedding: Vec<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    source: Option<String>,
}

// --- High-level MemoryStore ---

/// High-level memory store that manages markdown files + vector index.
/// Wraps a VectorStore and handles file I/O for MEMORY.md and daily files.
pub struct MemoryStore {
    data_dir: String,
    store: BruteForceStore,
}

/// Max MEMORY.md size in bytes.
const MAX_MEMORY_SIZE: usize = 32 * 1024;
/// Max daily memory files to keep.
const MAX_DAILY_FILES: usize = 30;
/// Default chunk size in chars.
const CHUNK_SIZE: usize = 400;
/// Overlap between chunks in chars.
const CHUNK_OVERLAP: usize = 80;

impl MemoryStore {
    pub fn new(data_dir: &str) -> Self {
        let mut store = BruteForceStore::new();
        let index_path = format!("{}/memory/index.json", data_dir);
        let _ = store.load_from_disk(&index_path);

        Self {
            data_dir: data_dir.to_string(),
            store,
        }
    }

    pub fn data_dir(&self) -> &str {
        &self.data_dir
    }

    /// Save a memory entry: append to MEMORY.md and add to vector store.
    pub async fn save(
        &mut self,
        text: &str,
        embedding: Option<Vec<f32>>,
    ) -> Result<String, MemoryError> {
        let id = format!(
            "mem_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis()
        );

        // Append to MEMORY.md
        let memory_path = format!("{}/MEMORY.md", self.data_dir);
        let current = fs::read_to_string(&memory_path).unwrap_or_default();
        if current.len() + text.len() + 2 <= MAX_MEMORY_SIZE {
            let new_content = if current.is_empty() {
                text.to_string()
            } else {
                format!("{}\n\n{}", current.trim(), text)
            };
            fs::write(&memory_path, new_content)?;
        }

        // Add to vector store
        let entry = MemoryEntry {
            id: id.clone(),
            text: text.to_string(),
            embedding: embedding.unwrap_or_default(),
            metadata: None,
            source: Some("MEMORY.md".to_string()),
        };
        self.store.add(entry).await?;

        // Persist index
        self.persist_index()?;

        Ok(id)
    }

    /// Search memory by embedding vector.
    pub async fn search(
        &self,
        query_embedding: &[f32],
        top_k: usize,
    ) -> Result<Vec<MemorySearchResult>, MemoryError> {
        self.store.search(query_embedding, top_k).await
    }

    /// Keyword search (no embeddings needed).
    pub fn search_keyword(&self, query: &str, top_k: usize) -> Vec<MemorySearchResult> {
        let query_lower = query.to_lowercase();
        let query_terms: Vec<&str> = query_lower.split_whitespace().collect();

        let mut results: Vec<MemorySearchResult> = self
            .store
            .entries
            .iter()
            .filter_map(|entry| {
                let text_lower = entry.text.to_lowercase();
                let matched = query_terms
                    .iter()
                    .filter(|t| text_lower.contains(*t))
                    .count();
                if matched > 0 {
                    Some(MemorySearchResult {
                        entry: entry.clone(),
                        score: matched as f32 / query_terms.len() as f32,
                    })
                } else {
                    None
                }
            })
            .collect();

        results.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
        results.truncate(top_k);
        results
    }

    /// Get a specific entry by ID.
    pub async fn get(&self, id: &str) -> Result<Option<MemoryEntry>, MemoryError> {
        self.store.get(id).await
    }

    /// Reindex: re-chunk all memory files and rebuild the vector store.
    pub async fn reindex(&mut self) -> Result<usize, MemoryError> {
        self.store.entries.clear();
        self.store.seen_hashes.clear();

        // Read MEMORY.md
        let memory_path = format!("{}/MEMORY.md", self.data_dir);
        if let Ok(content) = fs::read_to_string(&memory_path) {
            let chunks = chunk_text(&content, CHUNK_SIZE, CHUNK_OVERLAP);
            for (i, chunk) in chunks.into_iter().enumerate() {
                let entry = MemoryEntry {
                    id: format!("memory_md_{}", i),
                    text: chunk,
                    embedding: Vec::new(), // needs embedding call
                    metadata: None,
                    source: Some("MEMORY.md".to_string()),
                };
                self.store.add(entry).await?;
            }
        }

        // Read daily memory files
        let memory_dir = format!("{}/memory", self.data_dir);
        if let Ok(entries) = fs::read_dir(&memory_dir) {
            let mut files: Vec<String> = entries
                .filter_map(|e| e.ok())
                .map(|e| e.file_name().to_string_lossy().to_string())
                .filter(|name| name.ends_with(".md") && name.starts_with(|c: char| c.is_ascii_digit()))
                .collect();
            files.sort_by(|a, b| b.cmp(a));
            files.truncate(MAX_DAILY_FILES);

            for filename in &files {
                let path = format!("{}/{}", memory_dir, filename);
                if let Ok(content) = fs::read_to_string(&path) {
                    let chunks = chunk_text(&content, CHUNK_SIZE, CHUNK_OVERLAP);
                    for (i, chunk) in chunks.into_iter().enumerate() {
                        let entry = MemoryEntry {
                            id: format!("{}_{}", filename, i),
                            text: chunk,
                            embedding: Vec::new(),
                            metadata: None,
                            source: Some(format!("memory/{}", filename)),
                        };
                        self.store.add(entry).await?;
                    }
                }
            }
        }

        let count = self.store.len();
        self.persist_index()?;
        Ok(count)
    }

    pub fn chunk_count(&self) -> usize {
        self.store.len()
    }

    fn persist_index(&self) -> Result<(), MemoryError> {
        let dir = format!("{}/memory", self.data_dir);
        let _ = fs::create_dir_all(&dir);
        let path = format!("{}/index.json", dir);
        self.store.save_to_disk(&path)
    }
}

// --- Utility functions ---

/// DJB2 hash for deduplication (compatible with MicroPython implementation).
fn djb2_hash(s: &str) -> u64 {
    let mut hash: u64 = 5381;
    for b in s.bytes() {
        hash = hash.wrapping_mul(33).wrapping_add(b as u64);
    }
    hash
}

/// Cosine similarity between two vectors.
fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }
    let mut dot = 0.0f32;
    let mut norm_a = 0.0f32;
    let mut norm_b = 0.0f32;
    for (x, y) in a.iter().zip(b.iter()) {
        dot += x * y;
        norm_a += x * x;
        norm_b += y * y;
    }
    let denom = norm_a.sqrt() * norm_b.sqrt();
    if denom < f32::EPSILON {
        0.0
    } else {
        dot / denom
    }
}

/// Split text into overlapping chunks.
pub fn chunk_text(text: &str, chunk_size: usize, overlap: usize) -> Vec<String> {
    if text.len() <= chunk_size {
        return vec![text.to_string()];
    }

    let mut chunks = Vec::new();
    let chars: Vec<char> = text.chars().collect();
    let step = chunk_size.saturating_sub(overlap).max(1);
    let mut start = 0;

    while start < chars.len() {
        let end = (start + chunk_size).min(chars.len());
        let chunk: String = chars[start..end].iter().collect();
        chunks.push(chunk.trim().to_string());
        if end >= chars.len() {
            break;
        }
        start += step;
    }

    chunks
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cosine_similarity_identical() {
        let v = vec![1.0, 0.0, 0.0];
        assert!((cosine_similarity(&v, &v) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn test_cosine_similarity_orthogonal() {
        let a = vec![1.0, 0.0];
        let b = vec![0.0, 1.0];
        assert!(cosine_similarity(&a, &b).abs() < 1e-6);
    }

    #[test]
    fn test_cosine_similarity_opposite() {
        let a = vec![1.0, 0.0];
        let b = vec![-1.0, 0.0];
        assert!((cosine_similarity(&a, &b) + 1.0).abs() < 1e-6);
    }

    #[test]
    fn test_chunk_text_small() {
        let chunks = chunk_text("hello world", 100, 20);
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0], "hello world");
    }

    #[test]
    fn test_chunk_text_overlap() {
        let text = "a".repeat(1000);
        let chunks = chunk_text(&text, 400, 80);
        assert!(chunks.len() > 1);
        // Each chunk should be <= 400 chars
        for chunk in &chunks {
            assert!(chunk.len() <= 400);
        }
    }

    #[test]
    fn test_djb2_deterministic() {
        assert_eq!(djb2_hash("hello"), djb2_hash("hello"));
        assert_ne!(djb2_hash("hello"), djb2_hash("world"));
    }

    #[tokio::test]
    async fn test_brute_force_add_search() {
        let mut store = BruteForceStore::new();
        store
            .add(MemoryEntry {
                id: "1".to_string(),
                text: "hello world".to_string(),
                embedding: vec![1.0, 0.0, 0.0],
                metadata: None,
                source: None,
            })
            .await
            .unwrap();
        store
            .add(MemoryEntry {
                id: "2".to_string(),
                text: "goodbye world".to_string(),
                embedding: vec![0.0, 1.0, 0.0],
                metadata: None,
                source: None,
            })
            .await
            .unwrap();

        let results = store.search(&[1.0, 0.0, 0.0], 1).await.unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].entry.id, "1");
        assert!((results[0].score - 1.0).abs() < 1e-6);
    }

    #[tokio::test]
    async fn test_brute_force_dedup() {
        let mut store = BruteForceStore::new();
        store
            .add(MemoryEntry {
                id: "1".to_string(),
                text: "same content".to_string(),
                embedding: vec![1.0],
                metadata: None,
                source: None,
            })
            .await
            .unwrap();
        store
            .add(MemoryEntry {
                id: "2".to_string(),
                text: "same content".to_string(), // duplicate
                embedding: vec![1.0],
                metadata: None,
                source: None,
            })
            .await
            .unwrap();

        assert_eq!(store.len(), 1); // deduped
    }

    #[tokio::test]
    async fn test_index_persistence() {
        let tmp = std::env::temp_dir().join(format!("zenclaw_mem_test_{}", std::process::id()));
        let path = format!("{}/index.json", tmp.display());
        fs::create_dir_all(&tmp).unwrap();

        let mut store = BruteForceStore::new();
        store
            .add(MemoryEntry {
                id: "1".to_string(),
                text: "persisted entry".to_string(),
                embedding: vec![0.5, 0.5],
                metadata: None,
                source: Some("test.md".to_string()),
            })
            .await
            .unwrap();
        store.save_to_disk(&path).unwrap();

        let mut store2 = BruteForceStore::new();
        store2.load_from_disk(&path).unwrap();
        assert_eq!(store2.len(), 1);
        let entry = store2.get("1").await.unwrap().unwrap();
        assert_eq!(entry.text, "persisted entry");

        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_keyword_search() {
        let store = MemoryStore {
            data_dir: "/nonexistent".to_string(),
            store: {
                let mut s = BruteForceStore::new();
                s.entries.push(MemoryEntry {
                    id: "1".to_string(),
                    text: "The cat sat on the mat".to_string(),
                    embedding: Vec::new(),
                    metadata: None,
                    source: None,
                });
                s.entries.push(MemoryEntry {
                    id: "2".to_string(),
                    text: "The dog ran in the park".to_string(),
                    embedding: Vec::new(),
                    metadata: None,
                    source: None,
                });
                s
            },
        };

        let results = store.search_keyword("cat mat", 5);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].entry.id, "1");
    }
}
