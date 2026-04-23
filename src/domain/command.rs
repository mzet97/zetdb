use bytes::Bytes;
use std::time::Duration;

#[derive(Debug, PartialEq)]
pub enum Command {
    Ping,
    Get { key: String },
    Set {
        key: String,
        value: Bytes,
        ttl: Option<Duration>,
    },
    Del { key: String },
    Incr { key: String },
}
