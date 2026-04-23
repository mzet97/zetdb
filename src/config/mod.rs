use std::time::Duration;

pub struct Config {
    pub bind_addr: String,
    pub port: u16,
    pub read_timeout: Duration,
    pub sweep_interval: Duration,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            bind_addr: "127.0.0.1".to_string(),
            port: 6379,
            read_timeout: Duration::from_secs(30),
            sweep_interval: Duration::from_secs(1),
        }
    }
}
