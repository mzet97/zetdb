use bytes::Bytes;
use std::time::Duration;

use crate::observability::metrics::CommandType;

#[derive(Debug, PartialEq)]
pub enum Command {
    Ping,
    Get {
        key: String,
    },
    Set {
        key: String,
        value: Bytes,
        ttl: Option<Duration>,
    },
    Del {
        key: String,
    },
    Incr {
        key: String,
    },
    Info,
    DbSize,
    Exists {
        key: String,
    },
    Ttl {
        key: String,
    },
    Expire {
        key: String,
        seconds: u64,
    },
    FlushDb,
    Keys,
    Mget {
        keys: Vec<String>,
    },
    Mset {
        pairs: Vec<(String, Bytes)>,
    },
}

impl Command {
    pub fn is_write(&self) -> bool {
        matches!(
            self,
            Command::Set { .. }
                | Command::Del { .. }
                | Command::Incr { .. }
                | Command::Expire { .. }
                | Command::FlushDb
                | Command::Mset { .. }
        )
    }

    pub fn command_type(&self) -> CommandType {
        match self {
            Command::Ping => CommandType::Ping,
            Command::Get { .. } => CommandType::Get,
            Command::Set { .. } => CommandType::Set,
            Command::Del { .. } => CommandType::Del,
            Command::Incr { .. } => CommandType::Incr,
            Command::Info => CommandType::Info,
            Command::DbSize => CommandType::DbSize,
            Command::Exists { .. } => CommandType::Exists,
            Command::Ttl { .. } => CommandType::Ttl,
            Command::Expire { .. } => CommandType::Expire,
            Command::FlushDb => CommandType::FlushDb,
            Command::Keys => CommandType::Keys,
            Command::Mget { .. } => CommandType::Mget,
            Command::Mset { .. } => CommandType::Mset,
        }
    }

    /// Serialize to AOF binary format. Returns None for read-only commands.
    ///
    /// Format per command type:
    /// - SET: [0x01] [key_len:u16 LE] [key] [val_len:u32 LE] [value] [ttl_ms:i64 LE]
    /// - DEL: [0x02] [key_len:u16 LE] [key]
    /// - INCR: [0x03] [key_len:u16 LE] [key]
    /// - EXPIRE: [0x04] [key_len:u16 LE] [key] [seconds:u64 LE]
    /// - FLUSHDB: [0x05]
    /// - MSET: [0x06] [count:u16 LE] [for each: key_len:u16 LE, key, val_len:u32 LE, value]
    pub fn to_aof_entry(&self) -> Option<Vec<u8>> {
        match self {
            Command::Set { key, value, ttl } => {
                let key_bytes = key.as_bytes();
                let mut buf = Vec::with_capacity(1 + 2 + key_bytes.len() + 4 + value.len() + 8);
                buf.push(0x01);
                buf.extend_from_slice(&(key_bytes.len() as u16).to_le_bytes());
                buf.extend_from_slice(key_bytes);
                buf.extend_from_slice(&(value.len() as u32).to_le_bytes());
                buf.extend_from_slice(value);
                let ttl_ms = match ttl {
                    Some(d) => d.as_millis() as i64,
                    None => -1,
                };
                buf.extend_from_slice(&ttl_ms.to_le_bytes());
                Some(buf)
            }
            Command::Del { key } => {
                let key_bytes = key.as_bytes();
                let mut buf = Vec::with_capacity(1 + 2 + key_bytes.len());
                buf.push(0x02);
                buf.extend_from_slice(&(key_bytes.len() as u16).to_le_bytes());
                buf.extend_from_slice(key_bytes);
                Some(buf)
            }
            Command::Incr { key } => {
                let key_bytes = key.as_bytes();
                let mut buf = Vec::with_capacity(1 + 2 + key_bytes.len());
                buf.push(0x03);
                buf.extend_from_slice(&(key_bytes.len() as u16).to_le_bytes());
                buf.extend_from_slice(key_bytes);
                Some(buf)
            }
            Command::Expire { key, seconds } => {
                let key_bytes = key.as_bytes();
                let mut buf = Vec::with_capacity(1 + 2 + key_bytes.len() + 8);
                buf.push(0x04);
                buf.extend_from_slice(&(key_bytes.len() as u16).to_le_bytes());
                buf.extend_from_slice(key_bytes);
                buf.extend_from_slice(&seconds.to_le_bytes());
                Some(buf)
            }
            Command::FlushDb => Some(vec![0x05]),
            Command::Mset { pairs } => {
                let mut buf = Vec::with_capacity(1 + 2 + pairs.len() * 32);
                buf.push(0x06);
                buf.extend_from_slice(&(pairs.len() as u16).to_le_bytes());
                for (key, value) in pairs {
                    let key_bytes = key.as_bytes();
                    buf.extend_from_slice(&(key_bytes.len() as u16).to_le_bytes());
                    buf.extend_from_slice(key_bytes);
                    buf.extend_from_slice(&(value.len() as u32).to_le_bytes());
                    buf.extend_from_slice(value);
                }
                Some(buf)
            }
            _ => None,
        }
    }
}
