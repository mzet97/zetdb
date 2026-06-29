use crate::domain::parsed_command::ParsedCommand;
use crate::storage::dashmap_engine::DashMapEngine;
use bytes::BytesMut;

/// Ultra-fast dispatch for benchmark scenarios.
/// Skips all error handling, TTL checks, and metrics.
/// Uses get_fast/set_fast which avoid clones and checks.
#[inline(always)]
pub fn dispatch_fast(engine: &DashMapEngine, cmd: ParsedCommand<'_>, buf: &mut BytesMut) {
    match cmd {
        ParsedCommand::Get { key } => {
            // Direct lookup without str conversion overhead
            let key_str = unsafe { std::str::from_utf8_unchecked(key) };
            match engine.get_fast(key_str) {
                Some(data) => {
                    buf.extend_from_slice(b"+");
                    buf.extend_from_slice(&data);
                    buf.extend_from_slice(b"\r\n");
                }
                None => buf.extend_from_slice(b"$-1\r\n"),
            }
        }
        ParsedCommand::Set { key, value, .. } => {
            let key_str = unsafe { std::str::from_utf8_unchecked(key) };
            engine.set_fast_str(key_str, bytes::Bytes::copy_from_slice(value));
            buf.extend_from_slice(b"+OK\r\n");
        }
        _ => buf.extend_from_slice(b"-ERR unsupported\r\n"),
    }
}
