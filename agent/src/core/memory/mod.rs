use async_trait::async_trait;

/// A single memory entry with its embedding.
#[derive(Debug, Clone)]
pub struct MemoryEntry {
    pub id: String,
    pub text: String,
    pub embedding: Vec<f32>,
    pub metadata: Option<serde_json::Value>,
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
    async fn save(&mut self, entry: MemoryEntry) -> Result<(), Box<dyn std::error::Error + Send + Sync>>;
    async fn search(
        &self,
        query_embedding: &[f32],
        top_k: usize,
    ) -> Result<Vec<MemorySearchResult>, Box<dyn std::error::Error + Send + Sync>>;
    async fn get(&self, id: &str) -> Result<Option<MemoryEntry>, Box<dyn std::error::Error + Send + Sync>>;
    async fn reindex(&mut self) -> Result<(), Box<dyn std::error::Error + Send + Sync>>;
}

/// Brute-force in-memory vector store (no index, cosine similarity scan).
pub struct BruteForceStore {
    entries: Vec<MemoryEntry>,
}

impl BruteForceStore {
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
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
    async fn save(&mut self, entry: MemoryEntry) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        self.entries.push(entry);
        Ok(())
    }

    async fn search(
        &self,
        _query_embedding: &[f32],
        _top_k: usize,
    ) -> Result<Vec<MemorySearchResult>, Box<dyn std::error::Error + Send + Sync>> {
        // TODO: implement cosine similarity search
        Ok(Vec::new())
    }

    async fn get(&self, id: &str) -> Result<Option<MemoryEntry>, Box<dyn std::error::Error + Send + Sync>> {
        Ok(self.entries.iter().find(|e| e.id == id).cloned())
    }

    async fn reindex(&mut self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        // No-op for brute force
        Ok(())
    }
}

/// High-level memory store wrapping a VectorStore.
pub struct MemoryStore {
    data_dir: String,
}

impl MemoryStore {
    pub fn new(data_dir: &str) -> Self {
        Self {
            data_dir: data_dir.to_string(),
        }
    }

    pub fn data_dir(&self) -> &str {
        &self.data_dir
    }
}
