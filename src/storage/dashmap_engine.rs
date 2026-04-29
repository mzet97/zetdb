use bytes::Bytes;
use dashmap::mapref::entry::Entry;
#[cfg(test)]
use std::sync::Arc;
use std::time::Instant;

use crate::domain::errors::EngineError;
use crate::domain::value::ValueEntry;
use crate::storage::engine::KvEngine;

pub struct DashMapEngine {
    map: dashmap::DashMap<String, ValueEntry>,
}

impl DashMapEngine {
    pub fn new() -> Self {
        Self {
            map: dashmap::DashMap::new(),
        }
    }

    pub fn sweep_expired(&self) {
        self.map.retain(|_, v| !v.is_expired());
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
        }))
    }

    fn set(&self, key: String, value: ValueEntry) -> Result<(), EngineError> {
        self.map.insert(key, value);
        Ok(())
    }

    fn del(&self, key: &str) -> Result<bool, EngineError> {
        let removed = self.map.remove_if(key, |_, v| !v.is_expired());
        Ok(removed.is_some())
    }

    fn incr(&self, key: &str) -> Result<i64, EngineError> {
        match self.map.entry(key.to_string()) {
            Entry::Occupied(mut occ) => {
                if occ.get().is_expired() {
                    occ.remove();
                    self.map
                        .insert(key.to_string(), ValueEntry::new(Bytes::from_static(b"1")));
                    return Ok(1);
                }

                let val: i64 = String::from_utf8_lossy(&occ.get().data)
                    .parse()
                    .map_err(|_| EngineError::NotAnInteger(key.to_string()))?;

                let new_val = val + 1;
                let mut itoa_buf = itoa::Buffer::new();
                occ.get_mut().data = Bytes::copy_from_slice(itoa_buf.format(new_val).as_bytes());
                Ok(new_val)
            }
            Entry::Vacant(vac) => {
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
            None => -1,
            Some(exp) => {
                let remaining = (exp - Instant::now()).as_secs() as i64;
                if remaining > 0 {
                    remaining
                } else {
                    -2
                }
            }
        }
    }

    fn expire(&self, key: &str, seconds: u64) -> bool {
        let Some(mut entry) = self.map.get_mut(key) else {
            return false;
        };
        if entry.is_expired() {
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
        keys.iter()
            .map(|key| self.get(key).ok().flatten())
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
            .set("k".into(), ValueEntry::new(Bytes::from("v")))
            .unwrap();

        let entry = engine.get("k").unwrap().unwrap();
        assert_eq!(entry.data, Bytes::from("v"));
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
            .set("k".into(), ValueEntry::new(Bytes::from("v1")))
            .unwrap();
        engine
            .set("k".into(), ValueEntry::new(Bytes::from("v2")))
            .unwrap();

        let entry = engine.get("k").unwrap().unwrap();
        assert_eq!(entry.data, Bytes::from("v2"));
    }

    #[test]
    fn del_existing() {
        let engine = DashMapEngine::new();
        engine
            .set("k".into(), ValueEntry::new(Bytes::from("v")))
            .unwrap();
        assert!(engine.del("k").unwrap());
        assert!(engine.get("k").unwrap().is_none());
    }

    #[test]
    fn del_missing() {
        let engine = DashMapEngine::new();
        assert!(!engine.del("missing").unwrap());
    }

    #[test]
    fn del_expired_returns_false() {
        let engine = DashMapEngine::new();
        engine
            .set(
                "k".into(),
                ValueEntry::with_ttl(Bytes::from("v"), Duration::from_millis(1)),
            )
            .unwrap();
        std::thread::sleep(Duration::from_millis(5));
        assert!(!engine.del("k").unwrap());
    }

    #[test]
    fn incr_new_key() {
        let engine = DashMapEngine::new();
        assert_eq!(engine.incr("c").unwrap(), 1);
        assert_eq!(engine.incr("c").unwrap(), 2);
        assert_eq!(engine.incr("c").unwrap(), 3);
    }

    #[test]
    fn incr_existing_integer() {
        let engine = DashMapEngine::new();
        engine
            .set("c".into(), ValueEntry::new(Bytes::from("10")))
            .unwrap();
        assert_eq!(engine.incr("c").unwrap(), 11);
    }

    #[test]
    fn incr_non_integer() {
        let engine = DashMapEngine::new();
        engine
            .set("c".into(), ValueEntry::new(Bytes::from("hello")))
            .unwrap();
        let err = engine.incr("c").unwrap_err();
        assert!(matches!(err, EngineError::NotAnInteger(_)));
    }

    #[test]
    fn incr_expired_key_resets() {
        let engine = DashMapEngine::new();
        engine
            .set(
                "c".into(),
                ValueEntry::with_ttl(Bytes::from("99"), Duration::from_millis(1)),
            )
            .unwrap();
        std::thread::sleep(Duration::from_millis(5));
        assert_eq!(engine.incr("c").unwrap(), 1);
    }

    #[test]
    fn incr_preserves_ttl() {
        let engine = DashMapEngine::new();
        engine
            .set(
                "c".into(),
                ValueEntry::with_ttl(Bytes::from("5"), Duration::from_secs(60)),
            )
            .unwrap();
        let before = engine.get("c").unwrap().unwrap();
        assert!(before.expires_at.is_some());

        engine.incr("c").unwrap();
        let after = engine.get("c").unwrap().unwrap();
        assert_eq!(after.expires_at, before.expires_at);
    }

    #[test]
    fn lazy_eviction_on_get() {
        let engine = DashMapEngine::new();
        engine
            .set(
                "k".into(),
                ValueEntry::with_ttl(Bytes::from("v"), Duration::from_millis(1)),
            )
            .unwrap();
        std::thread::sleep(Duration::from_millis(5));
        assert!(engine.get("k").unwrap().is_none());
    }

    #[test]
    fn sweep_expired() {
        let engine = DashMapEngine::new();
        engine
            .set("a".into(), ValueEntry::new(Bytes::from("1")))
            .unwrap();
        engine
            .set(
                "b".into(),
                ValueEntry::with_ttl(Bytes::from("2"), Duration::from_millis(1)),
            )
            .unwrap();
        engine
            .set("c".into(), ValueEntry::new(Bytes::from("3")))
            .unwrap();
        engine
            .set(
                "d".into(),
                ValueEntry::with_ttl(Bytes::from("4"), Duration::from_millis(1)),
            )
            .unwrap();

        std::thread::sleep(Duration::from_millis(5));
        engine.sweep_expired();

        assert!(engine.get("a").unwrap().is_some());
        assert!(engine.get("b").unwrap().is_none());
        assert!(engine.get("c").unwrap().is_some());
        assert!(engine.get("d").unwrap().is_none());
    }

    #[test]
    fn concurrent_incr() {
        let engine = Arc::new(DashMapEngine::new());
        let num_threads = 100;
        let increments_per_thread = 100;

        let handles: Vec<_> = (0..num_threads)
            .map(|_| {
                let engine = engine.clone();
                std::thread::spawn(move || {
                    for _ in 0..increments_per_thread {
                        engine.incr("counter").unwrap();
                    }
                })
            })
            .collect();

        for handle in handles {
            handle.join().unwrap();
        }

        let entry = engine.get("counter").unwrap().unwrap();
        let expected = num_threads * increments_per_thread;
        assert_eq!(
            entry.data,
            Bytes::from(expected.to_string()),
            "Expected {} but got {:?}",
            expected,
            String::from_utf8_lossy(&entry.data)
        );
    }

    #[test]
    fn binary_value() {
        let engine = DashMapEngine::new();
        let binary: Vec<u8> = vec![0x00, 0xFF, 0xDE, 0xAD, 0xBE, 0xEF];
        engine
            .set("bin".into(), ValueEntry::new(Bytes::from(binary.clone())))
            .unwrap();

        let entry = engine.get("bin").unwrap().unwrap();
        assert_eq!(entry.data.as_ref(), binary.as_slice());
    }

    // --- EXISTS, TTL, EXPIRE, FLUSHDB, KEYS tests ---

    #[test]
    fn exists_key_present() {
        let engine = DashMapEngine::new();
        engine
            .set("k".into(), ValueEntry::new(Bytes::from("v")))
            .unwrap();
        assert!(engine.exists("k"));
    }

    #[test]
    fn exists_key_missing() {
        let engine = DashMapEngine::new();
        assert!(!engine.exists("missing"));
    }

    #[test]
    fn exists_key_expired() {
        let engine = DashMapEngine::new();
        engine
            .set(
                "k".into(),
                ValueEntry::with_ttl(Bytes::from("v"), Duration::from_millis(1)),
            )
            .unwrap();
        std::thread::sleep(Duration::from_millis(5));
        assert!(!engine.exists("k"));
    }

    #[test]
    fn ttl_no_expiry() {
        let engine = DashMapEngine::new();
        engine
            .set("k".into(), ValueEntry::new(Bytes::from("v")))
            .unwrap();
        assert_eq!(engine.ttl_secs("k"), -1);
    }

    #[test]
    fn ttl_key_missing() {
        let engine = DashMapEngine::new();
        assert_eq!(engine.ttl_secs("missing"), -2);
    }

    #[test]
    fn ttl_with_expiry() {
        let engine = DashMapEngine::new();
        engine
            .set(
                "k".into(),
                ValueEntry::with_ttl(Bytes::from("v"), Duration::from_secs(60)),
            )
            .unwrap();
        let ttl = engine.ttl_secs("k");
        assert!(ttl > 55 && ttl <= 60, "TTL should be ~60, got {ttl}");
    }

    #[test]
    fn expire_existing_key() {
        let engine = DashMapEngine::new();
        engine
            .set("k".into(), ValueEntry::new(Bytes::from("v")))
            .unwrap();
        assert!(engine.expire("k", 10));
        let ttl = engine.ttl_secs("k");
        assert!(
            (0..=10).contains(&ttl),
            "TTL should be 0..10 after EXPIRE, got {ttl}"
        );
    }

    #[test]
    fn expire_missing_key() {
        let engine = DashMapEngine::new();
        assert!(!engine.expire("missing", 10));
    }

    #[test]
    fn clear_removes_all() {
        let engine = DashMapEngine::new();
        engine
            .set("a".into(), ValueEntry::new(Bytes::from("1")))
            .unwrap();
        engine
            .set("b".into(), ValueEntry::new(Bytes::from("2")))
            .unwrap();
        engine.clear();
        assert_eq!(engine.len(), 0);
        assert!(engine.get("a").unwrap().is_none());
    }

    #[test]
    fn keys_returns_non_expired() {
        let engine = DashMapEngine::new();
        engine
            .set("a".into(), ValueEntry::new(Bytes::from("1")))
            .unwrap();
        engine
            .set(
                "b".into(),
                ValueEntry::with_ttl(Bytes::from("2"), Duration::from_millis(1)),
            )
            .unwrap();
        engine
            .set("c".into(), ValueEntry::new(Bytes::from("3")))
            .unwrap();
        std::thread::sleep(Duration::from_millis(5));
        let mut keys = engine.keys();
        keys.sort();
        assert_eq!(keys, vec!["a".to_string(), "c".to_string()]);
    }

    #[test]
    fn mget_multiple_keys() {
        let engine = DashMapEngine::new();
        engine
            .set("a".into(), ValueEntry::new(Bytes::from("1")))
            .unwrap();
        engine
            .set("b".into(), ValueEntry::new(Bytes::from("2")))
            .unwrap();

        let results = engine.mget(&["a", "missing", "b"]);
        assert_eq!(results.len(), 3);
        assert_eq!(results[0].as_ref().unwrap().data, Bytes::from("1"));
        assert!(results[1].is_none());
        assert_eq!(results[2].as_ref().unwrap().data, Bytes::from("2"));
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
        let results = engine.mget(&["x", "y"]);
        assert_eq!(results.len(), 2);
        assert!(results[0].is_none());
        assert!(results[1].is_none());
    }
}
