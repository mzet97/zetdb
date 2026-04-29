pub mod application;
pub mod config;
pub mod domain;
pub mod observability;
pub mod protocol;
pub mod server;
pub mod storage;

// Re-exports for ergonomic public API
pub use application::dispatcher::dispatch;
pub use config::{AofConfig, Config, FsyncPolicy, SnapshotConfig};
pub use domain::command::Command;
pub use domain::errors::{DomainError, EngineError};
pub use domain::value::ValueEntry;
pub use protocol::response::{Response, ResponseError};
pub use server::tcp::{run_server, run_server_with_shutdown};
pub use storage::dashmap_engine::DashMapEngine;
pub use storage::engine::KvEngine;
