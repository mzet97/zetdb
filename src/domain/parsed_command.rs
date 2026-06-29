use bytes::Bytes;
use std::time::Duration;

/// Borrowed command for zero-allocation hot path.
/// All string data borrows from the input buffer.
#[derive(Debug, Clone)]
pub enum ParsedCommand<'a> {
    Ping,
    Get { key: &'a [u8] },
    Set { key: &'a [u8], value: &'a [u8], ttl: Option<Duration> },
    Del { key: &'a [u8] },
    Incr { key: &'a [u8] },
    Info,
    DbSize,
    Exists { key: &'a [u8] },
    Ttl { key: &'a [u8] },
    Expire { key: &'a [u8], seconds: u64 },
    FlushDb,
    Keys,
    Mget { keys: Vec<&'a [u8]> },
    Mset { pairs: Vec<(&'a [u8], &'a [u8])> },
}

impl<'a> ParsedCommand<'a> {
    pub fn is_write(&self) -> bool {
        matches!(
            self,
            ParsedCommand::Set { .. }
                | ParsedCommand::Del { .. }
                | ParsedCommand::Incr { .. }
                | ParsedCommand::Expire { .. }
                | ParsedCommand::FlushDb
                | ParsedCommand::Mset { .. }
        )
    }

    pub fn command_type(&self) -> crate::observability::metrics::CommandType {
        use crate::observability::metrics::CommandType;
        match self {
            ParsedCommand::Ping => CommandType::Ping,
            ParsedCommand::Get { .. } => CommandType::Get,
            ParsedCommand::Set { .. } => CommandType::Set,
            ParsedCommand::Del { .. } => CommandType::Del,
            ParsedCommand::Incr { .. } => CommandType::Incr,
            ParsedCommand::Info => CommandType::Info,
            ParsedCommand::DbSize => CommandType::DbSize,
            ParsedCommand::Exists { .. } => CommandType::Exists,
            ParsedCommand::Ttl { .. } => CommandType::Ttl,
            ParsedCommand::Expire { .. } => CommandType::Expire,
            ParsedCommand::FlushDb => CommandType::FlushDb,
            ParsedCommand::Keys => CommandType::Keys,
            ParsedCommand::Mget { .. } => CommandType::Mget,
            ParsedCommand::Mset { .. } => CommandType::Mset,
        }
    }

    /// Convert to AOF entry
    pub fn to_aof_entry(&self) -> Option<Vec<u8>> {
        self.to_owned().to_aof_entry()
    }

    /// Convert to owned Command
    pub fn to_owned(&self) -> crate::domain::command::Command {
        use crate::domain::command::Command;
        match self {
            ParsedCommand::Ping => Command::Ping,
            ParsedCommand::Get { key } => Command::Get {
                key: bytes_to_string(key),
            },
            ParsedCommand::Set { key, value, ttl } => Command::Set {
                key: bytes_to_string(key),
                value: Bytes::copy_from_slice(value),
                ttl: *ttl,
            },
            ParsedCommand::Del { key } => Command::Del {
                key: bytes_to_string(key),
            },
            ParsedCommand::Incr { key } => Command::Incr {
                key: bytes_to_string(key),
            },
            ParsedCommand::Info => Command::Info,
            ParsedCommand::DbSize => Command::DbSize,
            ParsedCommand::Exists { key } => Command::Exists {
                key: bytes_to_string(key),
            },
            ParsedCommand::Ttl { key } => Command::Ttl {
                key: bytes_to_string(key),
            },
            ParsedCommand::Expire { key, seconds } => Command::Expire {
                key: bytes_to_string(key),
                seconds: *seconds,
            },
            ParsedCommand::FlushDb => Command::FlushDb,
            ParsedCommand::Keys => Command::Keys,
            ParsedCommand::Mget { keys } => Command::Mget {
                keys: keys.iter().map(|k| bytes_to_string(k)).collect(),
            },
            ParsedCommand::Mset { pairs } => Command::Mset {
                pairs: pairs
                    .iter()
                    .map(|(k, v)| (bytes_to_string(k), Bytes::copy_from_slice(v)))
                    .collect(),
            },
        }
    }
}

#[inline(always)]
fn bytes_to_string(b: &[u8]) -> String {
    unsafe { String::from_utf8_unchecked(b.to_vec()) }
}
