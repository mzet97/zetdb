use std::fs;
use std::io::{self, Write};
use std::time::{Duration, Instant};

use bytes::Bytes;

use crate::domain::value::ValueEntry;
use crate::storage::dashmap_engine::DashMapEngine;
use crate::storage::engine::KvEngine;

const CMD_SET: u8 = 0x01;
const CMD_DEL: u8 = 0x02;
const CMD_INCR: u8 = 0x03;
const CMD_EXPIRE: u8 = 0x04;
const CMD_FLUSHDB: u8 = 0x05;
const CMD_MSET: u8 = 0x06;

pub struct AofWriter {
    file: tokio::sync::Mutex<fs::File>,
    path: String,
    fsync_policy: crate::config::FsyncPolicy,
    last_fsync: tokio::sync::Mutex<Instant>,
}

impl AofWriter {
    pub fn new(path: &str, fsync_policy: crate::config::FsyncPolicy) -> io::Result<Self> {
        let file = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)?;

        Ok(Self {
            file: tokio::sync::Mutex::new(file),
            path: path.to_string(),
            fsync_policy,
            last_fsync: tokio::sync::Mutex::new(Instant::now()),
        })
    }

    /// Append a pre-serialized AOF entry (from Command::to_aof_entry).
    pub async fn append_raw(&self, entry: &[u8]) -> io::Result<()> {
        let mut file = self.file.lock().await;
        file.write_all(entry)?;

        match self.fsync_policy {
            crate::config::FsyncPolicy::Always => {
                file.sync_all()?;
            }
            crate::config::FsyncPolicy::Everysec => {
                let mut last = self.last_fsync.lock().await;
                if last.elapsed() >= Duration::from_secs(1) {
                    file.sync_all()?;
                    *last = Instant::now();
                }
            }
            crate::config::FsyncPolicy::No => {}
        }

        Ok(())
    }

    /// Force fsync regardless of policy (for background ticker).
    pub async fn flush_if_needed(&self) -> io::Result<()> {
        if matches!(self.fsync_policy, crate::config::FsyncPolicy::Everysec) {
            let mut last = self.last_fsync.lock().await;
            if last.elapsed() >= Duration::from_secs(1) {
                let file = self.file.lock().await;
                file.sync_all()?;
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

    /// Atomically swap the AOF file by renaming `tmp_path` over the current
    /// file path and reopening the file handle. This is done inside the lock
    /// so no concurrent append can write to a stale inode.
    pub async fn finalize_rewrite(&self, tmp_path: &str) -> io::Result<()> {
        let mut file = self.file.lock().await;
        fs::rename(tmp_path, &self.path)?;
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

/// Helper: read a u16 little-endian from `data` at `pos`, advance `pos` by 2.
fn read_u16_le(data: &[u8], pos: &mut usize) -> io::Result<u16> {
    let end = pos.checked_add(2).ok_or_else(|| {
        io::Error::new(io::ErrorKind::InvalidData, "aof read overflow")
    })?;
    let bytes = data.get(*pos..end).ok_or_else(|| {
        io::Error::new(io::ErrorKind::InvalidData, "aof truncated u16")
    })?;
    let arr: [u8; 2] = bytes.try_into().map_err(|_| {
        io::Error::new(io::ErrorKind::InvalidData, "aof truncated u16")
    })?;
    *pos = end;
    Ok(u16::from_le_bytes(arr))
}

/// Helper: read a u32 little-endian from `data` at `pos`, advance `pos` by 4.
fn read_u32_le(data: &[u8], pos: &mut usize) -> io::Result<u32> {
    let end = pos.checked_add(4).ok_or_else(|| {
        io::Error::new(io::ErrorKind::InvalidData, "aof read overflow")
    })?;
    let bytes = data.get(*pos..end).ok_or_else(|| {
        io::Error::new(io::ErrorKind::InvalidData, "aof truncated u32")
    })?;
    let arr: [u8; 4] = bytes.try_into().map_err(|_| {
        io::Error::new(io::ErrorKind::InvalidData, "aof truncated u32")
    })?;
    *pos = end;
    Ok(u32::from_le_bytes(arr))
}

/// Helper: read a u64 little-endian from `data` at `pos`, advance `pos` by 8.
fn read_u64_le(data: &[u8], pos: &mut usize) -> io::Result<u64> {
    let end = pos.checked_add(8).ok_or_else(|| {
        io::Error::new(io::ErrorKind::InvalidData, "aof read overflow")
    })?;
    let bytes = data.get(*pos..end).ok_or_else(|| {
        io::Error::new(io::ErrorKind::InvalidData, "aof truncated u64")
    })?;
    let arr: [u8; 8] = bytes.try_into().map_err(|_| {
        io::Error::new(io::ErrorKind::InvalidData, "aof truncated u64")
    })?;
    *pos = end;
    Ok(u64::from_le_bytes(arr))
}

/// Helper: read an i64 little-endian from `data` at `pos`, advance `pos` by 8.
fn read_i64_le(data: &[u8], pos: &mut usize) -> io::Result<i64> {
    let end = pos.checked_add(8).ok_or_else(|| {
        io::Error::new(io::ErrorKind::InvalidData, "aof read overflow")
    })?;
    let bytes = data.get(*pos..end).ok_or_else(|| {
        io::Error::new(io::ErrorKind::InvalidData, "aof truncated i64")
    })?;
    let arr: [u8; 8] = bytes.try_into().map_err(|_| {
        io::Error::new(io::ErrorKind::InvalidData, "aof truncated i64")
    })?;
    *pos = end;
    Ok(i64::from_le_bytes(arr))
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
                if pos + 2 > data.len() {
                    log::warn!("aof replay: truncated at key length (pos={pos})");
                    break;
                }
                let key_len = read_u16_le(&data, &mut pos)? as usize;
                if pos + key_len > data.len() {
                    log::warn!("aof replay: truncated at key data (pos={pos}, key_len={key_len})");
                    break;
                }
                let key = String::from_utf8_lossy(&data[pos..pos + key_len]).into_owned();
                pos += key_len;

                if pos + 4 > data.len() {
                    log::warn!("aof replay: truncated at value length (pos={pos})");
                    break;
                }
                let val_len = read_u32_le(&data, &mut pos)? as usize;
                if pos + val_len > data.len() {
                    log::warn!("aof replay: truncated at value data (pos={pos}, val_len={val_len})");
                    break;
                }
                let value = Bytes::copy_from_slice(&data[pos..pos + val_len]);
                pos += val_len;

                if pos + 8 > data.len() {
                    log::warn!("aof replay: truncated at ttl (pos={pos})");
                    break;
                }
                let ttl_ms = read_i64_le(&data, &mut pos)?;

                let expires_at = if ttl_ms > 0 {
                    Some(Instant::now() + Duration::from_millis(ttl_ms as u64))
                } else {
                    None
                };

                if let Err(e) = engine.set(
                    key.clone(),
                    ValueEntry {
                        data: value,
                        expires_at,
                        created_at: Instant::now(),
                    },
                ) {
                    log::warn!("aof replay: failed to set key '{key}': {e}");
                }
            }
            CMD_DEL => {
                if pos + 2 > data.len() {
                    log::warn!("aof replay: truncated at DEL key length (pos={pos})");
                    break;
                }
                let key_len = read_u16_le(&data, &mut pos)? as usize;
                if pos + key_len > data.len() {
                    log::warn!("aof replay: truncated at DEL key data (pos={pos}, key_len={key_len})");
                    break;
                }
                let key = String::from_utf8_lossy(&data[pos..pos + key_len]).into_owned();
                pos += key_len;

                if let Err(e) = engine.del(&key) {
                    log::warn!("aof replay: failed to del key '{key}': {e}");
                }
            }
            CMD_INCR => {
                if pos + 2 > data.len() {
                    log::warn!("aof replay: truncated at INCR key length (pos={pos})");
                    break;
                }
                let key_len = read_u16_le(&data, &mut pos)? as usize;
                if pos + key_len > data.len() {
                    log::warn!("aof replay: truncated at INCR key data (pos={pos}, key_len={key_len})");
                    break;
                }
                let key = String::from_utf8_lossy(&data[pos..pos + key_len]).into_owned();
                pos += key_len;

                if let Err(e) = engine.incr(&key) {
                    log::warn!("aof replay: failed to incr key '{key}': {e}");
                }
            }
            CMD_EXPIRE => {
                if pos + 2 > data.len() {
                    log::warn!("aof replay: truncated at EXPIRE key length (pos={pos})");
                    break;
                }
                let key_len = read_u16_le(&data, &mut pos)? as usize;
                if pos + key_len > data.len() {
                    log::warn!("aof replay: truncated at EXPIRE key data (pos={pos}, key_len={key_len})");
                    break;
                }
                let key = String::from_utf8_lossy(&data[pos..pos + key_len]).into_owned();
                pos += key_len;

                if pos + 8 > data.len() {
                    log::warn!("aof replay: truncated at EXPIRE seconds (pos={pos})");
                    break;
                }
                let seconds = read_u64_le(&data, &mut pos)?;

                engine.expire(&key, seconds);
            }
            CMD_FLUSHDB => {
                engine.clear();
            }
            CMD_MSET => {
                if pos + 2 > data.len() {
                    log::warn!("aof replay: truncated at MSET count (pos={pos})");
                    break;
                }
                let count = read_u16_le(&data, &mut pos)? as usize;

                for _ in 0..count {
                    if pos + 2 > data.len() {
                        log::warn!("aof replay: truncated at MSET key length (pos={pos})");
                        break;
                    }
                    let key_len = read_u16_le(&data, &mut pos)? as usize;
                    if pos + key_len > data.len() {
                        log::warn!("aof replay: truncated at MSET key data (pos={pos}, key_len={key_len})");
                        break;
                    }
                    let key = String::from_utf8_lossy(&data[pos..pos + key_len]).into_owned();
                    pos += key_len;

                    if pos + 4 > data.len() {
                        log::warn!("aof replay: truncated at MSET value length (pos={pos})");
                        break;
                    }
                    let val_len = read_u32_le(&data, &mut pos)? as usize;
                    if pos + val_len > data.len() {
                        log::warn!("aof replay: truncated at MSET value data (pos={pos}, val_len={val_len})");
                        break;
                    }
                    let value = Bytes::copy_from_slice(&data[pos..pos + val_len]);
                    pos += val_len;

                    if let Err(e) = engine.set(key.clone(), ValueEntry::new(value)) {
                        log::warn!("aof replay: failed to mset key '{key}': {e}");
                    }
                }
            }
            _ => {
                log::warn!("aof replay: unknown command type 0x{cmd_type:02x} at pos={pos}, stopping replay");
                break;
            }
        }

        replayed += 1;
    }

    Ok(replayed)
}

/// Rewrite AOF by dumping current state as SET commands only.
/// Writes to `{path}.tmp`; the caller must rename it atomically.
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
    // NOTE: rename is performed inside AofWriter::finalize_rewrite to avoid races

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
                let tmp_path = format!("{}.tmp", writer.path());
                if let Err(e) = writer.finalize_rewrite(&tmp_path).await {
                    log::error!("aof finalize rewrite failed: {e}");
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
        if let Err(e) = writer.flush_if_needed().await {
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

    #[tokio::test]
    async fn append_and_replay_roundtrip() {
        let path = temp_path("roundtrip");
        ensure_dir(&path);
        let _ = fs::remove_file(&path);

        let writer = AofWriter::new(&path, crate::config::FsyncPolicy::Always).unwrap();

        // Build entries manually
        let engine = DashMapEngine::new();
        engine
            .set("k1".into(), ValueEntry::new(Bytes::from("v1")))
            .unwrap();
        engine
            .set("k2".into(), ValueEntry::new(Bytes::from("v2")))
            .unwrap();

        // Serialize and append
        let mut buf = Vec::new();
        buf.push(CMD_SET);
        encode_key(&mut buf, "k1");
        encode_value(&mut buf, b"v1");
        encode_ttl(&mut buf, -1);
        writer.append_raw(&buf).await.unwrap();

        let mut buf = Vec::new();
        buf.push(CMD_SET);
        encode_key(&mut buf, "k2");
        encode_value(&mut buf, b"v2");
        encode_ttl(&mut buf, -1);
        writer.append_raw(&buf).await.unwrap();

        // Replay
        let engine2 = DashMapEngine::new();
        let count = replay_aof(&engine2, &path).unwrap();
        assert_eq!(count, 2);
        assert_eq!(engine2.get("k1").unwrap().unwrap().data, Bytes::from("v1"));
        assert_eq!(engine2.get("k2").unwrap().unwrap().data, Bytes::from("v2"));
    }

    #[tokio::test]
    async fn ttl_preserved_in_aof() {
        let path = temp_path("ttl");
        ensure_dir(&path);
        let _ = fs::remove_file(&path);

        let writer = AofWriter::new(&path, crate::config::FsyncPolicy::No).unwrap();

        let mut buf = Vec::new();
        buf.push(CMD_SET);
        encode_key(&mut buf, "ttl_key");
        encode_value(&mut buf, b"val");
        encode_ttl(&mut buf, 300_000); // 5 minutes
        writer.append_raw(&buf).await.unwrap();

        let engine = DashMapEngine::new();
        replay_aof(&engine, &path).unwrap();

        let entry = engine.get("ttl_key").unwrap().unwrap();
        assert_eq!(entry.data, Bytes::from("val"));
        assert!(entry.expires_at.is_some());
    }

    #[tokio::test]
    async fn del_removes_key() {
        let path = temp_path("del");
        ensure_dir(&path);
        let _ = fs::remove_file(&path);

        let writer = AofWriter::new(&path, crate::config::FsyncPolicy::No).unwrap();

        // SET k1 v1
        let mut buf = Vec::new();
        buf.push(CMD_SET);
        encode_key(&mut buf, "k1");
        encode_value(&mut buf, b"v1");
        encode_ttl(&mut buf, -1);
        writer.append_raw(&buf).await.unwrap();

        // DEL k1
        let mut buf = Vec::new();
        buf.push(CMD_DEL);
        encode_key(&mut buf, "k1");
        writer.append_raw(&buf).await.unwrap();

        let engine = DashMapEngine::new();
        replay_aof(&engine, &path).unwrap();

        assert!(engine.get("k1").unwrap().is_none());
    }

    #[tokio::test]
    async fn incr_replay() {
        let path = temp_path("incr");
        ensure_dir(&path);
        let _ = fs::remove_file(&path);

        let writer = AofWriter::new(&path, crate::config::FsyncPolicy::No).unwrap();

        // INCR counter x3
        for _ in 0..3 {
            let mut buf = Vec::new();
            buf.push(CMD_INCR);
            encode_key(&mut buf, "counter");
            writer.append_raw(&buf).await.unwrap();
        }

        let engine = DashMapEngine::new();
        replay_aof(&engine, &path).unwrap();

        assert_eq!(
            engine.get("counter").unwrap().unwrap().data,
            Bytes::from("3")
        );
    }

    #[test]
    fn rewrite_compacts_aof() {
        let path = temp_path("compact");
        ensure_dir(&path);
        let _ = fs::remove_file(&path);

        let engine = DashMapEngine::new();
        engine
            .set("k1".into(), ValueEntry::new(Bytes::from("v1")))
            .unwrap();
        engine
            .set("k2".into(), ValueEntry::new(Bytes::from("v2")))
            .unwrap();

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

    #[tokio::test]
    async fn binary_values_in_aof() {
        let path = temp_path("binary");
        ensure_dir(&path);
        let _ = fs::remove_file(&path);

        let writer = AofWriter::new(&path, crate::config::FsyncPolicy::No).unwrap();

        let binary: Vec<u8> = vec![0x00, 0xFF, 0xDE, 0xAD, 0xBE, 0xEF];
        let mut buf = Vec::new();
        buf.push(CMD_SET);
        encode_key(&mut buf, "bin");
        encode_value(&mut buf, &binary);
        encode_ttl(&mut buf, -1);
        writer.append_raw(&buf).await.unwrap();

        let engine = DashMapEngine::new();
        replay_aof(&engine, &path).unwrap();

        assert_eq!(
            engine.get("bin").unwrap().unwrap().data.as_ref(),
            binary.as_slice()
        );
    }

    #[tokio::test]
    async fn mset_replay() {
        let path = temp_path("mset");
        ensure_dir(&path);
        let _ = fs::remove_file(&path);

        let writer = AofWriter::new(&path, crate::config::FsyncPolicy::No).unwrap();

        // MSET with 2 pairs: k1=v1, k2=v2
        let mut buf = Vec::new();
        buf.push(CMD_MSET);
        buf.extend_from_slice(&2u16.to_le_bytes());
        encode_key(&mut buf, "k1");
        encode_value(&mut buf, b"v1");
        encode_key(&mut buf, "k2");
        encode_value(&mut buf, b"v2");
        writer.append_raw(&buf).await.unwrap();

        let engine = DashMapEngine::new();
        let count = replay_aof(&engine, &path).unwrap();
        assert_eq!(count, 1);
        assert_eq!(engine.get("k1").unwrap().unwrap().data, Bytes::from("v1"));
        assert_eq!(engine.get("k2").unwrap().unwrap().data, Bytes::from("v2"));
    }

    #[tokio::test]
    async fn truncated_aof_does_not_panic() {
        let path = temp_path("truncated");
        ensure_dir(&path);
        let _ = fs::remove_file(&path);

        let writer = AofWriter::new(&path, crate::config::FsyncPolicy::No).unwrap();

        // Write a complete SET for k1
        let mut buf = Vec::new();
        buf.push(CMD_SET);
        encode_key(&mut buf, "k1");
        encode_value(&mut buf, b"v1");
        encode_ttl(&mut buf, -1);
        writer.append_raw(&buf).await.unwrap();

        // Write a partial SET for k2 (truncate inside key length)
        let mut buf = Vec::new();
        buf.push(CMD_SET);
        buf.extend_from_slice(&2u16.to_le_bytes()); // key_len for "k2"
        buf.extend_from_slice(b"k"); // only 1 byte of key, missing "2"
        writer.append_raw(&buf).await.unwrap();

        let engine = DashMapEngine::new();
        // Should not panic — replays partial data and stops gracefully
        let count = replay_aof(&engine, &path).unwrap();
        assert_eq!(count, 1);
        assert_eq!(engine.get("k1").unwrap().unwrap().data, Bytes::from("v1"));
        assert!(engine.get("k2").unwrap().is_none());
    }
}
