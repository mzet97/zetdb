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
    peer: SocketAddr,
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
    log::info!("client connected: {peer}");

    // Fast path: no timeout overhead when write_timeout is 0
    let use_write_timeout = write_timeout.as_secs() > 0;

    loop {
        // Read data into buffer
        match tokio::time::timeout(read_timeout, reader.read_buf(&mut read_buf)).await {
            Ok(Ok(0)) => break,
            Ok(Ok(_)) => {}
            Ok(Err(e)) => {
                log::warn!("{peer}: read error: {e}");
                break;
            }
            Err(_) => {
                log::info!("{peer}: read timeout, closing connection");
                break;
            }
        };

        // Safety: prevent OOM from misbehaving clients
        if read_buf.len() > MAX_READ_BUF {
            log::warn!("{peer}: read buffer overflow, closing");
            break;
        }

        // Pre-allocate write buffer: estimate ~32 bytes per response for inline protocol
        // This avoids repeated allocations during the hot loop
        let estimated_lines = memchr::memchr_iter(b'\n', &read_buf).count();
        if estimated_lines > 0 {
            write_buf.reserve(estimated_lines * 32);
        }

        // Detect protocol once per batch: RESP starts with '*', inline doesn't
        // This is done once per read batch, not per command
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
                            if let Err(e) = aof_writer.append_raw(&entry).await {
                                log::warn!("{peer}: aof write failed: {e}");
                            }
                        }
                        
                        write_response(response, &mut write_buf, true);
                    }
                    Ok(FrameResult::Skip { consumed }) => {
                        read_buf.advance(consumed);
                    }
                    Ok(FrameResult::Incomplete) => break,
                    Err(ParseError::UnknownCommand(cmd)) => {
                        log::warn!("{peer}: unknown command '{cmd}'");
                        skip_to_newline(&mut read_buf);
                        Response::Error(ResponseError::UnknownCommand(cmd)).write_to_resp(&mut write_buf);
                    }
                    Err(ParseError::SyntaxError(msg)) => {
                        log::warn!("{peer}: syntax error: {msg}");
                        skip_to_newline(&mut read_buf);
                        Response::Error(ResponseError::SyntaxError(msg)).write_to_resp(&mut write_buf);
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
                            if let Err(e) = aof_writer.append_raw(&entry).await {
                                log::warn!("{peer}: aof write failed: {e}");
                            }
                        }
                        
                        response.write_to(&mut write_buf);
                    }
                    Ok(FrameResult::Skip { consumed }) => {
                        read_buf.advance(consumed);
                    }
                    Ok(FrameResult::Incomplete) => break,
                    Err(ParseError::UnknownCommand(cmd)) => {
                        log::warn!("{peer}: unknown command '{cmd}'");
                        skip_to_newline(&mut read_buf);
                        Response::Error(ResponseError::UnknownCommand(cmd)).write_to(&mut write_buf);
                    }
                    Err(ParseError::SyntaxError(msg)) => {
                        log::warn!("{peer}: syntax error: {msg}");
                        skip_to_newline(&mut read_buf);
                        Response::Error(ResponseError::SyntaxError(msg)).write_to(&mut write_buf);
                    }
                    Err(ParseError::EmptyCommand) => break,
                }
            }
        }

        // Safety: prevent OOM from misbehaving clients that don't consume responses
        if write_buf.len() > MAX_WRITE_BUF {
            log::warn!("{peer}: write buffer overflow, closing");
            break;
        }

        // Flush all accumulated responses at once
        if !write_buf.is_empty() {
            if use_write_timeout {
                // Slow path: with timeout protection
                match tokio::time::timeout(write_timeout, writer.write_all(&write_buf)).await {
                    Ok(Ok(())) => write_buf.clear(),
                    Ok(Err(e)) => {
                        log::warn!("{peer}: write error: {e}");
                        break;
                    }
                    Err(_) => {
                        log::warn!("{peer}: write timeout, closing connection");
                        break;
                    }
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

    log::info!("client disconnected: {peer}");
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