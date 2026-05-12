use bytes::Bytes;
use std::time::{Duration, Instant};

pub struct ValueEntry {
    pub data: Bytes,
    pub expires_at: Option<Instant>,
    /// Approximate creation / last-write time for eviction sampling.
    pub created_at: Instant,
}

impl ValueEntry {
    pub fn new(data: Bytes) -> Self {
        Self {
            data,
            expires_at: None,
            created_at: Instant::now(),
        }
    }

    pub fn with_ttl(data: Bytes, ttl: Duration) -> Self {
        Self {
            data,
            expires_at: Some(Instant::now() + ttl),
            created_at: Instant::now(),
        }
    }

    pub fn is_expired(&self) -> bool {
        self.expires_at.is_some_and(|exp| Instant::now() >= exp)
    }

    pub fn is_expired_at(&self, now: Instant) -> bool {
        self.expires_at.is_some_and(|exp| now >= exp)
    }
}
