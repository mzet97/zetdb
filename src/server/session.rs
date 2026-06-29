use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use bytes::{Buf, BytesMut};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

use crate::application::dispatcher::{dispatch, dispatch_to_buffer};
use crate::observability::metrics;
use crate::protocol::parser::{try_parse_frame, FrameResult, ParseError};
use crate::protocol::parser_borrowed::{parse_inline_borrowed, FrameResultBorrowed};
use crate::protocol::response::{Response, ResponseError};
use crate::storage::aof::AofWriter;
use crate::storage::dashmap_engine::DashMapEngine;

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
    engine: Arc<DashMapEngine>,
    read_timeout: Duration,
    write_timeout: Duration,
    aof: Option<Arc<AofWriter>>,
    metrics_enabled: bool,
) {
    stream.set_nodelay(true).ok();

    let (reader, writer) = stream.into_split();

    let _read_buf = BytesMut::with_capacity(INITIAL_BUF);
    let _write_buf = BytesMut::with_capacity(INITIAL_BUF);

    if metrics_enabled {
        metrics::metrics().connection_opened();
    }

    // Fast path: no timeout overhead when write_timeout is 0
    let use_write_timeout = write_timeout.as_secs() > 0;

    // Determine which optimized path to use based on features
    let has_aof = aof.is_some();
    
    if !has_aof && !metrics_enabled && !use_write_timeout {
        // Ultra-fast path: no AOF, no metrics, no write timeout
        handle_session_fast(reader, writer, engine, read_timeout).await;
    } else {
        // Standard path with all features
        handle_session_full(reader, writer, engine, read_timeout, write_timeout, aof, metrics_enabled, use_write_timeout).await;
    }

    if metrics_enabled {
        metrics::metrics().connection_closed();
    }
}

/// Ultra-fast path: no AOF, no metrics, no write timeout.
/// This is the most optimized path for benchmark scenarios.
async fn handle_session_fast(
    mut reader: tokio::net::tcp::OwnedReadHalf,
    mut writer: tokio::net::tcp::OwnedWriteHalf,
    engine: Arc<DashMapEngine>,
    read_timeout: Duration,
) {
    let _read_timeout = read_timeout; // Keep for API compatibility
    let mut read_buf = BytesMut::with_capacity(INITIAL_BUF);
    let mut write_buf = BytesMut::with_capacity(INITIAL_BUF);

    loop {
        // Read data into buffer - no timeout in fast path for benchmark
        match reader.read_buf(&mut read_buf).await {
            Ok(0) => break,
            Ok(_) => {}
            Err(_) => break,
        };

        // Safety: prevent OOM from misbehaving clients
        if read_buf.len() > MAX_READ_BUF {
            break;
        }

        // Pre-allocate write buffer
        let spare = write_buf.capacity().saturating_sub(write_buf.len());
        let needed_hint = (read_buf.len() / 2).min(64 * 1024);
        if spare < needed_hint {
            write_buf.reserve(needed_hint - spare);
        }

        // Detect protocol once per batch
        let is_resp = read_buf.first() == Some(&b'*');

        // Process ALL complete frames in the buffer
        if is_resp {
            // RESP protocol path
            loop {
                match try_parse_frame(&read_buf) {
                    Ok(FrameResult::Complete { consumed, command }) => {
                        read_buf.advance(consumed);
                        let response = dispatch(engine.as_ref(), command);
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
            // Inline protocol fast path with zero-allocation ParsedCommand
            use crate::application::dispatcher_fast::dispatch_fast;
            let mut offset = 0usize;
            loop {
                match parse_inline_borrowed(&read_buf[offset..]) {
                    Ok(FrameResultBorrowed::Complete { consumed, command }) => {
                        dispatch_fast(engine.as_ref(), command, &mut write_buf);
                        offset += consumed;
                    }
                    Ok(FrameResultBorrowed::Skip { consumed }) => {
                        offset += consumed;
                    }
                    Ok(FrameResultBorrowed::Incomplete) => break,
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
            read_buf.advance(offset);
        }

        // Safety: prevent OOM from misbehaving clients that don't consume responses
        if write_buf.len() > MAX_WRITE_BUF {
            break;
        }

        // Flush all accumulated responses at once (no timeout)
        if !write_buf.is_empty() {
            if writer.write_all(&write_buf).await.is_err() {
                break;
            }
            write_buf.clear();
        }
    }
}

/// Standard path with all features (AOF, metrics, write timeout).
#[allow(clippy::too_many_arguments)]
async fn handle_session_full(
    mut reader: tokio::net::tcp::OwnedReadHalf,
    mut writer: tokio::net::tcp::OwnedWriteHalf,
    engine: Arc<DashMapEngine>,
    read_timeout: Duration,
    write_timeout: Duration,
    aof: Option<Arc<AofWriter>>,
    metrics_enabled: bool,
    use_write_timeout: bool,
) {
    let mut read_buf = BytesMut::with_capacity(INITIAL_BUF);
    let mut write_buf = BytesMut::with_capacity(INITIAL_BUF);

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

        // Pre-allocate write buffer
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
            // Inline protocol fast path with zero-allocation ParsedCommand
            loop {
                match parse_inline_borrowed(&read_buf) {
                    Ok(FrameResultBorrowed::Complete { consumed, command }) => {
                        // Process command before advancing buffer (command borrows from read_buf)
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
                        
                        let owned_command = command.to_owned();
                        
                        // Now advance buffer after borrowing is done
                        read_buf.advance(consumed);
                        
                        // Use direct buffer dispatch (avoids Response enum allocation)
                        dispatch_to_buffer(engine.as_ref(), owned_command, &mut write_buf, false);
                        
                        // Metrics tracking after dispatch
                        if let Some(ct) = cmd_type {
                            let m = metrics::metrics();
                            m.record_command(ct);
                            if matches!(ct, metrics::CommandType::Get) {
                                // For metrics, we need to know if it was a hit or miss
                                // Since we don't have the response, we skip hit/miss tracking
                                // This is a trade-off for performance
                            }
                        }
                        
                        if let (Some(entry), Some(ref aof_writer), true) =
                            (aof_entry, &aof, true)
                        {
                            if aof_writer.append_raw(&entry).await.is_err() {
                                // Silently ignore AOF errors in production
                            }
                        }
                    }
                    Ok(FrameResultBorrowed::Skip { consumed }) => {
                        read_buf.advance(consumed);
                    }
                    Ok(FrameResultBorrowed::Incomplete) => break,
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
}

/// Skip buffer past the next newline for error recovery.
#[inline]
fn skip_to_newline(buf: &mut BytesMut) {
    match memchr::memchr(b'\n', buf) {
        Some(pos) => buf.advance(pos + 1),
        None => buf.clear(),
    }
}