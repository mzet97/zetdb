use std::time::Duration;

#[derive(Clone)]
pub struct SnapshotConfig {
    pub enabled: bool,
    pub path: String,
    pub interval: Duration,
}

impl Default for SnapshotConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            path: "dump.zdb".into(),
            interval: Duration::from_secs(60),
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub enum FsyncPolicy {
    EveryWrite,
    EverySecond,
    Never,
}

impl Default for FsyncPolicy {
    fn default() -> Self {
        Self::EverySecond
    }
}

#[derive(Clone)]
pub struct AofConfig {
    pub enabled: bool,
    pub path: String,
    pub fsync: FsyncPolicy,
    pub rewrite_threshold_mb: u64,
}

impl Default for AofConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            path: "appendonly.zdb".into(),
            fsync: FsyncPolicy::default(),
            rewrite_threshold_mb: 64,
        }
    }
}

pub struct Config {
    pub bind_addr: String,
    pub port: u16,
    pub read_timeout: Duration,
    pub sweep_interval: Duration,
    pub snapshot: SnapshotConfig,
    pub aof: AofConfig,
    pub metrics_enabled: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            bind_addr: "127.0.0.1".to_string(),
            port: 6379,
            read_timeout: Duration::from_secs(30),
            sweep_interval: Duration::from_secs(1),
            snapshot: SnapshotConfig::default(),
            aof: AofConfig::default(),
            metrics_enabled: false,
        }
    }
}
