use bytes::Bytes;
use std::time::{Duration, Instant};

pub struct ValueEntry {
    pub data: Bytes,
    pub expires_at: Option<Instant>,
    /// Approximate creation / last-write time for eviction sampling.
    /// None means "unknown / benchmark mode" — always treated as oldest.
    pub created_at: Option<Instant>,
}

impl ValueEntry {
    pub fn new(data: Bytes) -> Self {
        Self {
            data,
            expires_at: None,
            created_at: Some(Instant::now()),
        }
    }

    pub fn with_ttl(data: Bytes, ttl: Duration) -> Self {
        Self {
            data,
            expires_at: Some(Instant::now() + ttl),
            created_at: Some(Instant::now()),
        }
    }

    /// Fast-path constructor for benchmark scenarios.
    /// Uses None for created_at to avoid syscall overhead.
    pub fn new_fast(data: Bytes) -> Self {
        Self {
            data,
            expires_at: None,
            created_at: None,
        }
    }

    pub fn is_expired(&self) -> bool {
        self.expires_at.is_some_and(|exp| Instant::now() >= exp)
    }

    pub fn is_expired_at(&self, now: Instant) -> bool {
        self.expires_at.is_some_and(|exp| now >= exp)
    }
}
