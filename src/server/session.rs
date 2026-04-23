use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use bytes::{Buf, BytesMut};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

use crate::application::dispatcher::dispatch;
use crate::protocol::parser::{try_parse_frame, FrameResult, ParseError};
use crate::protocol::response::{Response, ResponseError};
use crate::storage::engine::KvEngine;

const MAX_READ_BUF: usize = 1024 * 1024; // 1MB safety limit
const INITIAL_BUF: usize = 16384;         // 16KB initial buffer

pub async fn handle_session(
    stream: TcpStream,
    peer: SocketAddr,
    engine: Arc<dyn KvEngine>,
    read_timeout: Duration,
) {
    stream.set_nodelay(true).ok();

    let (mut reader, mut writer) = stream.into_split();

    let mut read_buf = BytesMut::with_capacity(INITIAL_BUF);
    let mut write_buf = BytesMut::with_capacity(INITIAL_BUF);

    log::info!("client connected: {peer}");

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

        // Process ALL complete frames in the buffer (inline or RESP)
        loop {
            match try_parse_frame(&read_buf) {
                Ok(FrameResult::Complete { consumed, command }) => {
                    read_buf.advance(consumed);
                    log::debug!("{peer}: {command:?}");
                    dispatch(engine.as_ref(), command).write_to(&mut write_buf);
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

        // Flush all accumulated responses at once
        if !write_buf.is_empty() {
            if writer.write_all(&write_buf).await.is_err() {
                break;
            }
            write_buf.clear();
        }
    }

    log::info!("client disconnected: {peer}");
}

/// Skip buffer past the next newline for error recovery.
fn skip_to_newline(buf: &mut BytesMut) {
    match memchr::memchr(b'\n', buf) {
        Some(pos) => buf.advance(pos + 1),
        None => buf.clear(),
    }
}
