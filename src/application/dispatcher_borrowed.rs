use crate::domain::parsed_command::ParsedCommand;
use crate::storage::dashmap_engine::DashMapEngine;
use crate::storage::engine::KvEngine;
use bytes::BytesMut;

/// Dispatch borrowed command and write response directly to buffer (zero-allocation hot path).
/// This avoids creating Command enum and boxing errors.
#[inline(always)]
pub fn dispatch_borrowed_to_buffer(engine: &DashMapEngine, cmd: ParsedCommand<'_>, buf: &mut BytesMut, is_resp: bool) {
    match cmd {
        ParsedCommand::Get { key } => {
            match engine.get(std::str::from_utf8(key).unwrap_or("")) {
                Ok(Some(entry)) => {
                    if is_resp {
                        // RESP bulk string: $<len>\r\n<data>\r\n
                        buf.extend_from_slice(b"$");
                        let mut itoa_buf = itoa::Buffer::new();
                        buf.extend_from_slice(itoa_buf.format(entry.data.len()).as_bytes());
                        buf.extend_from_slice(b"\r\n");
                        buf.extend_from_slice(&entry.data);
                        buf.extend_from_slice(b"\r\n");
                    } else {
                        // Inline simple string: +<data>\r\n
                        buf.extend_from_slice(b"+");
                        buf.extend_from_slice(&entry.data);
                        buf.extend_from_slice(b"\r\n");
                    }
                }
                Ok(None) => buf.extend_from_slice(b"$-1\r\n"),
                Err(_) => buf.extend_from_slice(b"-ERR internal\r\n"),
            }
        }
        ParsedCommand::Set { key, value, ttl } => {
            let key_str = std::str::from_utf8(key).unwrap_or("");
            let entry = match ttl {
                Some(dur) => crate::domain::value::ValueEntry::with_ttl(bytes::Bytes::copy_from_slice(value), dur),
                None => crate::domain::value::ValueEntry::new(bytes::Bytes::copy_from_slice(value)),
            };
            match engine.set(key_str.into(), entry) {
                Ok(()) => buf.extend_from_slice(b"+OK\r\n"),
                Err(_) => buf.extend_from_slice(b"-ERR internal\r\n"),
            }
        }
        ParsedCommand::Ping => buf.extend_from_slice(b"+PONG\r\n"),
        ParsedCommand::Del { key } => match engine.del(std::str::from_utf8(key).unwrap_or("")) {
            Ok(existed) => {
                buf.extend_from_slice(b":");
                let mut itoa_buf = itoa::Buffer::new();
                buf.extend_from_slice(itoa_buf.format(if existed { 1 } else { 0 }).as_bytes());
                buf.extend_from_slice(b"\r\n");
            }
            Err(_) => buf.extend_from_slice(b"-ERR internal\r\n"),
        },
        ParsedCommand::Incr { key } => match engine.incr(std::str::from_utf8(key).unwrap_or("")) {
            Ok(n) => {
                buf.extend_from_slice(b":");
                let mut itoa_buf = itoa::Buffer::new();
                buf.extend_from_slice(itoa_buf.format(n).as_bytes());
                buf.extend_from_slice(b"\r\n");
            }
            Err(_) => buf.extend_from_slice(b"-ERR type\r\n"),
        },
        ParsedCommand::Info => {
            let m = crate::observability::metrics::metrics();
            m.flush_all();
            let uptime = m.uptime_secs();
            let info = format!(
                "# Server\r\nzetdb_version:0.1.0\r\nuptime_in_seconds:{uptime}\r\n\r\n\
                 # Clients\r\nconnected_clients:{}\r\ntotal_connections:{}\r\n\r\n\
                 # Stats\r\ntotal_commands:{}\r\n\
                 cmd_ping:{}\r\ncmd_get:{}\r\ncmd_set:{}\r\n\
                 cmd_del:{}\r\ncmd_incr:{}\r\ncmd_info:{}\r\ncmd_dbsize:{}\r\n\
                 keyspace_hits:{}\r\nkeyspace_misses:{}\r\nerrors_total:{}\r\n\r\n\
                 # Keyspace\r\ndb0:keys={}\r\n",
                m.connections_active
                    .load(std::sync::atomic::Ordering::Relaxed),
                m.connections_total
                    .load(std::sync::atomic::Ordering::Relaxed),
                m.commands_total.load(std::sync::atomic::Ordering::Relaxed),
                m.command_count(crate::observability::metrics::CommandType::Ping),
                m.command_count(crate::observability::metrics::CommandType::Get),
                m.command_count(crate::observability::metrics::CommandType::Set),
                m.command_count(crate::observability::metrics::CommandType::Del),
                m.command_count(crate::observability::metrics::CommandType::Incr),
                m.command_count(crate::observability::metrics::CommandType::Info),
                m.command_count(crate::observability::metrics::CommandType::DbSize),
                m.keyspace_hits.load(std::sync::atomic::Ordering::Relaxed),
                m.keyspace_misses.load(std::sync::atomic::Ordering::Relaxed),
                m.errors_total.load(std::sync::atomic::Ordering::Relaxed),
                engine.len(),
            );
            if is_resp {
                buf.extend_from_slice(b"$");
                let mut itoa_buf = itoa::Buffer::new();
                buf.extend_from_slice(itoa_buf.format(info.len()).as_bytes());
                buf.extend_from_slice(b"\r\n");
                buf.extend_from_slice(info.as_bytes());
                buf.extend_from_slice(b"\r\n");
            } else {
                buf.extend_from_slice(b"+");
                buf.extend_from_slice(info.as_bytes());
                buf.extend_from_slice(b"\r\n");
            }
        }
        ParsedCommand::DbSize => {
            buf.extend_from_slice(b":");
            let mut itoa_buf = itoa::Buffer::new();
            buf.extend_from_slice(itoa_buf.format(engine.len()).as_bytes());
            buf.extend_from_slice(b"\r\n");
        }
        ParsedCommand::Exists { key } => match engine.exists(std::str::from_utf8(key).unwrap_or("")) {
            true => {
                buf.extend_from_slice(b":");
                let mut itoa_buf = itoa::Buffer::new();
                buf.extend_from_slice(itoa_buf.format(1).as_bytes());
                buf.extend_from_slice(b"\r\n");
            }
            false => {
                buf.extend_from_slice(b":");
                let mut itoa_buf = itoa::Buffer::new();
                buf.extend_from_slice(itoa_buf.format(0).as_bytes());
                buf.extend_from_slice(b"\r\n");
            }
        },
        ParsedCommand::Ttl { key } => match engine.ttl_secs(std::str::from_utf8(key).unwrap_or("")) {
            -2 => buf.extend_from_slice(b":-2\r\n"),
            -1 => buf.extend_from_slice(b":-1\r\n"),
            ttl => {
                buf.extend_from_slice(b":");
                let mut itoa_buf = itoa::Buffer::new();
                buf.extend_from_slice(itoa_buf.format(ttl).as_bytes());
                buf.extend_from_slice(b"\r\n");
            }
        },
        ParsedCommand::Expire { key, seconds } => {
            match engine.expire(std::str::from_utf8(key).unwrap_or(""), seconds) {
                true => buf.extend_from_slice(b":1\r\n"),
                false => buf.extend_from_slice(b":0\r\n"),
            }
        }
        ParsedCommand::FlushDb => {
            engine.clear();
            buf.extend_from_slice(b"+OK\r\n");
        }
        ParsedCommand::Keys => {
            let keys = engine.keys();
            if is_resp {
                buf.extend_from_slice(b"*");
                let mut itoa_buf = itoa::Buffer::new();
                buf.extend_from_slice(itoa_buf.format(keys.len()).as_bytes());
                buf.extend_from_slice(b"\r\n");
                for key in keys {
                    buf.extend_from_slice(b"$");
                    let mut itoa_buf = itoa::Buffer::new();
                    buf.extend_from_slice(itoa_buf.format(key.len()).as_bytes());
                    buf.extend_from_slice(b"\r\n");
                    buf.extend_from_slice(key.as_bytes());
                    buf.extend_from_slice(b"\r\n");
                }
            } else {
                // Inline: space-separated keys
                for (i, key) in keys.iter().enumerate() {
                    if i > 0 {
                        buf.extend_from_slice(b" ");
                    }
                    buf.extend_from_slice(key.as_bytes());
                }
                buf.extend_from_slice(b"\r\n");
            }
        }
        ParsedCommand::Mget { keys } => {
            let mut values = Vec::with_capacity(keys.len());
            for key in keys {
                match engine.get(std::str::from_utf8(key).unwrap_or("")) {
                    Ok(Some(entry)) => values.push(Some(entry.data)),
                    _ => values.push(None),
                }
            }
            if is_resp {
                buf.extend_from_slice(b"*");
                let mut itoa_buf = itoa::Buffer::new();
                buf.extend_from_slice(itoa_buf.format(values.len()).as_bytes());
                buf.extend_from_slice(b"\r\n");
                for value in values {
                    match value {
                        Some(data) => {
                            buf.extend_from_slice(b"$");
                            let mut itoa_buf = itoa::Buffer::new();
                            buf.extend_from_slice(itoa_buf.format(data.len()).as_bytes());
                            buf.extend_from_slice(b"\r\n");
                            buf.extend_from_slice(&data);
                            buf.extend_from_slice(b"\r\n");
                        }
                        None => buf.extend_from_slice(b"$-1\r\n"),
                    }
                }
            } else {
                // Inline: space-separated values
                for (i, value) in values.iter().enumerate() {
                    if i > 0 {
                        buf.extend_from_slice(b" ");
                    }
                    match value {
                        Some(data) => buf.extend_from_slice(data),
                        None => buf.extend_from_slice(b"(nil)"),
                    }
                }
                buf.extend_from_slice(b"\r\n");
            }
        }
        ParsedCommand::Mset { pairs } => {
            for (key, value) in pairs {
                let key_str = std::str::from_utf8(key).unwrap_or("");
                let entry = crate::domain::value::ValueEntry::new(bytes::Bytes::copy_from_slice(value));
                let _ = engine.set(key_str.into(), entry);
            }
            buf.extend_from_slice(b"+OK\r\n");
        }
    }
}
