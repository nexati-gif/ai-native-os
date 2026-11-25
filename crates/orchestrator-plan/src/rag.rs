//! RAG (Retrieval-Augmented Generation) index using Qdrant

use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::Mutex;
use tracing::{info, warn, debug, error};
use orchestrator_common::{Error, Result, MetricsRecorder};
use qdrant_client::qdrant::{
    CreateCollection, Distance, VectorParams, PointStruct, SearchPoints, 
    UpsertPoints, Filter, FieldCondition, MatchValue, Condition,
    Payload, Value as QdrantValue
};
use qdrant_client::Qdrant;

/// RAG document chunk
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct DocumentChunk {
    pub id: String,
    pub content: String,
    pub source: String,      // e.g., "kernel_docs", "nixpkgs", "user_code", "logs"
    pub metadata: serde_json::Value,
    pub embedding: Option<Vec<f32>>,
}

/// RAG index manager
pub struct RagIndex {
    client: Qdrant,
    collection_name: String,
    embedder: Arc<Mutex<Option<Embedder>>>,
    metrics: MetricsRecorder,
    vector_dim: usize,
}

impl RagIndex {
    pub async fn new(path: PathBuf, vector_dim: usize) -> Result<Self> {
        // Use embedded Qdrant
        let client = Qdrant::from_url(&format!("file://{}", path.display())).build()?;
        
        let index = Self {
            client,
            collection_name: "orchestrator_rag".to_string(),
            embedder: Arc::new(Mutex::new(None)),
            metrics: MetricsRecorder::new("rag"),
            vector_dim,
        };
        
        index.init_collection().await?;
        Ok(index)
    }
    
    async fn init_collection(&self) -> Result<()> {
        let collections = self.client.list_collections().await?;
        
        if !collections.collections.iter().any(|c| c.name == self.collection_name) {
            info!("Creating RAG collection: {}", self.collection_name);
            
            self.client
                .create_collection(CreateCollection {
                    collection_name: self.collection_name.clone(),
                    vectors_config: Some(VectorParams {
                        size: self.vector_dim as u64,
                        distance: Distance::Cosine.into(),
                        ..Default::default()
                    }.into()),
                    ..Default::default()
                })
                .await?;
        }
        
        Ok(())
    }
    
    /// Initialize embedder (lazy)
    async fn ensure_embedder(&self) -> Result<()> {
        let mut embedder_guard = self.embedder.lock().await;
        if embedder_guard.is_none() {
            *embedder_guard = Some(Embedder::new().await?);
        }
        Ok(())
    }
    
    /// Add document chunks to index
    pub async fn add_documents(&self, chunks: Vec<DocumentChunk>) -> Result<()> {
        self.ensure_embedder().await?;
        
        let embedder = self.embedder.lock().await;
        let embedder = embedder.as_ref().unwrap();
        
        let mut points = Vec::new();
        
        for chunk in chunks {
            let embedding = if let Some(emb) = chunk.embedding {
                emb
            } else {
                embedder.embed(&chunk.content).await?
            };
            
            let mut payload = serde_json::json!({
                "content": chunk.content,
                "source": chunk.source,
                "metadata": chunk.metadata,
            });
            
            points.push(PointStruct {
                id: Some(chunk.id.into()),
                vectors: Some(embedding.into()),
                payload: payload.into(),
            });
        }
        
        self.client
            .upsert_points(UpsertPoints {
                collection_name: self.collection_name.clone(),
                points,
                ..Default::default()
            })
            .await?;
        
        self.metrics.counter("documents_indexed", points.len() as u64);
        info!("Indexed {} document chunks", points.len());
        Ok(())
    }
    
    /// Search for relevant chunks
    pub async fn search(&self, query: &str, limit: usize, source_filter: Option<&str>) -> Result<Vec<DocumentChunk>> {
        self.ensure_embedder().await?;
        
        let embedder = self.embedder.lock().await;
        let embedder = embedder.as_ref().unwrap();
        
        let query_embedding = embedder.embed(query).await?;
        
        let mut filter = None;
        if let Some(source) = source_filter {
            filter = Some(Filter {
                must: vec![Condition::Field(FieldCondition {
                    key: "source".to_string(),
                    match_: Some(MatchValue {
                        value: Some(source.into()),
                    }),
                    ..Default::default()
                })],
                ..Default::default()
            });
        }
        
        let search_result = self.client
            .search_points(SearchPoints {
                collection_name: self.collection_name.clone(),
                vector: query_embedding,
                limit: limit as u64,
                filter,
                with_payload: Some(true.into()),
                with_vectors: Some(false.into()),
                ..Default::default()
            })
            .await?;
        
        let mut chunks = Vec::new();
        for scored_point in search_result.result {
            if let Some(payload) = scored_point.payload {
                let content = payload.get("content")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                
                let source = payload.get("source")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                
                let metadata = payload.get("metadata")
                    .cloned()
                    .unwrap_or(serde_json::Value::Null);
                
                chunks.push(DocumentChunk {
                    id: scored_point.id.unwrap_or_default().to_string(),
                    content,
                    source,
                    metadata,
                    embedding: None,
                });
            }
        }
        
        self.metrics.histogram("search_latency_ms", 0.0); // Would measure actual latency
        self.metrics.counter("searches", 1);
        
        Ok(chunks)
    }
    
    /// Hybrid search: combine vector + keyword
    pub async fn hybrid_search(&self, query: &str, limit: usize) -> Result<Vec<DocumentChunk>> {
        // For now, just vector search
        // Could be extended with BM25 or full-text search
        self.search(query, limit, None).await
    }
    
    /// Get collection stats
    pub async fn stats(&self) -> Result<RagStats> {
        let info = self.client.collection_info(&self.collection_name).await?;
        
        Ok(RagStats {
            vectors_count: info.result.unwrap_or_default().vectors_count.unwrap_or(0) as usize,
            indexed_vectors: info.result.unwrap_or_default().indexed_vectors_count.unwrap_or(0) as usize,
            points_count: info.result.unwrap_or_default().points_count.unwrap_or(0) as usize,
        })
    }
    
    /// Delete documents by source
    pub async fn delete_by_source(&self, source: &str) -> Result<()> {
        self.client
            .delete_points(qdrant_client::qdrant::DeletePoints {
                collection_name: self.collection_name.clone(),
                points: Some(qdrant_client::qdrant::PointsSelector {
                    points_selector: Some(qdrant_client::qdrant::points_selector::PointsSelector::Filter(Filter {
                        must: vec![Condition::Field(FieldCondition {
                            key: "source".to_string(),
                            match_: Some(MatchValue {
                                value: Some(source.into()),
                            }),
                            ..Default::default()
                        })],
                        ..Default::default()
                    })),
                }),
                ..Default::default()
            })
            .await?;
        
        Ok(())
    }
}

/// Embedder for generating vector embeddings
struct Embedder {
    model: Option<candle_core::Device>,
    tokenizer: Option<tokenizers::Tokenizer>,
    embed_model: Option<candle_nn::VarMap>,
}

impl Embedder {
    async fn new() -> Result<Self> {
        // Load embedding model (e.g., BGE-small, E5-small, or all-MiniLM-L6-v2)
        // This is a placeholder - real implementation would load a sentence transformer
        Ok(Self {
            model: None,
            tokenizer: None,
            embed_model: None,
        })
    }
    
    async fn embed(&self, text: &str) -> Result<Vec<f32>> {
        // Placeholder - would run text through embedding model
        // For now, return a deterministic hash-based vector
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};
        
        let mut hasher = DefaultHasher::new();
        text.hash(&mut hasher);
        let hash = hasher.finish();
        
        // Generate pseudo-random vector from hash
        let mut vec = Vec::with_capacity(384); // Common embedding dim
        let mut state = hash;
        for _ in 0..384 {
            state = state.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            vec.push(((state >> 32) as f32) / (u32::MAX as f32));
        }
        
        // Normalize
        let norm: f32 = vec.iter().map(|x| x * x).sum::<f32>().sqrt();
        if norm > 0.0 {
            for x in &mut vec {
                *x /= norm;
            }
        }
        
        Ok(vec)
    }
}

/// RAG index statistics
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct RagStats {
    pub vectors_count: usize,
    pub indexed_vectors: usize,
    pub points_count: usize,
}

/// Pre-built knowledge sources for RAG
pub mod sources {
    use super::*;
    use std::fs;
    use std::path::Path;
    
    /// Load kernel documentation
    pub async fn load_kernel_docs(index: &RagIndex, kernel_src: &Path) -> Result<()> {
        let mut chunks = Vec::new();
        
        // Walk kernel source tree for .c, .h files and Documentation/
        for entry in walkdir::WalkDir::new(kernel_src) {
            let entry = entry?;
            let path = entry.path();
            
            if path.extension().map_or(false, |e| e == "c" || e == "h") {
                if let Ok(content) = fs::read_to_string(path) {
                    // Split into functions/sections
                    for (i, chunk) in split_code(&content).enumerate() {
                        if chunk.len() > 100 {
                            chunks.push(DocumentChunk {
                                id: format!("kernel:{}:{}", path.display(), i),
                                content: chunk,
                                source: "kernel_docs".to_string(),
                                metadata: serde_json::json!({
                                    "file": path.to_string_lossy(),
                                    "type": "source",
                                }),
                                embedding: None,
                            });
                        }
                    }
                }
            }
        }
        
        if !chunks.is_empty() {
            index.add_documents(chunks).await?;
        }
        
        Ok(())
    }
    
    /// Load Nixpkgs documentation and derivations
    pub async fn load_nixpkgs(index: &RagIndex, nixpkgs_path: &Path) -> Result<()> {
        let mut chunks = Vec::new();
        
        // Load package derivations
        for entry in walkdir::WalkDir::new(nixpkgs_path).max_depth(4) {
            let entry = entry?;
            let path = entry.path();
            
            if path.extension().map_or(false, |e| e == "nix") {
                if let Ok(content) = fs::read_to_string(path) {
                    for (i, chunk) in split_nix(&content).enumerate() {
                        if chunk.len() > 50 {
                            chunks.push(DocumentChunk {
                                id: format!("nixpkgs:{}:{}", path.display(), i),
                                content: chunk,
                                source: "nixpkgs".to_string(),
                                metadata: serde_json::json!({
                                    "file": path.to_string_lossy(),
                                    "type": "derivation",
                                }),
                                embedding: None,
                            });
                        }
                    }
                }
            }
        }
        
        if !chunks.is_empty() {
            index.add_documents(chunks).await?;
        }
        
        Ok(())
    }
    
    /// Load user's configuration and code
    pub async fn load_user_code(index: &RagIndex, config_dir: &Path) -> Result<()> {
        let mut chunks = Vec::new();
        
        for entry in walkdir::WalkDir::new(config_dir) {
            let entry = entry?;
            let path = entry.path();
            
            if path.extension().map_or(false, |e| {
                matches!(e.to_str(), Some("rs" | "py" | "js" | "ts" | "nix" | "toml" | "yaml" | "yml"))
            }) {
                if let Ok(content) = fs::read_to_string(path) {
                    for (i, chunk) in split_generic(&content).enumerate() {
                        if chunk.len() > 50 {
                            chunks.push(DocumentChunk {
                                id: format!("user:{}:{}", path.display(), i),
                                content: chunk,
                                source: "user_code".to_string(),
                                metadata: serde_json::json!({
                                    "file": path.to_string_lossy(),
                                }),
                                embedding: None,
                            });
                        }
                    }
                }
            }
        }
        
        if !chunks.is_empty() {
            index.add_documents(chunks).await?;
        }
        
        Ok(())
    }
    
    /// Load system logs
    pub async fn load_logs(index: &RagIndex, journal_path: &Path, max_entries: usize) -> Result<()> {
        // Would use journalctl or read journal files
        // Placeholder
        Ok(())
    }
    
    fn split_code(content: &str) -> Vec<String> {
        // Split C code by functions
        let mut chunks = Vec::new();
        let mut current = String::new();
        let mut brace_count = 0;
        let mut in_function = false;
        
        for line in content.lines() {
            current.push_str(line);
            current.push('\n');
            
            for ch in line.chars() {
                if ch == '{' {
                    brace_count += 1;
                    if brace_count == 1 {
                        in_function = true;
                    }
                } else if ch == '}' {
                    if brace_count > 0 {
                        brace_count -= 1;
                    }
                    if brace_count == 0 && in_function {
                        if current.len() > 200 {
                            chunks.push(current.clone());
                        }
                        current.clear();
                        in_function = false;
                    }
                }
            }
        }
        
        if !current.is_empty() && current.len() > 200 {
            chunks.push(current);
        }
        
        chunks
    }
    
    fn split_nix(content: &str) -> Vec<String> {
        // Split Nix by top-level attributes
        let mut chunks = Vec::new();
        let mut current = String::new();
        let mut brace_count = 0;
        
        for line in content.lines() {
            current.push_str(line);
            current.push('\n');
            
            for ch in line.chars() {
                if ch == '{' { brace_count += 1; }
                else if ch == '}' { 
                    if brace_count > 0 { brace_count -= 1; }
                }
            }
            
            if brace_count == 0 && current.len() > 100 {
                chunks.push(current.clone());
                current.clear();
            }
        }
        
        if !current.is_empty() && current.len() > 100 {
            chunks.push(current);
        }
        
        chunks
    }
    
    fn split_generic(content: &str) -> Vec<String> {
        // Simple paragraph-based splitting
        content
            .split("\n\n")
            .filter(|s| s.len() > 100)
            .map(|s| s.to_string())
            .collect()
    }
}