//! Model manager for LLM inference with lazy loading and auto-unload

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{Mutex, RwLock};
use tracing::{info, warn, debug, error};
use orchestrator_common::{Error, Result};
use orchestrator_common::config::ModelConfig;
use async_trait::async_trait;

/// Trait for different inference backends
#[async_trait]
pub trait InferenceEngine: Send + Sync {
    /// Load the model
    async fn load(&mut self, config: &ModelConfig) -> Result<()>;
    
    /// Check if model is loaded
    fn is_loaded(&self) -> bool;
    
    /// Generate completion
    async fn generate(&self, prompt: &str, params: &GenerationParams) -> Result<String>;
    
    /// Generate with streaming
    async fn generate_stream(&self, prompt: &str, params: &GenerationParams, callback: Box<dyn FnMut(String) + Send>) -> Result<()>;
    
    /// Unload the model
    async fn unload(&mut self) -> Result<()>;
    
    /// Get model info
    fn model_info(&self) -> ModelInfo;
}

/// Generation parameters
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct GenerationParams {
    pub max_tokens: usize,
    pub temperature: f32,
    pub top_p: f32,
    pub top_k: usize,
    pub repetition_penalty: f32,
    pub stop_sequences: Vec<String>,
    pub seed: Option<u64>,
}

impl Default for GenerationParams {
    fn default() -> Self {
        Self {
            max_tokens: 4096,
            temperature: 0.2,
            top_p: 0.95,
            top_k: 50,
            repetition_penalty: 1.1,
            stop_sequences: vec!["</proposal>".to_string()],
            seed: None,
        }
    }
}

/// Model information
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ModelInfo {
    pub name: String,
    pub path: PathBuf,
    pub quantization: String,
    pub context_length: usize,
    pub parameter_count: String,
    pub loaded: bool,
    pub vram_usage_mib: Option<u64>,
    pub ram_usage_mib: Option<u64>,
}

/// Model state
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelState {
    Unloaded,
    Loading,
    Loaded,
    Unloading,
    Error,
}

/// Model manager with lazy loading and auto-unload
pub struct ModelManager {
    config: ModelConfig,
    engine: Arc<Mutex<Option<Box<dyn InferenceEngine>>>>,
    state: Arc<RwLock<ModelState>>,
    last_used: Arc<RwLock<Option<Instant>>>,
    idle_timeout: Duration,
    load_lock: Arc<tokio::sync::Mutex<()>>, // Prevent concurrent loads
}

impl ModelManager {
    pub fn new(config: ModelConfig) -> Self {
        let idle_timeout = Duration::from_secs(config.idle_timeout_secs.unwrap_or(30));
        
        Self {
            config,
            engine: Arc::new(Mutex::new(None)),
            state: Arc::new(RwLock::new(ModelState::Unloaded)),
            last_used: Arc::new(RwLock::new(None)),
            idle_timeout,
            load_lock: Arc::new(tokio::sync::Mutex::new(())),
        }
    }
    
    /// Get or load the model
    pub async fn get_engine(&self) -> Result<Arc<Mutex<Option<Box<dyn InferenceEngine>>>>> {
        self.ensure_loaded().await?;
        Ok(self.engine.clone())
    }
    
    /// Ensure model is loaded
    pub async fn ensure_loaded(&self) -> Result<()> {
        let mut state = self.state.write().await;
        
        match *state {
            ModelState::Loaded => {
                // Update last used time
                *self.last_used.write().await = Some(Instant::now());
                return Ok(());
            }
            ModelState::Loading => {
                // Wait for loading to complete
                drop(state);
                while *self.state.read().await == ModelState::Loading {
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
                return Ok(());
            }
            ModelState::Error => {
                return Err(Error::Model("Model in error state".to_string()));
            }
            _ => {}
        }
        
        // Acquire load lock to prevent concurrent loads
        let _guard = self.load_lock.lock().await;
        
        // Double-check after acquiring lock
        if *self.state.read().await == ModelState::Loaded {
            *self.last_used.write().await = Some(Instant::now());
            return Ok(());
        }
        
        *state = ModelState::Loading;
        drop(state);
        
        // Create engine based on backend
        let mut engine: Box<dyn InferenceEngine> = match self.config.backend.as_str() {
            "candle" => Box::new(CandleEngine::new()),
            "llamacpp" => Box::new(LlamaCppEngine::new()),
            "vllm" => Box::new(VllmEngine::new()),
            _ => return Err(Error::Config(format!("Unknown inference backend: {}", self.config.backend))),
        };
        
        info!("Loading model from {} using {}", self.config.model_path.display(), self.config.backend);
        
        // Load the model
        if let Err(e) = engine.load(&self.config).await {
            *self.state.write().await = ModelState::Error;
            error!("Failed to load model: {}", e);
            return Err(e);
        }
        
        *self.engine.lock().await = Some(engine);
        *self.state.write().await = ModelState::Loaded;
        *self.last_used.write().await = Some(Instant::now());
        
        info!("Model loaded successfully");
        Ok(())
    }
    
    /// Generate completion
    pub async fn generate(&self, prompt: &str, params: Option<GenerationParams>) -> Result<String> {
        self.ensure_loaded().await?;
        
        let engine_guard = self.engine.lock().await;
        let engine = engine_guard.as_ref().ok_or_else(|| Error::Model("Engine not loaded".to_string()))?;
        
        let params = params.unwrap_or_default();
        let result = engine.generate(prompt, &params).await;
        
        // Update last used
        *self.last_used.write().await = Some(Instant::now());
        
        result
    }
    
    /// Generate with streaming
    pub async fn generate_stream<F>(&self, prompt: &str, params: Option<GenerationParams>, mut callback: F) -> Result<()>
    where
        F: FnMut(String) + Send + 'static,
    {
        self.ensure_loaded().await?;
        
        let engine_guard = self.engine.lock().await;
        let engine = engine_guard.as_ref().ok_or_else(|| Error::Model("Engine not loaded".to_string()))?;
        
        let params = params.unwrap_or_default();
        let result = engine.generate_stream(prompt, &params, Box::new(move |s| callback(s))).await;
        
        *self.last_used.write().await = Some(Instant::now());
        
        result
    }
    
    /// Check if model is loaded
    pub async fn is_loaded(&self) -> bool {
        *self.state.read().await == ModelState::Loaded
    }
    
    /// Get current state
    pub async fn state(&self) -> ModelState {
        *self.state.read().await
    }
    
    /// Get model info
    pub async fn info(&self) -> ModelInfo {
        let engine_guard = self.engine.lock().await;
        if let Some(engine) = engine_guard.as_ref() {
            engine.model_info()
        } else {
            ModelInfo {
                name: "unknown".to_string(),
                path: self.config.model_path.clone(),
                quantization: self.config.quantization.clone().unwrap_or_default(),
                context_length: self.config.context_length.unwrap_or(32768),
                parameter_count: "unknown".to_string(),
                loaded: false,
                vram_usage_mib: None,
                ram_usage_mib: None,
            }
        }
    }
    
    /// Unload model
    pub async fn unload(&self) -> Result<()> {
        let mut state = self.state.write().await;
        
        if *state == ModelState::Unloaded {
            return Ok(());
        }
        
        if *state == ModelState::Loading {
            // Wait for loading to complete first
            drop(state);
            while *self.state.read().await == ModelState::Loading {
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            state = self.state.write().await;
        }
        
        *state = ModelState::Unloading;
        drop(state);
        
        let mut engine_guard = self.engine.lock().await;
        if let Some(mut engine) = engine_guard.take() {
            engine.unload().await?;
        }
        
        *self.state.write().await = ModelState::Unloaded;
        *self.last_used.write().await = None;
        
        info!("Model unloaded");
        Ok(())
    }
    
    /// Check if idle timeout has elapsed
    pub async fn is_idle(&self) -> bool {
        let last_used = self.last_used.read().await;
        if let Some(last) = *last_used {
            last.elapsed() > self.idle_timeout
        } else {
            false
        }
    }
    
    /// Start idle monitor task
    pub fn start_idle_monitor(self: Arc<Self>) {
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(10));
            loop {
                interval.tick().await;
                if self.is_idle().await && self.is_loaded().await {
                    info!("Model idle for {}s, unloading", self.idle_timeout.as_secs());
                    if let Err(e) = self.unload().await {
                        warn!("Failed to unload idle model: {}", e);
                    }
                }
            }
        });
    }
}

/// Candle inference engine (preferred)
pub struct CandleEngine {
    device: Option<candle_core::Device>,
    tokenizer: Option<tokenizers::Tokenizer>,
    model_info: ModelInfo,
}

impl CandleEngine {
    pub fn new() -> Self {
        Self {
            device: None,
            tokenizer: None,
            model_info: ModelInfo {
                name: "candle".to_string(),
                path: PathBuf::new(),
                quantization: String::new(),
                context_length: 32768,
                parameter_count: String::new(),
                loaded: false,
                vram_usage_mib: None,
                ram_usage_mib: None,
            },
        }
    }
}

#[async_trait]
impl InferenceEngine for CandleEngine {
    async fn load(&mut self, config: &ModelConfig) -> Result<()> {
        use candle_core::{Device, DType};
        use tokenizers::Tokenizer;
        
        // Determine device
        let device = if config.gpu_layers.unwrap_or(-1) > 0 {
            #[cfg(feature = "cuda")]
            { Device::new_cuda(0)? }
            #[cfg(feature = "metal")]
            { Device::new_metal(0)? }
            #[cfg(not(any(feature = "cuda", feature = "metal")))]
            { Device::Cpu }
        } else {
            Device::Cpu
        };
        
        info!("Loading model on device: {:?}", device);
        
        // Load tokenizer
        let tokenizer_path = config.model_path.with_extension("tokenizer.json");
        let tokenizer = if tokenizer_path.exists() {
            Tokenizer::from_file(&tokenizer_path).map_err(|e| Error::Model(format!("Tokenizer load failed: {}", e)))?
        } else {
            // Try to load from model directory
            let model_dir = config.model_path.parent().unwrap_or(&config.model_path);
            let tokenizer_path = model_dir.join("tokenizer.json");
            Tokenizer::from_file(&tokenizer_path).map_err(|e| Error::Model(format!("Tokenizer load failed: {}", e)))?
        };
        
        // For GGUF models, we'd use candle's gguf support
        // This is a placeholder - actual implementation depends on model format
        
        self.device = Some(device);
        self.tokenizer = Some(tokenizer);
        self.model_info = ModelInfo {
            name: "Qwen2.5-Coder".to_string(),
            path: config.model_path.clone(),
            quantization: config.quantization.clone().unwrap_or("Q4_K_M".to_string()),
            context_length: config.context_length.unwrap_or(32768),
            parameter_count: "32B".to_string(),
            loaded: true,
            vram_usage_mib: Some(16000),
            ram_usage_mib: Some(20000),
        };
        
        Ok(())
    }
    
    fn is_loaded(&self) -> bool {
        self.device.is_some()
    }
    
    async fn generate(&self, prompt: &str, params: &GenerationParams) -> Result<String> {
        if !self.is_loaded() {
            return Err(Error::Model("Model not loaded".to_string()));
        }
        
        // Placeholder - actual generation would use candle's sampling
        // This would involve:
        // 1. Tokenize prompt
        // 2. Run forward pass with sampling
        // 3. Decode tokens
        
        Err(Error::NotImplemented("Candle generation not yet implemented".to_string()))
    }
    
    async fn generate_stream(&self, _prompt: &str, _params: &GenerationParams, _callback: Box<dyn FnMut(String) + Send>) -> Result<()> {
        Err(Error::NotImplemented("Candle streaming not yet implemented".to_string()))
    }
    
    async fn unload(&mut self) -> Result<()> {
        self.device = None;
        self.tokenizer = None;
        self.model_info.loaded = false;
        Ok(())
    }
    
    fn model_info(&self) -> ModelInfo {
        self.model_info.clone()
    }
}

/// llama.cpp inference engine (fallback)
pub struct LlamaCppEngine {
    ctx: Option<llama_cpp::LlamaContext>,
    model_info: ModelInfo,
}

impl LlamaCppEngine {
    pub fn new() -> Self {
        Self {
            ctx: None,
            model_info: ModelInfo {
                name: "llamacpp".to_string(),
                path: PathBuf::new(),
                quantization: String::new(),
                context_length: 32768,
                parameter_count: String::new(),
                loaded: false,
                vram_usage_mib: None,
                ram_usage_mib: None,
            },
        }
    }
}

#[async_trait]
impl InferenceEngine for LlamaCppEngine {
    async fn load(&mut self, config: &ModelConfig) -> Result<()> {
        use llama_cpp::{LlamaModel, LlamaContextParams, LlamaBackend};
        
        let backend = LlamaBackend::new().map_err(|e| Error::Model(format!("llama.cpp backend init failed: {}", e)))?;
        
        let model = LlamaModel::load_from_file(
            &backend,
            &config.model_path,
            &Default::default(),
        ).map_err(|e| Error::Model(format!("Model load failed: {}", e)))?;
        
        let ctx_params = LlamaContextParams::default()
            .with_n_ctx(config.context_length.unwrap_or(32768) as i32)
            .with_n_gpu_layers(config.gpu_layers.unwrap_or(-1));
        
        let ctx = model.new_context(&backend, ctx_params)
            .map_err(|e| Error::Model(format!("Context creation failed: {}", e)))?;
        
        self.ctx = Some(ctx);
        self.model_info = ModelInfo {
            name: "Qwen2.5-Coder".to_string(),
            path: config.model_path.clone(),
            quantization: config.quantization.clone().unwrap_or("Q4_K_M".to_string()),
            context_length: config.context_length.unwrap_or(32768),
            parameter_count: "32B".to_string(),
            loaded: true,
            vram_usage_mib: Some(16000),
            ram_usage_mib: Some(20000),
        };
        
        Ok(())
    }
    
    fn is_loaded(&self) -> bool {
        self.ctx.is_some()
    }
    
    async fn generate(&self, prompt: &str, params: &GenerationParams) -> Result<String> {
        let ctx = self.ctx.as_ref().ok_or_else(|| Error::Model("Context not loaded".to_string()))?;
        
        // Placeholder for actual generation
        // Would use llama_cpp::LlamaContext::generate
        
        Err(Error::NotImplemented("llama.cpp generation not yet implemented".to_string()))
    }
    
    async fn generate_stream(&self, _prompt: &str, _params: &GenerationParams, _callback: Box<dyn FnMut(String) + Send>) -> Result<()> {
        Err(Error::NotImplemented("llama.cpp streaming not yet implemented".to_string()))
    }
    
    async fn unload(&mut self) -> Result<()> {
        self.ctx = None;
        self.model_info.loaded = false;
        Ok(())
    }
    
    fn model_info(&self) -> ModelInfo {
        self.model_info.clone()
    }
}

/// vLLM / TGI remote inference engine
pub struct VllmEngine {
    client: reqwest::Client,
    base_url: String,
    model_info: ModelInfo,
}

impl VllmEngine {
    pub fn new() -> Self {
        Self {
            client: reqwest::Client::new(),
            base_url: String::new(),
            model_info: ModelInfo {
                name: "vllm".to_string(),
                path: PathBuf::new(),
                quantization: String::new(),
                context_length: 32768,
                parameter_count: String::new(),
                loaded: true, // Remote is always "loaded"
                vram_usage_mib: None,
                ram_usage_mib: None,
            },
        }
    }
}

#[async_trait]
impl InferenceEngine for VllmEngine {
    async fn load(&mut self, config: &ModelConfig) -> Result<()> {
        self.base_url = config.vllm_url.clone().unwrap_or("http://localhost:8000".to_string());
        self.model_info.path = config.model_path.clone();
        Ok(())
    }
    
    fn is_loaded(&self) -> bool {
        true // Remote service
    }
    
    async fn generate(&self, prompt: &str, params: &GenerationParams) -> Result<String> {
        let url = format!("{}/v1/completions", self.base_url);
        
        let request = serde_json::json!({
            "model": self.model_info.path.file_name().unwrap_or_default().to_string_lossy(),
            "prompt": prompt,
            "max_tokens": params.max_tokens,
            "temperature": params.temperature,
            "top_p": params.top_p,
            "top_k": params.top_k,
            "repetition_penalty": params.repetition_penalty,
            "stop": params.stop_sequences,
            "seed": params.seed,
        });
        
        let response = self.client
            .post(&url)
            .json(&request)
            .send()
            .await
            .map_err(|e| Error::Model(format!("vLLM request failed: {}", e)))?;
        
        if !response.status().is_success() {
            let err = response.text().await.unwrap_or_default();
            return Err(Error::Model(format!("vLLM error: {}", err)));
        }
        
        let json: serde_json::Value = response.json().await
            .map_err(|e| Error::Model(format!("vLLM response parse failed: {}", e)))?;
        
        let text = json["choices"][0]["text"].as_str()
            .ok_or_else(|| Error::Model("No text in vLLM response".to_string()))?;
        
        Ok(text.to_string())
    }
    
    async fn generate_stream(&self, prompt: &str, params: &GenerationParams, mut callback: Box<dyn FnMut(String) + Send>) -> Result<()> {
        let url = format!("{}/v1/completions", self.base_url);
        
        let request = serde_json::json!({
            "model": self.model_info.path.file_name().unwrap_or_default().to_string_lossy(),
            "prompt": prompt,
            "max_tokens": params.max_tokens,
            "temperature": params.temperature,
            "top_p": params.top_p,
            "top_k": params.top_k,
            "repetition_penalty": params.repetition_penalty,
            "stop": params.stop_sequences,
            "seed": params.seed,
            "stream": true,
        });
        
        let response = self.client
            .post(&url)
            .json(&request)
            .send()
            .await
            .map_err(|e| Error::Model(format!("vLLM stream request failed: {}", e)))?;
        
        let mut stream = response.bytes_stream();
        use futures::StreamExt;
        
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|e| Error::Model(format!("Stream error: {}", e)))?;
            let text = String::from_utf8_lossy(&chunk);
            
            for line in text.lines() {
                if line.starts_with("data: ") {
                    let data = &line[6..];
                    if data == "[DONE]" {
                        return Ok(());
                    }
                    if let Ok(json) = serde_json::from_str::<serde_json::Value>(data) {
                        if let Some(token) = json["choices"][0]["text"].as_str() {
                            callback(token.to_string());
                        }
                    }
                }
            }
        }
        
        Ok(())
    }
    
    async fn unload(&mut self) -> Result<()> {
        // Nothing to unload for remote
        Ok(())
    }
    
    fn model_info(&self) -> ModelInfo {
        self.model_info.clone()
    }
}