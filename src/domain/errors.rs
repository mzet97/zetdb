#[derive(Debug)]
pub enum EngineError {
    StorageError(String),
    NotAnInteger(String),
}

#[derive(Debug)]
pub enum DomainError {
    KeyNotFound(String),
    NotAnInteger(String),
    Engine(EngineError),
}

impl std::fmt::Display for DomainError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::KeyNotFound(key) => write!(f, "key not found: {key}"),
            Self::NotAnInteger(key) => write!(f, "value is not an integer: {key}"),
            Self::Engine(e) => write!(f, "engine error: {e}"),
        }
    }
}

impl std::fmt::Display for EngineError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::StorageError(msg) => write!(f, "storage error: {msg}"),
            Self::NotAnInteger(key) => write!(f, "value is not an integer: {key}"),
        }
    }
}

impl std::error::Error for DomainError {}
impl std::error::Error for EngineError {}
