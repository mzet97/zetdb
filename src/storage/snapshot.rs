use std::fs;
use std::io::{self, Write};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use bytes::Bytes;

use crate::domain::value::ValueEntry;
use crate::storage::dashmap_engine::DashMapEngine;
use crate::storage::engine::KvEngine;

const MAGIC: &[u8; 4] = b"ZDB1";
const VERSION: u8 = 1;
const HEADER_SIZE: usize = 4 + 1 + 1 + 4 + 8; // magic + version + flags + count + timestamp
const CRC_SIZE: usize = 4;

/// Dump all non-expired entries to a snapshot file.
/// Uses atomic write (temp file + rename) for consistency.
pub fn dump_snapshot(engine: &DashMapEngine, path: &str) -> Result<usize, io::Error> {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;

    let mut buf = Vec::with_capacity(64 * 1024);

    // Header
    buf.extend_from_slice(MAGIC);
    buf.push(VERSION);
    buf.push(0); // flags (reserved)
    buf.extend_from_slice(&0u32.to_le_bytes()); // count placeholder
    buf.extend_from_slice(&timestamp.to_le_bytes());

    // Entries
    let count = engine.dump_entries(|key, value, ttl_ms| {
        let key_bytes = key.as_bytes();
        buf.extend_from_slice(&(key_bytes.len() as u16).to_le_bytes());
        buf.extend_from_slice(key_bytes);
        buf.extend_from_slice(&(value.len() as u32).to_le_bytes());
        buf.extend_from_slice(value);
        buf.extend_from_slice(&ttl_ms.to_le_bytes());
    });

    // Fill count in header
    buf[6..10].copy_from_slice(&(count as u32).to_le_bytes());

    // CRC32 of everything so far
    let crc = crc32fast::hash(&buf);
    buf.extend_from_slice(&crc.to_le_bytes());

    // Atomic write: temp file + fsync + rename
    let tmp_path = format!("{path}.tmp");
    {
        let mut file = fs::File::create(&tmp_path)?;
        file.write_all(&buf)?;
        file.sync_all()?;
    }
    fs::rename(&tmp_path, path)?;

    Ok(count)
}

/// Load entries from a snapshot file into the engine.
/// Returns Ok(0) if file doesn't exist. Returns Err on corruption.
pub fn load_snapshot(engine: &DashMapEngine, path: &str) -> Result<usize, io::Error> {
    let data = match fs::read(path) {
        Ok(d) => d,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(0),
        Err(e) => return Err(e),
    };

    if data.len() < HEADER_SIZE + CRC_SIZE {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "snapshot too short"));
    }

    // Verify magic
    if &data[0..4] != MAGIC {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "invalid snapshot magic"));
    }

    // Verify version
    if data[4] != VERSION {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("unsupported snapshot version: {}", data[4]),
        ));
    }

    // Verify CRC
    let payload_end = data.len() - CRC_SIZE;
    let stored_crc = u32::from_le_bytes(data[payload_end..].try_into().unwrap());
    let computed_crc = crc32fast::hash(&data[..payload_end]);
    if stored_crc != computed_crc {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "snapshot CRC mismatch"));
    }

    // Parse header
    let count = u32::from_le_bytes(data[6..10].try_into().unwrap()) as usize;

    // Parse entries
    let mut pos = HEADER_SIZE;
    let mut loaded = 0;

    for _ in 0..count {
        // Key
        if pos + 2 > payload_end {
            break;
        }
        let key_len = u16::from_le_bytes(data[pos..pos + 2].try_into().unwrap()) as usize;
        pos += 2;

        if pos + key_len > payload_end {
            break;
        }
        let key = String::from_utf8_lossy(&data[pos..pos + key_len]).into_owned();
        pos += key_len;

        // Value
        if pos + 4 > payload_end {
            break;
        }
        let val_len = u32::from_le_bytes(data[pos..pos + 4].try_into().unwrap()) as usize;
        pos += 4;

        if pos + val_len > payload_end {
            break;
        }
        let value = Bytes::copy_from_slice(&data[pos..pos + val_len]);
        pos += val_len;

        // TTL
        if pos + 8 > payload_end {
            break;
        }
        let ttl_ms = i64::from_le_bytes(data[pos..pos + 8].try_into().unwrap());
        pos += 8;

        let expires_at = if ttl_ms > 0 {
            Some(Instant::now() + Duration::from_millis(ttl_ms as u64))
        } else {
            None
        };

        engine
            .set(key, ValueEntry { data: value, expires_at })
            .ok();
        loaded += 1;
    }

    Ok(loaded)
}

/// Background task that periodically dumps snapshots.
pub async fn run_snapshotter(
    engine: std::sync::Arc<DashMapEngine>,
    path: String,
    interval: Duration,
) {
    let mut ticker = tokio::time::interval(interval);
    loop {
        ticker.tick().await;
        match dump_snapshot(engine.as_ref(), &path) {
            Ok(count) => log::info!("snapshot saved: {count} entries"),
            Err(e) => log::error!("snapshot failed: {e}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::engine::KvEngine;
    use std::path::Path;
    use std::time::Duration;

    fn temp_path(name: &str) -> String {
        format!("target/test_snapshots/{name}.zdb")
    }

    fn ensure_dir(path: &str) {
        let dir = Path::new(path).parent().unwrap();
        let _ = fs::create_dir_all(dir);
    }

    #[test]
    fn dump_and_load_roundtrip() {
        let path = temp_path("roundtrip");
        ensure_dir(&path);
        let _ = fs::remove_file(&path);

        let engine = DashMapEngine::new();
        engine.set("k1".into(), ValueEntry::new(Bytes::from("v1"))).unwrap();
        engine.set("k2".into(), ValueEntry::new(Bytes::from("v2"))).unwrap();
        engine.set("k3".into(), ValueEntry::new(Bytes::from("hello world"))).unwrap();

        let count = dump_snapshot(&engine, &path).unwrap();
        assert_eq!(count, 3);

        let engine2 = DashMapEngine::new();
        let loaded = load_snapshot(&engine2, &path).unwrap();
        assert_eq!(loaded, 3);

        assert_eq!(engine2.get("k1").unwrap().unwrap().data, Bytes::from("v1"));
        assert_eq!(engine2.get("k2").unwrap().unwrap().data, Bytes::from("v2"));
        assert_eq!(engine2.get("k3").unwrap().unwrap().data, Bytes::from("hello world"));
    }

    #[test]
    fn ttl_preserved() {
        let path = temp_path("ttl");
        ensure_dir(&path);
        let _ = fs::remove_file(&path);

        let engine = DashMapEngine::new();
        engine
            .set("ttl_key".into(), ValueEntry::with_ttl(Bytes::from("val"), Duration::from_secs(300)))
            .unwrap();

        dump_snapshot(&engine, &path).unwrap();

        let engine2 = DashMapEngine::new();
        load_snapshot(&engine2, &path).unwrap();

        let entry = engine2.get("ttl_key").unwrap().unwrap();
        assert_eq!(entry.data, Bytes::from("val"));
        assert!(entry.expires_at.is_some());
    }

    #[test]
    fn expired_entries_skipped() {
        let path = temp_path("expired");
        ensure_dir(&path);
        let _ = fs::remove_file(&path);

        let engine = DashMapEngine::new();
        engine.set("live".into(), ValueEntry::new(Bytes::from("yes"))).unwrap();
        engine
            .set("dead".into(), ValueEntry::with_ttl(Bytes::from("no"), Duration::from_millis(1)))
            .unwrap();

        std::thread::sleep(Duration::from_millis(5));

        let count = dump_snapshot(&engine, &path).unwrap();
        assert_eq!(count, 1); // only "live"

        let engine2 = DashMapEngine::new();
        load_snapshot(&engine2, &path).unwrap();

        assert_eq!(engine2.get("live").unwrap().unwrap().data, Bytes::from("yes"));
        assert!(engine2.get("dead").unwrap().is_none());
    }

    #[test]
    fn crc_detects_corruption() {
        let path = temp_path("corrupt");
        ensure_dir(&path);
        let _ = fs::remove_file(&path);

        let engine = DashMapEngine::new();
        engine.set("k".into(), ValueEntry::new(Bytes::from("v"))).unwrap();
        dump_snapshot(&engine, &path).unwrap();

        // Corrupt a byte in the middle
        let mut data = fs::read(&path).unwrap();
        let mid = data.len() / 2;
        data[mid] ^= 0xFF;
        fs::write(&path, &data).unwrap();

        let engine2 = DashMapEngine::new();
        let result = load_snapshot(&engine2, &path);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("CRC"));
    }

    #[test]
    fn missing_file_returns_zero() {
        let engine = DashMapEngine::new();
        let count = load_snapshot(&engine, "nonexistent_file_xyz.zdb").unwrap();
        assert_eq!(count, 0);
    }

    #[test]
    fn wrong_magic() {
        let path = temp_path("badmagic");
        ensure_dir(&path);
        // HEADER_SIZE (18) + CRC_SIZE (4) = 22 bytes minimum
        let mut data = vec![0u8; 22];
        data[0..4].copy_from_slice(b"BAD1");
        data[4] = 1; // version
        fs::write(&path, &data).unwrap();

        let engine = DashMapEngine::new();
        let result = load_snapshot(&engine, &path);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("magic"));
    }

    #[test]
    fn binary_values() {
        let path = temp_path("binary");
        ensure_dir(&path);
        let _ = fs::remove_file(&path);

        let engine = DashMapEngine::new();
        let binary: Vec<u8> = vec![0x00, 0xFF, 0xDE, 0xAD, 0xBE, 0xEF];
        engine
            .set("bin".into(), ValueEntry::new(Bytes::from(binary.clone())))
            .unwrap();

        dump_snapshot(&engine, &path).unwrap();

        let engine2 = DashMapEngine::new();
        load_snapshot(&engine2, &path).unwrap();

        assert_eq!(engine2.get("bin").unwrap().unwrap().data.as_ref(), binary.as_slice());
    }
}
