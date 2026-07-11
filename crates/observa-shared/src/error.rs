use thiserror::Error;

#[derive(Debug, Error)]
pub enum ObservaError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("configuration error: {0}")]
    Config(String),
    #[error("database error: {0}")]
    Database(String),
    #[error("cache error: {0}")]
    Cache(String),
    #[error("event bus error: {0}")]
    EventBus(String),
    #[error("llm error: {0}")]
    Llm(String),
    #[error("store error: {0}")]
    Store(String),
}

pub type Result<T> = std::result::Result<T, ObservaError>;
