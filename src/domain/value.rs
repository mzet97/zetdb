use bytes::Bytes;
use std::time::{Duration, Instant};

pub struct ValueEntry {
    pub data: Bytes,
    pub expires_at: Option<Instant>,
}

impl ValueEntry {
    pub fn new(data: Bytes) -> Self {
        Self {
            data,
            expires_at: None,
        }
    }

    pub fn with_ttl(data: Bytes, ttl: Duration) -> Self {
        Self {
            data,
            expires_at: Some(Instant::now() + ttl),
        }
    }

    pub fn is_expired(&self) -> bool {
        self.expires_at.is_some_and(|exp| Instant::now() >= exp)
    }
}
