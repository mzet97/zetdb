use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use bytes::{Buf, BytesMut};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

use crate::application::dispatcher::dispatch;
use crate::observability::metrics;
use crate::protocol::parser::{try_parse_frame, try_parse_frame_inline, FrameResult, ParseError};
use crate::protocol::response::{Response, ResponseError};
use crate::storage::aof::AofWriter;
use crate::storage::engine::KvEngine;

const MAX_READ_BUF: usize = 1024 * 1024; // 1MB safety limit
const MAX_WRITE_BUF: usize = 1024 * 1024; // 1MB safety limit
const INITIAL_BUF: usize = 16384; // 16KB initial buffer

/// Write response to buffer using the appropriate protocol.
/// This is a hot path - must be inlined.
#[inline(always)]
fn write_response(response: Response, buf: &mut BytesMut, is_resp: bool) {
    if is_resp {
        response.write_to_resp(buf);
    } else {
        response.write_to(buf);
    }
}

pub async fn handle_session(
    stream: TcpStream,
    _peer: SocketAddr,
    engine: Arc<dyn KvEngine>,
    read_timeout: Duration,
    write_timeout: Duration,
    aof: Option<Arc<AofWriter>>,
    metrics_enabled: bool,
) {
    stream.set_nodelay(true).ok();

    let (mut reader, mut writer) = stream.into_split();

    let mut read_buf = BytesMut::with_capacity(INITIAL_BUF);
    let mut write_buf = BytesMut::with_capacity(INITIAL_BUF);

    if metrics_enabled {
        metrics::metrics().connection_opened();
    }

    // Fast path: no timeout overhead when write_timeout is 0
    let use_write_timeout = write_timeout.as_secs() > 0;

    loop {
        // Read data into buffer
        match tokio::time::timeout(read_timeout, reader.read_buf(&mut read_buf)).await {
            Ok(Ok(0)) => break,
            Ok(Ok(_)) => {}
            Ok(Err(_)) => break,
            Err(_) => break,
        };

        // Safety: prevent OOM from misbehaving clients
        if read_buf.len() > MAX_READ_BUF {
            break;
        }

        // Pre-allocate write buffer: simple heuristic based on read buffer size
        // Avoids scanning entire buffer for newlines (expensive in hot path)
        let spare = write_buf.capacity().saturating_sub(write_buf.len());
        let needed_hint = (read_buf.len() / 2).min(64 * 1024);
        if spare < needed_hint {
            write_buf.reserve(needed_hint - spare);
        }

        // Detect protocol once per batch: RESP starts with '*', inline doesn't
        let is_resp = read_buf.first() == Some(&b'*');

        // Process ALL complete frames in the buffer (inline or RESP)
        if is_resp {
            // RESP protocol path (slower, more complex)
            loop {
                match try_parse_frame(&read_buf) {
                    Ok(FrameResult::Complete { consumed, command }) => {
                        read_buf.advance(consumed);
                        
                        // Only compute AOF entry if AOF is actually enabled
                        let aof_entry = if aof.is_some() && command.is_write() {
                            command.to_aof_entry()
                        } else {
                            None
                        };
                        
                        let cmd_type = if metrics_enabled {
                            Some(command.command_type())
                        } else {
                            None
                        };
                        
                        let response = dispatch(engine.as_ref(), command);
                        
                        if let Some(ct) = cmd_type {
                            let m = metrics::metrics();
                            m.record_command(ct);
                            if matches!(ct, metrics::CommandType::Get) {
                                if matches!(&response, Response::Value(Some(_))) {
                                    m.record_hit();
                                } else if matches!(&response, Response::Value(None)) {
                                    m.record_miss();
                                }
                            }
                            if matches!(&response, Response::Error(_)) {
                                m.record_error();
                            }
                        }
                        
                        if let (Some(entry), Some(ref aof_writer), true) =
                            (aof_entry, &aof, response.is_success())
                        {
                            if aof_writer.append_raw(&entry).await.is_err() {
                                // Silently ignore AOF errors in production
                            }
                        }
                        
                        write_response(response, &mut write_buf, true);
                    }
                    Ok(FrameResult::Skip { consumed }) => {
                        read_buf.advance(consumed);
                    }
                    Ok(FrameResult::Incomplete) => break,
                    Err(ParseError::SyntaxError(_)) => {
                        skip_to_newline(&mut read_buf);
                        Response::Error(ResponseError::SyntaxError("syntax error".to_string())).write_to_resp(&mut write_buf);
                    }
                    Err(ParseError::UnknownCommand(_)) => {
                        skip_to_newline(&mut read_buf);
                        Response::Error(ResponseError::UnknownCommand("unknown command".to_string())).write_to_resp(&mut write_buf);
                    }
                    Err(ParseError::EmptyCommand) => break,
                }
            }
        } else {
            // Inline protocol fast path (zero overhead)
            loop {
                match try_parse_frame_inline(&read_buf) {
                    Ok(FrameResult::Complete { consumed, command }) => {
                        read_buf.advance(consumed);
                        
                        // Only compute AOF entry if AOF is actually enabled
                        let aof_entry = if aof.is_some() && command.is_write() {
                            command.to_aof_entry()
                        } else {
                            None
                        };
                        
                        let cmd_type = if metrics_enabled {
                            Some(command.command_type())
                        } else {
                            None
                        };
                        
                        let response = dispatch(engine.as_ref(), command);
                        
                        if let Some(ct) = cmd_type {
                            let m = metrics::metrics();
                            m.record_command(ct);
                            if matches!(ct, metrics::CommandType::Get) {
                                if matches!(&response, Response::Value(Some(_))) {
                                    m.record_hit();
                                } else if matches!(&response, Response::Value(None)) {
                                    m.record_miss();
                                }
                            }
                            if matches!(&response, Response::Error(_)) {
                                m.record_error();
                            }
                        }
                        
                        if let (Some(entry), Some(ref aof_writer), true) =
                            (aof_entry, &aof, response.is_success())
                        {
                            if aof_writer.append_raw(&entry).await.is_err() {
                                // Silently ignore AOF errors in production
                            }
                        }
                        
                        response.write_to(&mut write_buf);
                    }
                    Ok(FrameResult::Skip { consumed }) => {
                        read_buf.advance(consumed);
                    }
                    Ok(FrameResult::Incomplete) => break,
                    Err(ParseError::SyntaxError(_)) => {
                        skip_to_newline(&mut read_buf);
                        Response::Error(ResponseError::SyntaxError("syntax error".to_string())).write_to(&mut write_buf);
                    }
                    Err(ParseError::UnknownCommand(_)) => {
                        skip_to_newline(&mut read_buf);
                        Response::Error(ResponseError::UnknownCommand("unknown command".to_string())).write_to(&mut write_buf);
                    }
                    Err(ParseError::EmptyCommand) => break,
                }
            }
        }

        // Safety: prevent OOM from misbehaving clients that don't consume responses
        if write_buf.len() > MAX_WRITE_BUF {
            break;
        }

        // Flush all accumulated responses at once
        if !write_buf.is_empty() {
            if use_write_timeout {
                // Slow path: with timeout protection
                match tokio::time::timeout(write_timeout, writer.write_all(&write_buf)).await {
                    Ok(Ok(())) => write_buf.clear(),
                    Ok(Err(_)) => break,
                    Err(_) => break,
                }
            } else {
                // Fast path: zero timeout overhead
                if writer.write_all(&write_buf).await.is_err() {
                    break;
                }
                write_buf.clear();
            }
        }
    }

    if metrics_enabled {
        metrics::metrics().connection_closed();
    }
}

/// Skip buffer past the next newline for error recovery.
#[inline]
fn skip_to_newline(buf: &mut BytesMut) {
    match memchr::memchr(b'\n', buf) {
        Some(pos) => buf.advance(pos + 1),
        None => buf.clear(),
    }
}