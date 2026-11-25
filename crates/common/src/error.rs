//! Error types for the orchestrator

use std::fmt;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum Error {
    #[error("Configuration error: {0}")]
    Config(#[from] config::ConfigError),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Serialization error: {0}")]
    Serde(#[from] serde_json::Error),

    #[error("TOML parse error: {0}")]
    Toml(#[from] toml::de::Error),

    #[error("Database error: {0}")]
    Database(#[from] redb::Error),

    #[error("eBPF error: {0}")]
    Ebpf(#[from] aya::EbpfError),

    #[error("Nix error: {0}")]
    Nix(String),

    #[error("Sandbox error: {0}")]
    Sandbox(String),

    #[error("Model error: {0}")]
    Model(String),

    #[error("Inference error: {0}")]
    Inference(String),

    #[error("RAG error: {0}")]
    Rag(String),

    #[error("Federation error: {0}")]
    Federation(String),

    #[error("Policy denied: {0}")]
    PolicyDenied(String),

    #[error("Validation failed: {0}")]
    ValidationFailed(String),

    #[error("Rollback failed: {0}")]
    RollbackFailed(String),

    #[error("Timeout: {0}")]
    Timeout(String),

    #[error("Not found: {0}")]
    NotFound(String),

    #[error("Invalid state: {0}")]
    InvalidState(String),

    #[error("Internal error: {0}")]
    Internal(String),

    #[error("Anyhow error: {0}")]
    Anyhow(#[from] anyhow::Error),
}

pub type Result<T> = std::result::Result<T, Error>;

impl Error {
    pub fn is_retryable(&self) -> bool {
        matches!(
            self,
            Error::Timeout(_) | Error::Io(_) | Error::Sandbox(_) | Error::Model(_)
        )
    }

    pub fn severity(&self) -> Severity {
        match self {
            Error::PolicyDenied(_) => Severity::Critical,
            Error::RollbackFailed(_) => Severity::Critical,
            Error::ValidationFailed(_) => Severity::High,
            Error::Database(_) | Error::Ebpf(_) => Severity::High,
            Error::Config(_) | Error::Nix(_) => Severity::High,
            Error::Model(_) | Error::Inference(_) | Error::Rag(_) => Severity::Medium,
            Error::Sandbox(_) | Error::Federation(_) => Severity::Medium,
            Error::Timeout(_) | Error::Io(_) => Severity::Medium,
            Error::NotFound(_) | Error::InvalidState(_) => Severity::Low,
            Error::Internal(_) | Error::Anyhow(_) => Severity::High,
            Error::Serde(_) | Error::Toml(_) => Severity::Low,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    Low,
    Medium,
    High,
    Critical,
}

impl fmt::Display for Severity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Severity::Low => write!(f, "low"),
            Severity::Medium => write!(f, "medium"),
            Severity::High => write!(f, "high"),
            Severity::Critical => write!(f, "critical"),
        }
    }
}