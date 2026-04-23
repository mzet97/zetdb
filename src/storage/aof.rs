use std::fs;
use std::io::{self, Write};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use bytes::Bytes;

use crate::storage::dashmap_engine::DashMapEngine;
use crate::storage::engine::KvEngine;
use crate::domain::value::ValueEntry;

const CMD_SET: u8 = 0x01;
const CMD_DEL: u8 = 0x02;
const CMD_INCR: u8 = 0x03;

pub struct AofWriter {
    file: Mutex<fs::File>,
    path: String,
    fsync_policy: crate::config::FsyncPolicy,
    last_fsync: Mutex<Instant>,
}

impl AofWriter {
    pub fn new(path: &str, fsync_policy: crate::config::FsyncPolicy) -> io::Result<Self> {
        let file = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)?;

        Ok(Self {
            file: Mutex::new(file),
            path: path.to_string(),
            fsync_policy,
            last_fsync: Mutex::new(Instant::now()),
        })
    }

    /// Append a pre-serialized AOF entry (from Command::to_aof_entry).
    pub fn append_raw(&self, entry: &[u8]) -> io::Result<()> {
        let mut file = self.file.lock().unwrap();
        file.write_all(entry)?;

        match self.fsync_policy {
            crate::config::FsyncPolicy::EveryWrite => {
                file.sync_all()?;
            }
            crate::config::FsyncPolicy::EverySecond => {
                let mut last = self.last_fsync.lock().unwrap();
                if last.elapsed() >= Duration::from_secs(1) {
                    file.sync_all()?;
                    *last = Instant::now();
                }
            }
            crate::config::FsyncPolicy::Never => {}
        }

        Ok(())
    }

    /// Force fsync regardless of policy (for background ticker).
    pub fn flush_if_needed(&self) -> io::Result<()> {
        if matches!(self.fsync_policy, crate::config::FsyncPolicy::EverySecond) {
            let mut last = self.last_fsync.lock().unwrap();
            if last.elapsed() >= Duration::from_secs(1) {
                self.file.lock().unwrap().sync_all()?;
                *last = Instant::now();
            }
        }
        Ok(())
    }

    pub fn file_size(&self) -> io::Result<u64> {
        Ok(fs::metadata(&self.path)?.len())
    }

    pub fn path(&self) -> &str {
        &self.path
    }

    /// Reopen file handle after rewrite.
    pub fn reopen(&self) -> io::Result<()> {
        let mut file = self.file.lock().unwrap();
        *file = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)?;
        Ok(())
    }
}

// -- Encoding helpers (shared with Command::to_aof_entry format) --

fn encode_key(buf: &mut Vec<u8>, key: &str) {
    let key_bytes = key.as_bytes();
    buf.extend_from_slice(&(key_bytes.len() as u16).to_le_bytes());
    buf.extend_from_slice(key_bytes);
}

fn encode_value(buf: &mut Vec<u8>, value: &[u8]) {
    buf.extend_from_slice(&(value.len() as u32).to_le_bytes());
    buf.extend_from_slice(value);
}

fn encode_ttl(buf: &mut Vec<u8>, ttl_ms: i64) {
    buf.extend_from_slice(&ttl_ms.to_le_bytes());
}

/// Replay AOF entries into the engine. Returns number of commands replayed.
/// Returns Ok(0) if file doesn't exist.
pub fn replay_aof(engine: &DashMapEngine, path: &str) -> Result<usize, io::Error> {
    let data = match fs::read(path) {
        Ok(d) => d,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(0),
        Err(e) => return Err(e),
    };

    let mut pos = 0;
    let mut replayed = 0;

    while pos < data.len() {
        if pos + 1 > data.len() {
            break;
        }
        let cmd_type = data[pos];
        pos += 1;

        match cmd_type {
            CMD_SET => {
                // Key
                if pos + 2 > data.len() {
                    break;
                }
                let key_len = u16::from_le_bytes(data[pos..pos + 2].try_into().unwrap()) as usize;
                pos += 2;
                if pos + key_len > data.len() {
                    break;
                }
                let key = String::from_utf8_lossy(&data[pos..pos + key_len]).into_owned();
                pos += key_len;

                // Value
                if pos + 4 > data.len() {
                    break;
                }
                let val_len =
                    u32::from_le_bytes(data[pos..pos + 4].try_into().unwrap()) as usize;
                pos += 4;
                if pos + val_len > data.len() {
                    break;
                }
                let value = Bytes::copy_from_slice(&data[pos..pos + val_len]);
                pos += val_len;

                // TTL
                if pos + 8 > data.len() {
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
            }
            CMD_DEL => {
                if pos + 2 > data.len() {
                    break;
                }
                let key_len = u16::from_le_bytes(data[pos..pos + 2].try_into().unwrap()) as usize;
                pos += 2;
                if pos + key_len > data.len() {
                    break;
                }
                let key = String::from_utf8_lossy(&data[pos..pos + key_len]).into_owned();
                pos += key_len;

                engine.del(&key).ok();
            }
            CMD_INCR => {
                if pos + 2 > data.len() {
                    break;
                }
                let key_len = u16::from_le_bytes(data[pos..pos + 2].try_into().unwrap()) as usize;
                pos += 2;
                if pos + key_len > data.len() {
                    break;
                }
                let key = String::from_utf8_lossy(&data[pos..pos + key_len]).into_owned();
                pos += key_len;

                engine.incr(&key).ok();
            }
            _ => break, // Unknown command type, stop replay
        }

        replayed += 1;
    }

    Ok(replayed)
}

/// Rewrite AOF by dumping current state as SET commands only.
/// Uses atomic write (temp file + rename).
pub fn rewrite_aof(engine: &DashMapEngine, path: &str) -> Result<usize, io::Error> {
    let tmp_path = format!("{path}.tmp");

    let mut buf = Vec::with_capacity(64 * 1024);

    let count = engine.dump_entries(|key, value, ttl_ms| {
        buf.push(CMD_SET);
        encode_key(&mut buf, key);
        encode_value(&mut buf, value);
        encode_ttl(&mut buf, ttl_ms);
    });

    {
        let mut file = fs::File::create(&tmp_path)?;
        file.write_all(&buf)?;
        file.sync_all()?;
    }
    fs::rename(&tmp_path, path)?;

    Ok(count)
}

/// Background task that periodically rewrites the AOF when it exceeds the threshold.
pub async fn run_aof_rewriter(
    engine: std::sync::Arc<DashMapEngine>,
    writer: std::sync::Arc<AofWriter>,
    threshold_bytes: u64,
    check_interval: Duration,
) {
    let mut ticker = tokio::time::interval(check_interval);
    loop {
        ticker.tick().await;

        let file_size = match writer.file_size() {
            Ok(s) => s,
            Err(e) => {
                log::error!("aof size check failed: {e}");
                continue;
            }
        };

        if file_size < threshold_bytes {
            continue;
        }

        log::info!("aof rewrite triggered: {file_size} bytes >= {threshold_bytes} threshold");

        match rewrite_aof(engine.as_ref(), writer.path()) {
            Ok(count) => {
                if let Err(e) = writer.reopen() {
                    log::error!("aof reopen after rewrite failed: {e}");
                } else {
                    log::info!("aof rewrite complete: {count} entries");
                }
            }
            Err(e) => log::error!("aof rewrite failed: {e}"),
        }
    }
}

/// Background fsync ticker for EverySecond policy.
pub async fn run_aof_fsync(writer: std::sync::Arc<AofWriter>) {
    let mut ticker = tokio::time::interval(Duration::from_secs(1));
    loop {
        ticker.tick().await;
        if let Err(e) = writer.flush_if_needed() {
            log::error!("aof fsync failed: {e}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn temp_path(name: &str) -> String {
        format!("target/test_aof/{name}.aof")
    }

    fn ensure_dir(path: &str) {
        let dir = Path::new(path).parent().unwrap();
        let _ = fs::create_dir_all(dir);
    }

    #[test]
    fn append_and_replay_roundtrip() {
        let path = temp_path("roundtrip");
        ensure_dir(&path);
        let _ = fs::remove_file(&path);

        let writer = AofWriter::new(&path, crate::config::FsyncPolicy::EveryWrite).unwrap();

        // Build entries manually
        let engine = DashMapEngine::new();
        engine.set("k1".into(), ValueEntry::new(Bytes::from("v1"))).unwrap();
        engine.set("k2".into(), ValueEntry::new(Bytes::from("v2"))).unwrap();

        // Serialize and append
        let mut buf = Vec::new();
        buf.push(CMD_SET);
        encode_key(&mut buf, "k1");
        encode_value(&mut buf, b"v1");
        encode_ttl(&mut buf, -1);
        writer.append_raw(&buf).unwrap();

        let mut buf = Vec::new();
        buf.push(CMD_SET);
        encode_key(&mut buf, "k2");
        encode_value(&mut buf, b"v2");
        encode_ttl(&mut buf, -1);
        writer.append_raw(&buf).unwrap();

        // Replay
        let engine2 = DashMapEngine::new();
        let count = replay_aof(&engine2, &path).unwrap();
        assert_eq!(count, 2);
        assert_eq!(engine2.get("k1").unwrap().unwrap().data, Bytes::from("v1"));
        assert_eq!(engine2.get("k2").unwrap().unwrap().data, Bytes::from("v2"));
    }

    #[test]
    fn ttl_preserved_in_aof() {
        let path = temp_path("ttl");
        ensure_dir(&path);
        let _ = fs::remove_file(&path);

        let writer = AofWriter::new(&path, crate::config::FsyncPolicy::Never).unwrap();

        let mut buf = Vec::new();
        buf.push(CMD_SET);
        encode_key(&mut buf, "ttl_key");
        encode_value(&mut buf, b"val");
        encode_ttl(&mut buf, 300_000); // 5 minutes
        writer.append_raw(&buf).unwrap();

        let engine = DashMapEngine::new();
        replay_aof(&engine, &path).unwrap();

        let entry = engine.get("ttl_key").unwrap().unwrap();
        assert_eq!(entry.data, Bytes::from("val"));
        assert!(entry.expires_at.is_some());
    }

    #[test]
    fn del_removes_key() {
        let path = temp_path("del");
        ensure_dir(&path);
        let _ = fs::remove_file(&path);

        let writer = AofWriter::new(&path, crate::config::FsyncPolicy::Never).unwrap();

        // SET k1 v1
        let mut buf = Vec::new();
        buf.push(CMD_SET);
        encode_key(&mut buf, "k1");
        encode_value(&mut buf, b"v1");
        encode_ttl(&mut buf, -1);
        writer.append_raw(&buf).unwrap();

        // DEL k1
        let mut buf = Vec::new();
        buf.push(CMD_DEL);
        encode_key(&mut buf, "k1");
        writer.append_raw(&buf).unwrap();

        let engine = DashMapEngine::new();
        replay_aof(&engine, &path).unwrap();

        assert!(engine.get("k1").unwrap().is_none());
    }

    #[test]
    fn incr_replay() {
        let path = temp_path("incr");
        ensure_dir(&path);
        let _ = fs::remove_file(&path);

        let writer = AofWriter::new(&path, crate::config::FsyncPolicy::Never).unwrap();

        // INCR counter x3
        for _ in 0..3 {
            let mut buf = Vec::new();
            buf.push(CMD_INCR);
            encode_key(&mut buf, "counter");
            writer.append_raw(&buf).unwrap();
        }

        let engine = DashMapEngine::new();
        replay_aof(&engine, &path).unwrap();

        assert_eq!(engine.get("counter").unwrap().unwrap().data, Bytes::from("3"));
    }

    #[test]
    fn rewrite_compacts_aof() {
        let path = temp_path("compact");
        ensure_dir(&path);
        let _ = fs::remove_file(&path);

        let engine = DashMapEngine::new();
        engine.set("k1".into(), ValueEntry::new(Bytes::from("v1"))).unwrap();
        engine.set("k2".into(), ValueEntry::new(Bytes::from("v2"))).unwrap();

        rewrite_aof(&engine, &path).unwrap();

        let engine2 = DashMapEngine::new();
        let count = replay_aof(&engine2, &path).unwrap();
        assert_eq!(count, 2);
        assert_eq!(engine2.get("k1").unwrap().unwrap().data, Bytes::from("v1"));
        assert_eq!(engine2.get("k2").unwrap().unwrap().data, Bytes::from("v2"));
    }

    #[test]
    fn missing_file_returns_zero() {
        let engine = DashMapEngine::new();
        let count = replay_aof(&engine, "nonexistent_aof_xyz.aof").unwrap();
        assert_eq!(count, 0);
    }

    #[test]
    fn binary_values_in_aof() {
        let path = temp_path("binary");
        ensure_dir(&path);
        let _ = fs::remove_file(&path);

        let writer = AofWriter::new(&path, crate::config::FsyncPolicy::Never).unwrap();

        let binary: Vec<u8> = vec![0x00, 0xFF, 0xDE, 0xAD, 0xBE, 0xEF];
        let mut buf = Vec::new();
        buf.push(CMD_SET);
        encode_key(&mut buf, "bin");
        encode_value(&mut buf, &binary);
        encode_ttl(&mut buf, -1);
        writer.append_raw(&buf).unwrap();

        let engine = DashMapEngine::new();
        replay_aof(&engine, &path).unwrap();

        assert_eq!(
            engine.get("bin").unwrap().unwrap().data.as_ref(),
            binary.as_slice()
        );
    }
}
