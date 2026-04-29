use std::time::Duration;

use clap::Parser;

#[derive(Clone, Parser)]
#[command(
    name = "zetdb",
    version,
    about = "High-performance in-memory key-value store"
)]
pub struct Config {
    /// Bind address
    #[arg(long, default_value = "127.0.0.1", env = "ZETDB_BIND_ADDR")]
    pub bind_addr: String,

    /// TCP port
    #[arg(long, default_value_t = 6379, env = "ZETDB_PORT")]
    pub port: u16,

    /// Read timeout in seconds per connection
    #[arg(long, default_value_t = 30, env = "ZETDB_READ_TIMEOUT")]
    pub read_timeout_secs: u64,

    /// TTL sweeper interval in seconds
    #[arg(long, default_value_t = 1, env = "ZETDB_SWEEP_INTERVAL")]
    pub sweep_interval_secs: u64,

    /// Maximum concurrent connections (0 = unlimited)
    #[arg(long, default_value_t = 0, env = "ZETDB_MAX_CONNECTIONS")]
    pub max_connections: usize,

    #[command(flatten)]
    pub snapshot: SnapshotConfig,

    #[command(flatten)]
    pub aof: AofConfig,

    /// Enable metrics counters (INFO command stats)
    #[arg(long, default_value_t = false, env = "ZETDB_METRICS_ENABLED")]
    pub metrics_enabled: bool,
}

impl Config {
    pub fn read_timeout(&self) -> Duration {
        Duration::from_secs(self.read_timeout_secs)
    }

    pub fn sweep_interval(&self) -> Duration {
        Duration::from_secs(self.sweep_interval_secs)
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            bind_addr: "127.0.0.1".to_string(),
            port: 6379,
            read_timeout_secs: 30,
            sweep_interval_secs: 1,
            max_connections: 0,
            snapshot: SnapshotConfig::default(),
            aof: AofConfig::default(),
            metrics_enabled: false,
        }
    }
}

#[derive(Clone, Parser)]
pub struct SnapshotConfig {
    /// Enable snapshot persistence
    #[arg(long, default_value_t = true, env = "ZETDB_SNAPSHOT_ENABLED")]
    pub snapshot_enabled: bool,

    /// Snapshot file path
    #[arg(long, default_value = "dump.zdb", env = "ZETDB_SNAPSHOT_PATH")]
    pub snapshot_path: String,

    /// Snapshot interval in seconds
    #[arg(long, default_value_t = 60, env = "ZETDB_SNAPSHOT_INTERVAL")]
    pub snapshot_interval_secs: u64,
}

impl SnapshotConfig {
    pub fn enabled(&self) -> bool {
        self.snapshot_enabled
    }

    pub fn path(&self) -> &str {
        &self.snapshot_path
    }

    pub fn interval(&self) -> Duration {
        Duration::from_secs(self.snapshot_interval_secs)
    }
}

impl Default for SnapshotConfig {
    fn default() -> Self {
        Self {
            snapshot_enabled: true,
            snapshot_path: "dump.zdb".into(),
            snapshot_interval_secs: 60,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, clap::ValueEnum)]
pub enum FsyncPolicy {
    Always,
    #[default]
    Everysec,
    No,
}

impl FsyncPolicy {
    pub fn is_every_write(&self) -> bool {
        matches!(self, FsyncPolicy::Always)
    }

    pub fn is_every_second(&self) -> bool {
        matches!(self, FsyncPolicy::Everysec)
    }

    pub fn is_never(&self) -> bool {
        matches!(self, FsyncPolicy::No)
    }
}

#[derive(Clone, Parser)]
pub struct AofConfig {
    /// Enable AOF (append-only file) persistence
    #[arg(long, default_value_t = false, env = "ZETDB_AOF_ENABLED")]
    pub aof_enabled: bool,

    /// AOF file path
    #[arg(long, default_value = "appendonly.zdb", env = "ZETDB_AOF_PATH")]
    pub aof_path: String,

    /// AOF fsync policy: always, everysec, no
    #[arg(long, default_value = "everysec", env = "ZETDB_AOF_FSYNC", value_enum)]
    pub aof_fsync: FsyncPolicy,

    /// AOF rewrite threshold in MB
    #[arg(long, default_value_t = 64, env = "ZETDB_AOF_REWRITE_THRESHOLD")]
    pub aof_rewrite_threshold_mb: u64,
}

impl AofConfig {
    pub fn enabled(&self) -> bool {
        self.aof_enabled
    }

    pub fn path(&self) -> &str {
        &self.aof_path
    }

    pub fn fsync(&self) -> FsyncPolicy {
        self.aof_fsync
    }

    pub fn rewrite_threshold_mb(&self) -> u64 {
        self.aof_rewrite_threshold_mb
    }
}

impl Default for AofConfig {
    fn default() -> Self {
        Self {
            aof_enabled: false,
            aof_path: "appendonly.zdb".into(),
            aof_fsync: FsyncPolicy::default(),
            aof_rewrite_threshold_mb: 64,
        }
    }
}
