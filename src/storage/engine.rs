use crate::domain::errors::EngineError;
use crate::domain::value::ValueEntry;

pub trait KvEngine: Send + Sync {
    fn get(&self, key: &str) -> Result<Option<ValueEntry>, EngineError>;
    fn set(&self, key: String, value: ValueEntry) -> Result<(), EngineError>;
    fn del(&self, key: &str) -> Result<bool, EngineError>;
    fn incr(&self, key: &str) -> Result<i64, EngineError>;
    fn len(&self) -> usize;

    fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Returns true if key exists and is not expired.
    fn exists(&self, key: &str) -> bool;

    /// Returns remaining TTL in seconds:
    /// -2 = key does not exist, -1 = no TTL, >=0 = remaining seconds
    fn ttl_secs(&self, key: &str) -> i64;

    /// Set expiry on an existing key. Returns false if key not found.
    fn expire(&self, key: &str, seconds: u64) -> bool;

    /// Remove all keys from the database.
    fn clear(&self);

    /// Return all non-expired key names.
    fn keys(&self) -> Vec<String>;

    /// Get multiple values at once. Returns a Vec where each element is
    /// Some(ValueEntry) if the key exists and is not expired, or None.
    fn mget(&self, keys: &[&str]) -> Vec<Option<ValueEntry>>;
}
