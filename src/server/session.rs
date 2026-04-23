use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use bytes::BytesMut;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

use crate::application::dispatcher::dispatch;
use crate::protocol::parser::{parse_bytes, ParseError};
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

        // Pre-allocate write buffer: estimate ~32 bytes per response
        let estimated_lines = count_newlines(&read_buf);
        if estimated_lines > 0 {
            write_buf.reserve(estimated_lines * 32);
        }

        // Process ALL complete lines in the buffer using memchr for fast scanning
        while let Some(pos) = memchr::memchr(b'\n', &read_buf) {
            let mut line = read_buf.split_to(pos + 1);
            let line = trim_trailing(&mut line);

            if line.is_empty() {
                continue;
            }

            let response = match parse_bytes(line) {
                Ok(cmd) => {
                    log::debug!("{peer}: {cmd:?}");
                    dispatch(engine.as_ref(), cmd)
                }
                Err(ParseError::EmptyCommand) => continue,
                Err(ParseError::UnknownCommand(cmd)) => {
                    log::warn!("{peer}: unknown command '{cmd}'");
                    Response::Error(ResponseError::UnknownCommand(cmd))
                }
                Err(ParseError::SyntaxError(msg)) => {
                    log::warn!("{peer}: syntax error: {msg}");
                    Response::Error(ResponseError::SyntaxError(msg))
                }
            };

            response.write_to(&mut write_buf);
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

fn count_newlines(buf: &[u8]) -> usize {
    bytecount(buf, b'\n')
}

fn bytecount(haystack: &[u8], needle: u8) -> usize {
    // Fast count using memchr iterator
    memchr::memchr_iter(needle, haystack).count()
}

fn trim_trailing(buf: &mut [u8]) -> &[u8] {
    let mut end = buf.len();
    while end > 0 && (buf[end - 1] == b'\n' || buf[end - 1] == b'\r' || buf[end - 1] == b' ') {
        end -= 1;
    }
    &buf[..end]
}
