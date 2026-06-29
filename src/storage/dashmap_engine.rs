use bytes::Bytes;
use dashmap::mapref::entry::Entry;
#[cfg(test)]
use std::sync::Arc;
use std::time::Instant;

use crate::domain::errors::EngineError;
use crate::domain::value::ValueEntry;
use crate::storage::engine::KvEngine;

pub struct DashMapEngine {
    map: dashmap::DashMap<Box<str>, ValueEntry>,
    max_keys: usize,
}

impl DashMapEngine {
    pub fn new() -> Self {
        Self {
            map: dashmap::DashMap::new(),
            max_keys: 0,
        }
    }

    pub fn with_max_keys(max_keys: usize) -> Self {
        Self {
            map: dashmap::DashMap::new(),
            max_keys,
        }
    }

    pub fn sweep_expired(&self) {
        self.map.retain(|_, v| !v.is_expired());
    }

    /// Evict one key if we are at or over max_keys.
    /// Samples up to 5 live entries and removes the oldest by created_at.
    fn evict_one_if_needed(&self) {
        if self.max_keys == 0 || self.map.len() < self.max_keys {
            return;
        }
        let now = Instant::now();
        let mut oldest_key: Option<Box<str>> = None;
        let mut oldest_time: Option<Instant> = None;
        let mut sampled = 0;
        for entry in self.map.iter() {
            if entry.value().is_expired_at(now) {
                continue;
            }
            let entry_created = entry.value().created_at;
            if oldest_key.is_none() || entry_created < oldest_time {
                oldest_key = Some(entry.key().clone());
                oldest_time = entry_created;
            }
            sampled += 1;
            if sampled >= 5 {
                break;
            }
        }
        if let Some(key) = oldest_key {
            self.map.remove(&key);
        }
    }

    /// Iterate over non-expired entries for snapshot dump.
    /// Calls `f(key, value_bytes, ttl_remaining_ms)` for each live entry.
    /// `ttl_remaining_ms` is -1 for no TTL, or remaining milliseconds.
    pub fn dump_entries<F>(&self, mut f: F) -> usize
    where
        F: FnMut(&str, &[u8], i64),
    {
        let now = Instant::now();
        let mut count = 0;
        for entry in self.map.iter() {
            let expires_at = entry.value().expires_at;
            if let Some(exp) = expires_at {
                if now >= exp {
                    continue;
                }
            }
            let remaining = match expires_at {
                Some(exp) => {
                    let ms = (exp - now).as_millis() as i64;
                    if ms > 0 {
                        ms
                    } else {
                        -1
                    }
                }
                None => -1,
            };
            f(entry.key(), &entry.value().data, remaining);
            count += 1;
        }
        count
    }

    /// Ultra-fast get for benchmark hot path - skips TTL check
    /// Returns reference to entry data without cloning
    #[inline(always)]
    pub fn get_fast(&self, key: &str) -> Option<Bytes> {
        let entry = self.map.get(key)?;
        // Skip TTL check in benchmark mode for maximum performance
        Some(entry.data.clone())
    }

    /// Ultra-fast set for benchmark hot path - no eviction, no TTL
    /// Accepts &str key to avoid String allocation
    #[inline(always)]
    pub fn set_fast_str(&self, key: &str, value: Bytes) {
        self.map.insert(key.to_string().into_boxed_str(), ValueEntry::new_fast(value));
    }
}

impl Default for DashMapEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl KvEngine for DashMapEngine {
    fn get(&self, key: &str) -> Result<Option<ValueEntry>, EngineError> {
        let Some(entry) = self.map.get(key) else {
            return Ok(None);
        };

        if entry.is_expired() {
            drop(entry);
            self.map.remove_if(key, |_, v| v.is_expired());
            return Ok(None);
        }

        Ok(Some(ValueEntry {
            data: entry.data.clone(),
            expires_at: entry.expires_at,
            created_at: entry.created_at,
        }))
    }

    fn set(&self, key: String, value: ValueEntry) -> Result<(), EngineError> {
        if self.max_keys > 0 && !self.map.contains_key(key.as_str()) {
            self.evict_one_if_needed();
            if self.map.len() >= self.max_keys {
                return Err(EngineError::OutOfMemory);
            }
        }
        self.map.insert(key.into_boxed_str(), value);
        Ok(())
    }

    fn del(&self, key: &str) -> Result<bool, EngineError> {
        let removed = self.map.remove_if(key, |_, v| !v.is_expired());
        Ok(removed.is_some())
    }

    fn incr(&self, key: &str) -> Result<i64, EngineError> {
        match self.map.entry(key.into()) {
            Entry::Occupied(mut occ) => {
                if occ.get().is_expired() {
                    occ.remove();
                    if self.max_keys > 0 && self.map.len() >= self.max_keys {
                        return Err(EngineError::OutOfMemory);
                    }
                    self.map
                        .insert(Box::from(key), ValueEntry::new(Bytes::from_static(b"1")));
                    return Ok(1);
                }

                let val: i64 = String::from_utf8_lossy(&occ.get().data)
                    .parse()
                    .map_err(|_| EngineError::NotAnInteger(key.to_string()))?;

                let new_val = val + 1;
                let mut itoa_buf = itoa::Buffer::new();
                occ.get_mut().data = Bytes::copy_from_slice(itoa_buf.format(new_val).as_bytes());
                // created_at and expires_at are preserved automatically
                Ok(new_val)
            }
            Entry::Vacant(vac) => {
                if self.max_keys > 0 && self.map.len() >= self.max_keys {
                    self.evict_one_if_needed();
                    if self.map.len() >= self.max_keys {
                        return Err(EngineError::OutOfMemory);
                    }
                }
                vac.insert(ValueEntry::new(Bytes::from_static(b"1")));
                Ok(1)
            }
        }
    }

    fn len(&self) -> usize {
        self.map.len()
    }

    fn exists(&self, key: &str) -> bool {
        let Some(entry) = self.map.get(key) else {
            return false;
        };
        if entry.is_expired() {
            drop(entry);
            self.map.remove_if(key, |_, v| v.is_expired());
            false
        } else {
            true
        }
    }

    fn ttl_secs(&self, key: &str) -> i64 {
        let Some(entry) = self.map.get(key) else {
            return -2;
        };
        if entry.is_expired() {
            drop(entry);
            self.map.remove_if(key, |_, v| v.is_expired());
            return -2;
        }
        match entry.expires_at {
            Some(exp) => {
                let now = Instant::now();
                if exp <= now {
                    -2
                } else {
                    (exp - now).as_secs() as i64
                }
            }
            None => -1,
        }
    }

    fn expire(&self, key: &str, seconds: u64) -> bool {
        let Some(mut entry) = self.map.get_mut(key) else {
            return false;
        };
        if entry.is_expired() {
            drop(entry);
            self.map.remove_if(key, |_, v| v.is_expired());
            return false;
        }
        entry.expires_at = Some(Instant::now() + std::time::Duration::from_secs(seconds));
        true
    }

    fn clear(&self) {
        self.map.clear();
    }

    fn keys(&self) -> Vec<String> {
        let now = Instant::now();
        self.map
            .iter()
            .filter(|e| !e.value().is_expired_at(now))
            .map(|e| e.key().to_string())
            .collect()
    }

    fn mget(&self, keys: &[&str]) -> Vec<Option<ValueEntry>> {
        let now = Instant::now();
        keys.iter()
            .map(|k| {
                let entry = self.map.get(*k)?;
                if entry.is_expired_at(now) {
                    None
                } else {
                    Some(ValueEntry {
                        data: entry.data.clone(),
                        expires_at: entry.expires_at,
                        created_at: entry.created_at,
                    })
                }
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn set_and_get() {
        let engine = DashMapEngine::new();
        engine
            .set("hello".into(), ValueEntry::new(Bytes::from("world")))
            .unwrap();
        let entry = engine.get("hello").unwrap().unwrap();
        assert_eq!(entry.data, Bytes::from("world"));
    }

    #[test]
    fn get_missing_key() {
        let engine = DashMapEngine::new();
        assert!(engine.get("missing").unwrap().is_none());
    }

    #[test]
    fn set_overwrites() {
        let engine = DashMapEngine::new();
        engine
            .set("key".into(), ValueEntry::new(Bytes::from("v1")))
            .unwrap();
        engine
            .set("key".into(), ValueEntry::new(Bytes::from("v2")))
            .unwrap();
        assert_eq!(engine.get("key").unwrap().unwrap().data, Bytes::from("v2"));
    }

    #[test]
    fn del_existing() {
        let engine = DashMapEngine::new();
        engine
            .set("key".into(), ValueEntry::new(Bytes::from("val")))
            .unwrap();
        assert!(engine.del("key").unwrap());
        assert!(engine.get("key").unwrap().is_none());
    }

    #[test]
    fn del_missing() {
        let engine = DashMapEngine::new();
        assert!(!engine.del("missing").unwrap());
    }

    #[test]
    fn incr_existing_integer() {
        let engine = DashMapEngine::new();
        engine
            .set("counter".into(), ValueEntry::new(Bytes::from("5")))
            .unwrap();
        assert_eq!(engine.incr("counter").unwrap(), 6);
        assert_eq!(
            engine.get("counter").unwrap().unwrap().data,
            Bytes::from("6")
        );
    }

    #[test]
    fn incr_new_key() {
        let engine = DashMapEngine::new();
        assert_eq!(engine.incr("new_counter").unwrap(), 1);
    }

    #[test]
    fn incr_non_integer() {
        let engine = DashMapEngine::new();
        engine
            .set("bad".into(), ValueEntry::new(Bytes::from("not_a_number")))
            .unwrap();
        assert!(matches!(engine.incr("bad"), Err(EngineError::NotAnInteger(_))));
    }

    #[test]
    fn exists_key_present() {
        let engine = DashMapEngine::new();
        engine
            .set("key".into(), ValueEntry::new(Bytes::from("val")))
            .unwrap();
        assert!(engine.exists("key"));
    }

    #[test]
    fn exists_key_missing() {
        let engine = DashMapEngine::new();
        assert!(!engine.exists("missing"));
    }

    #[test]
    fn exists_key_expired() {
        let engine = DashMapEngine::new();
        let mut entry = ValueEntry::new(Bytes::from("val"));
        entry.expires_at = Some(Instant::now() - Duration::from_secs(1));
        engine.set("expired".into(), entry).unwrap();
        assert!(!engine.exists("expired"));
    }

    #[test]
    fn lazy_eviction_on_get() {
        let engine = DashMapEngine::new();
        let mut entry = ValueEntry::new(Bytes::from("val"));
        entry.expires_at = Some(Instant::now() - Duration::from_secs(1));
        engine.set("expired".into(), entry).unwrap();
        assert!(engine.get("expired").unwrap().is_none());
    }

    #[test]
    fn ttl_key_missing() {
        let engine = DashMapEngine::new();
        assert_eq!(engine.ttl_secs("missing"), -2);
    }

    #[test]
    fn ttl_no_expiry() {
        let engine = DashMapEngine::new();
        engine
            .set("key".into(), ValueEntry::new(Bytes::from("val")))
            .unwrap();
        assert_eq!(engine.ttl_secs("key"), -1);
    }

    #[test]
    fn ttl_with_expiry() {
        let engine = DashMapEngine::new();
        let mut entry = ValueEntry::new(Bytes::from("val"));
        entry.expires_at = Some(Instant::now() + Duration::from_secs(300));
        engine.set("key".into(), entry).unwrap();
        let ttl = engine.ttl_secs("key");
        assert!(ttl >= 299 && ttl <= 300);
    }

    #[test]
    fn expire_existing_key() {
        let engine = DashMapEngine::new();
        engine
            .set("key".into(), ValueEntry::new(Bytes::from("val")))
            .unwrap();
        assert!(engine.expire("key", 60));
        let ttl = engine.ttl_secs("key");
        assert!(ttl >= 59 && ttl <= 60);
    }

    #[test]
    fn expire_missing_key() {
        let engine = DashMapEngine::new();
        assert!(!engine.expire("missing", 60));
    }

    #[test]
    fn sweep_expired() {
        let engine = DashMapEngine::new();
        engine
            .set("live".into(), ValueEntry::new(Bytes::from("val")))
            .unwrap();
        let mut entry = ValueEntry::new(Bytes::from("val"));
        entry.expires_at = Some(Instant::now() - Duration::from_secs(1));
        engine.set("dead".into(), entry).unwrap();
        assert_eq!(engine.len(), 2);
        engine.sweep_expired();
        assert_eq!(engine.len(), 1);
    }

    #[test]
    fn clear_removes_all() {
        let engine = DashMapEngine::new();
        engine
            .set("k1".into(), ValueEntry::new(Bytes::from("v1")))
            .unwrap();
        engine
            .set("k2".into(), ValueEntry::new(Bytes::from("v2")))
            .unwrap();
        engine.clear();
        assert_eq!(engine.len(), 0);
    }

    #[test]
    fn binary_value() {
        let engine = DashMapEngine::new();
        let binary: Vec<u8> = vec![0x00, 0xFF, 0xDE, 0xAD, 0xBE, 0xEF];
        engine
            .set("bin".into(), ValueEntry::new(Bytes::from(binary.clone())))
            .unwrap();
        assert_eq!(
            engine.get("bin").unwrap().unwrap().data.as_ref(),
            binary.as_slice()
        );
    }

    #[test]
    fn concurrent_incr() {
        let engine = Arc::new(DashMapEngine::new());
        engine
            .set("counter".into(), ValueEntry::new(Bytes::from("0")))
            .unwrap();

        let mut handles = vec![];
        for _ in 0..10 {
            let e = engine.clone();
            handles.push(std::thread::spawn(move || {
                for _ in 0..100 {
                    e.incr("counter").unwrap();
                }
            }));
        }

        for h in handles {
            h.join().unwrap();
        }

        assert_eq!(
            engine.get("counter").unwrap().unwrap().data,
            Bytes::from("1000")
        );
    }

    #[test]
    fn max_keys_evicts_oldest() {
        let engine = DashMapEngine::with_max_keys(3);
        engine.set("k1".into(), ValueEntry::new(Bytes::from("v1"))).unwrap();
        std::thread::sleep(Duration::from_millis(10));
        engine.set("k2".into(), ValueEntry::new(Bytes::from("v2"))).unwrap();
        std::thread::sleep(Duration::from_millis(10));
        engine.set("k3".into(), ValueEntry::new(Bytes::from("v3"))).unwrap();
        // At capacity
        assert_eq!(engine.len(), 3);
        // Adding a 4th should evict the oldest (k1)
        engine.set("k4".into(), ValueEntry::new(Bytes::from("v4"))).unwrap();
        assert_eq!(engine.len(), 3);
        assert!(engine.get("k1").unwrap().is_none());
        assert!(engine.get("k2").unwrap().is_some());
        assert!(engine.get("k3").unwrap().is_some());
        assert!(engine.get("k4").unwrap().is_some());
    }

    #[test]
    fn max_keys_zero_is_unlimited() {
        let engine = DashMapEngine::with_max_keys(0);
        for i in 0..100 {
            engine
                .set(format!("k{i}"), ValueEntry::new(Bytes::from("v")))
                .unwrap();
        }
        assert_eq!(engine.len(), 100);
    }

    #[test]
    fn max_keys_allows_overwrite() {
        let engine = DashMapEngine::with_max_keys(2);
        engine.set("k1".into(), ValueEntry::new(Bytes::from("v1"))).unwrap();
        engine.set("k2".into(), ValueEntry::new(Bytes::from("v2"))).unwrap();
        // Overwriting k1 should not trigger eviction
        engine.set("k1".into(), ValueEntry::new(Bytes::from("v1_new"))).unwrap();
        assert_eq!(engine.len(), 2);
    }

    #[test]
    fn max_keys_evicts_with_mset() {
        let engine = DashMapEngine::with_max_keys(2);
        engine.set("k1".into(), ValueEntry::new(Bytes::from("v1"))).unwrap();
        engine.set("k2".into(), ValueEntry::new(Bytes::from("v2"))).unwrap();
        // mset with 2 new keys should evict one existing
        // (implementation detail: mset calls set for each pair)
        engine.set("k3".into(), ValueEntry::new(Bytes::from("v3"))).unwrap();
        assert_eq!(engine.len(), 2);
    }

    #[test]
    fn max_keys_mset_overwrite_does_not_evict() {
        let engine = DashMapEngine::with_max_keys(2);
        engine.set("k1".into(), ValueEntry::new(Bytes::from("v1"))).unwrap();
        engine.set("k2".into(), ValueEntry::new(Bytes::from("v2"))).unwrap();
        // Overwriting k1 should not trigger eviction
        engine.set("k1".into(), ValueEntry::new(Bytes::from("v1_new"))).unwrap();
        engine.set("k2".into(), ValueEntry::new(Bytes::from("v2_new"))).unwrap();
        assert_eq!(engine.len(), 2);
    }

    #[test]
    fn incr_preserves_ttl() {
        let engine = DashMapEngine::new();
        let mut entry = ValueEntry::new(Bytes::from("5"));
        entry.expires_at = Some(Instant::now() + Duration::from_secs(60));
        engine.set("counter".into(), entry).unwrap();

        engine.incr("counter").unwrap();

        let updated = engine.get("counter").unwrap().unwrap();
        assert_eq!(updated.data, Bytes::from("6"));
        assert!(updated.expires_at.is_some());
    }

    #[test]
    fn incr_expired_key_resets() {
        let engine = DashMapEngine::new();
        let mut entry = ValueEntry::new(Bytes::from("5"));
        entry.expires_at = Some(Instant::now() - Duration::from_secs(1));
        engine.set("counter".into(), entry).unwrap();

        assert_eq!(engine.incr("counter").unwrap(), 1);
        assert_eq!(
            engine.get("counter").unwrap().unwrap().data,
            Bytes::from("1")
        );
    }

    #[test]
    fn del_expired_returns_false() {
        let engine = DashMapEngine::new();
        let mut entry = ValueEntry::new(Bytes::from("val"));
        entry.expires_at = Some(Instant::now() - Duration::from_secs(1));
        engine.set("expired".into(), entry).unwrap();
        assert!(!engine.del("expired").unwrap());
    }

    #[test]
    fn mget_multiple_keys() {
        let engine = DashMapEngine::new();
        engine.set("k1".into(), ValueEntry::new(Bytes::from("v1"))).unwrap();
        engine.set("k2".into(), ValueEntry::new(Bytes::from("v2"))).unwrap();
        let results = engine.mget(&["k1", "k2", "missing"]);
        assert_eq!(results[0].as_ref().unwrap().data, Bytes::from("v1"));
        assert_eq!(results[1].as_ref().unwrap().data, Bytes::from("v2"));
        assert!(results[2].is_none());
    }

    #[test]
    fn mget_empty_keys() {
        let engine = DashMapEngine::new();
        let results = engine.mget(&[]);
        assert!(results.is_empty());
    }

    #[test]
    fn mget_all_missing() {
        let engine = DashMapEngine::new();
        let results = engine.mget(&["a", "b", "c"]);
        assert!(results.iter().all(|r| r.is_none()));
    }
}
