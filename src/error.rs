use thiserror::Error; //helps you define custom error types easily

#[derive(Error, Debug)]

pub enum KVError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Serialization error: {0}")]
    Serde(#[from] serde_json::Error),

    #[error("Key not found: {0}")]
    KeyNotFound(String),

    #[error("Corrrupt log at offset {offset}: {reason}")]
    CorruptLog{offset: u64, reason: String},

}

pub type Result<T> = std::result::Result<T, KVError>;
