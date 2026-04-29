use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::io::AsyncWriteExt;
use tokio::net::TcpListener;

use crate::config::Config;
use crate::server::session::handle_session;
use crate::storage::aof::AofWriter;
use crate::storage::engine::KvEngine;

const DRAIN_TIMEOUT: Duration = Duration::from_secs(5);
const DRAIN_POLL_INTERVAL: Duration = Duration::from_millis(50);

/// Run the TCP server with a pre-bound listener and graceful shutdown.
pub async fn run_server_with_listener(
    listener: TcpListener,
    engine: Arc<dyn KvEngine>,
    aof: Option<Arc<AofWriter>>,
    mut shutdown_rx: tokio::sync::watch::Receiver<bool>,
    read_timeout: Duration,
    max_conns: usize,
    metrics_enabled: bool,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let local_addr = listener.local_addr()?;
    let active_conns = Arc::new(AtomicUsize::new(0));

    if max_conns > 0 {
        log::info!("max connections: {max_conns}");
    }

    log::info!("ZetDB listening on {local_addr}");
    log::info!("read timeout: {read_timeout:?}");

    loop {
        tokio::select! {
            accept_result = listener.accept() => {
                let (mut stream, peer) = accept_result?;

                // Enforce connection limit (atomic check — slight raciness is acceptable)
                if max_conns > 0 && active_conns.load(Ordering::Relaxed) >= max_conns {
                    log::warn!("{peer}: rejected — max connections reached");
                    let _ = stream.write_all(b"-ERR max connections reached\r\n").await;
                    continue;
                }

                let engine = engine.clone();
                let aof = aof.clone();
                let conns = active_conns.clone();
                conns.fetch_add(1, Ordering::Relaxed);
                tokio::spawn(async move {
                    handle_session(stream, peer, engine, read_timeout, aof, metrics_enabled).await;
                    conns.fetch_sub(1, Ordering::Relaxed);
                });
            }
            _ = shutdown_rx.changed() => {
                log::info!("shutdown signal received, draining connections...");
                break;
            }
        }
    }

    // Wait for in-flight connections to finish
    let start = Instant::now();
    loop {
        let remaining = active_conns.load(Ordering::Relaxed);
        if remaining == 0 {
            break;
        }
        if start.elapsed() > DRAIN_TIMEOUT {
            log::warn!("drain timeout, {remaining} connections still active");
            break;
        }
        tokio::time::sleep(DRAIN_POLL_INTERVAL).await;
    }
    log::info!("all connections drained, server stopped");
    Ok(())
}

/// Run the TCP server with graceful shutdown support.
/// Returns when the shutdown signal is received and all connections have drained.
pub async fn run_server_with_shutdown(
    config: Config,
    engine: Arc<dyn KvEngine>,
    aof: Option<Arc<AofWriter>>,
    shutdown_rx: tokio::sync::watch::Receiver<bool>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let listener = TcpListener::bind((config.bind_addr.as_str(), config.port)).await?;
    run_server_with_listener(
        listener,
        engine,
        aof,
        shutdown_rx,
        config.read_timeout(),
        config.max_connections,
        config.metrics_enabled,
    )
    .await
}

/// Convenience wrapper for tests — runs until aborted or connection error.
pub async fn run_server(
    config: Config,
    engine: Arc<dyn KvEngine>,
    aof: Option<Arc<AofWriter>>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let (_tx, rx) = tokio::sync::watch::channel(false);
    run_server_with_shutdown(config, engine, aof, rx).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::dashmap_engine::DashMapEngine;
    use std::time::Duration;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

    fn find_available_port() -> u16 {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.local_addr().unwrap().port()
    }

    fn test_config(port: u16) -> Config {
        Config {
            bind_addr: "127.0.0.1".into(),
            port,
            ..Default::default()
        }
    }

    struct TestClient {
        writer: tokio::io::WriteHalf<tokio::net::TcpStream>,
        reader: BufReader<tokio::io::ReadHalf<tokio::net::TcpStream>>,
    }

    impl TestClient {
        async fn connect(addr: &str) -> Self {
            let stream = tokio::net::TcpStream::connect(addr).await.unwrap();
            let (reader, writer) = tokio::io::split(stream);
            Self {
                writer,
                reader: BufReader::new(reader),
            }
        }

        async fn command(&mut self, cmd: &str) -> String {
            self.writer
                .write_all(format!("{cmd}\r\n").as_bytes())
                .await
                .unwrap();
            self.writer.flush().await.unwrap();
            let mut line = String::new();
            self.reader.read_line(&mut line).await.unwrap();
            line
        }
    }

    async fn start_server(port: u16) -> tokio::task::JoinHandle<()> {
        let config = test_config(port);
        let engine = Arc::new(DashMapEngine::new());
        let handle = tokio::spawn(async move {
            let _ = run_server(config, engine, None).await;
        });
        tokio::time::sleep(Duration::from_millis(50)).await;
        handle
    }

    #[tokio::test]
    async fn ping() {
        let port = find_available_port();
        let server = start_server(port).await;
        let mut client = TestClient::connect(&format!("127.0.0.1:{port}")).await;

        let resp = client.command("PING").await;
        assert_eq!(resp, "+PONG\r\n");

        server.abort();
    }

    #[tokio::test]
    async fn set_get_del() {
        let port = find_available_port();
        let server = start_server(port).await;
        let mut client = TestClient::connect(&format!("127.0.0.1:{port}")).await;

        assert_eq!(client.command("SET mykey hello").await, "+OK\r\n");
        assert_eq!(client.command("GET mykey").await, "+hello\r\n");
        assert_eq!(client.command("DEL mykey").await, ":1\r\n");
        assert_eq!(client.command("GET mykey").await, "$-1\r\n");
        assert_eq!(client.command("DEL mykey").await, ":0\r\n");

        server.abort();
    }

    #[tokio::test]
    async fn incr() {
        let port = find_available_port();
        let server = start_server(port).await;
        let mut client = TestClient::connect(&format!("127.0.0.1:{port}")).await;

        assert_eq!(client.command("INCR counter").await, ":1\r\n");
        assert_eq!(client.command("INCR counter").await, ":2\r\n");
        assert_eq!(client.command("INCR counter").await, ":3\r\n");

        server.abort();
    }

    #[tokio::test]
    async fn unknown_command() {
        let port = find_available_port();
        let server = start_server(port).await;
        let mut client = TestClient::connect(&format!("127.0.0.1:{port}")).await;

        let resp = client.command("FOOBAR").await;
        assert!(resp.starts_with("-ERR unknown command"));

        server.abort();
    }

    #[tokio::test]
    async fn syntax_error() {
        let port = find_available_port();
        let server = start_server(port).await;
        let mut client = TestClient::connect(&format!("127.0.0.1:{port}")).await;

        let resp = client.command("GET").await;
        assert!(resp.starts_with("-ERR syntax"));

        server.abort();
    }

    #[tokio::test]
    async fn multiple_clients() {
        let port = find_available_port();
        let server = start_server(port).await;

        let mut c1 = TestClient::connect(&format!("127.0.0.1:{port}")).await;
        let mut c2 = TestClient::connect(&format!("127.0.0.1:{port}")).await;

        c1.command("SET shared value").await;
        let resp = c2.command("GET shared").await;
        assert_eq!(resp, "+value\r\n");

        assert_eq!(c1.command("PING").await, "+PONG\r\n");
        assert_eq!(c2.command("PING").await, "+PONG\r\n");

        server.abort();
    }

    #[tokio::test]
    async fn incr_not_integer() {
        let port = find_available_port();
        let server = start_server(port).await;
        let mut client = TestClient::connect(&format!("127.0.0.1:{port}")).await;

        client.command("SET text hello").await;
        let resp = client.command("INCR text").await;
        assert!(resp.starts_with("-ERR type"));

        server.abort();
    }

    #[tokio::test]
    async fn read_timeout_disconnects_idle_client() {
        let port = find_available_port();
        let config = Config {
            bind_addr: "127.0.0.1".into(),
            port,
            read_timeout_secs: 1,
            ..Default::default()
        };
        let engine = Arc::new(DashMapEngine::new());
        let server = tokio::spawn(async move {
            let _ = run_server(config, engine, None).await;
        });
        tokio::time::sleep(Duration::from_millis(50)).await;

        let mut client = TestClient::connect(&format!("127.0.0.1:{port}")).await;
        assert_eq!(client.command("PING").await, "+PONG\r\n");

        // Wait for timeout to trigger (1s timeout + margin)
        tokio::time::sleep(Duration::from_millis(1200)).await;

        // Server should have closed the connection — write or read fails
        let write_result = client.writer.write_all(b"PING\r\n").await;
        client.writer.flush().await.unwrap();
        if write_result.is_ok() {
            let mut line = String::new();
            let read_result = client.reader.read_line(&mut line).await;
            assert!(
                read_result.is_err() || read_result.unwrap() == 0,
                "Expected connection closed after timeout"
            );
        }

        server.abort();
    }

    // --- RESP integration tests ---

    /// Client that sends commands in RESP protocol format.
    struct RespTestClient {
        writer: tokio::io::WriteHalf<tokio::net::TcpStream>,
        reader: BufReader<tokio::io::ReadHalf<tokio::net::TcpStream>>,
    }

    impl RespTestClient {
        async fn connect(addr: &str) -> Self {
            let stream = tokio::net::TcpStream::connect(addr).await.unwrap();
            let (reader, writer) = tokio::io::split(stream);
            Self {
                writer,
                reader: BufReader::new(reader),
            }
        }

        /// Send a RESP array command and return the raw response (handles bulk strings).
        async fn resp_command(&mut self, args: &[&[u8]]) -> String {
            // Build RESP frame: *<count>\r\n$<len>\r\n<data>\r\n...
            let mut frame = Vec::new();
            frame.extend_from_slice(format!("*{}\r\n", args.len()).as_bytes());
            for arg in args {
                frame.extend_from_slice(format!("${}\r\n", arg.len()).as_bytes());
                frame.extend_from_slice(arg);
                frame.extend_from_slice(b"\r\n");
            }

            self.writer.write_all(&frame).await.unwrap();
            self.writer.flush().await.unwrap();

            self.read_resp_response().await
        }

        /// Read a complete RESP response (handles bulk strings, simple strings, integers, errors).
        async fn read_resp_response(&mut self) -> String {
            let mut first_line = String::new();
            self.reader.read_line(&mut first_line).await.unwrap();

            if first_line.starts_with('$') {
                // Bulk string: parse length, read data line
                let len_str = first_line.trim_end_matches("\r\n").trim_start_matches('$');
                if len_str == "-1" {
                    return first_line; // nil
                }
                let _len: usize = len_str.parse().unwrap();
                let mut data_line = String::new();
                self.reader.read_line(&mut data_line).await.unwrap();
                // Return the full raw response: first_line + data_line
                format!("{first_line}{data_line}")
            } else {
                // Simple string, integer, or error — single line
                first_line
            }
        }
    }

    #[tokio::test]
    async fn resp_ping() {
        let port = find_available_port();
        let server = start_server(port).await;
        let mut client = RespTestClient::connect(&format!("127.0.0.1:{port}")).await;

        let resp = client.resp_command(&[b"PING"]).await;
        assert_eq!(resp, "+PONG\r\n");

        server.abort();
    }

    #[tokio::test]
    async fn resp_set_get_del() {
        let port = find_available_port();
        let server = start_server(port).await;
        let mut client = RespTestClient::connect(&format!("127.0.0.1:{port}")).await;

        assert_eq!(
            client.resp_command(&[b"SET", b"mykey", b"hello"]).await,
            "+OK\r\n"
        );
        assert_eq!(
            client.resp_command(&[b"GET", b"mykey"]).await,
            "$5\r\nhello\r\n"
        );
        assert_eq!(client.resp_command(&[b"DEL", b"mykey"]).await, ":1\r\n");
        assert_eq!(client.resp_command(&[b"GET", b"mykey"]).await, "$-1\r\n");
        assert_eq!(client.resp_command(&[b"DEL", b"mykey"]).await, ":0\r\n");

        server.abort();
    }

    #[tokio::test]
    async fn resp_incr() {
        let port = find_available_port();
        let server = start_server(port).await;
        let mut client = RespTestClient::connect(&format!("127.0.0.1:{port}")).await;

        assert_eq!(client.resp_command(&[b"INCR", b"counter"]).await, ":1\r\n");
        assert_eq!(client.resp_command(&[b"INCR", b"counter"]).await, ":2\r\n");
        assert_eq!(client.resp_command(&[b"INCR", b"counter"]).await, ":3\r\n");

        server.abort();
    }

    #[tokio::test]
    async fn resp_case_insensitive() {
        let port = find_available_port();
        let server = start_server(port).await;
        let mut client = RespTestClient::connect(&format!("127.0.0.1:{port}")).await;

        assert_eq!(client.resp_command(&[b"ping"]).await, "+PONG\r\n");
        assert_eq!(client.resp_command(&[b"set", b"k", b"v"]).await, "+OK\r\n");
        assert_eq!(client.resp_command(&[b"get", b"k"]).await, "$1\r\nv\r\n");

        server.abort();
    }

    #[tokio::test]
    async fn resp_unknown_command() {
        let port = find_available_port();
        let server = start_server(port).await;
        let mut client = RespTestClient::connect(&format!("127.0.0.1:{port}")).await;

        let resp = client.resp_command(&[b"FOOBAR"]).await;
        assert!(resp.starts_with("-ERR unknown command"));

        server.abort();
    }

    #[tokio::test]
    async fn resp_wrong_arg_count() {
        let port = find_available_port();
        let server = start_server(port).await;
        let mut client = RespTestClient::connect(&format!("127.0.0.1:{port}")).await;

        let resp = client.resp_command(&[b"GET"]).await;
        assert!(resp.starts_with("-ERR syntax"));

        server.abort();
    }

    #[tokio::test]
    async fn resp_pipeline() {
        let port = find_available_port();
        let server = start_server(port).await;
        let mut client = RespTestClient::connect(&format!("127.0.0.1:{port}")).await;

        // Send multiple RESP commands in one write (pipeline)
        let mut batch = Vec::new();
        for i in 0..5 {
            batch.extend_from_slice(
                format!("*3\r\n$3\r\nSET\r\n$2\r\nk{}\r\n$2\r\nv{}\r\n", i, i).as_bytes(),
            );
        }
        batch.extend_from_slice(b"*1\r\n$4\r\nPING\r\n");
        client.writer.write_all(&batch).await.unwrap();
        client.writer.flush().await.unwrap();

        // Read 6 responses
        for _ in 0..5 {
            let mut line = String::new();
            client.reader.read_line(&mut line).await.unwrap();
            assert_eq!(line, "+OK\r\n");
        }
        let mut line = String::new();
        client.reader.read_line(&mut line).await.unwrap();
        assert_eq!(line, "+PONG\r\n");

        // Verify values
        assert_eq!(client.resp_command(&[b"GET", b"k0"]).await, "$2\r\nv0\r\n");
        assert_eq!(client.resp_command(&[b"GET", b"k4"]).await, "$2\r\nv4\r\n");

        server.abort();
    }

    #[tokio::test]
    async fn max_connections_limit() {
        let port = find_available_port();
        let config = Config {
            bind_addr: "127.0.0.1".into(),
            port,
            max_connections: 1,
            read_timeout_secs: 30,
            ..Default::default()
        };
        let engine = Arc::new(DashMapEngine::new());
        let server = tokio::spawn(async move {
            let _ = run_server(config, engine, None).await;
        });
        tokio::time::sleep(Duration::from_millis(50)).await;

        // First client takes the only slot
        let mut c1 = TestClient::connect(&format!("127.0.0.1:{port}")).await;
        assert_eq!(c1.command("PING").await, "+PONG\r\n");

        // Second client should be rejected — just read (server sends error before we write)
        let stream = tokio::net::TcpStream::connect(format!("127.0.0.1:{port}"))
            .await
            .unwrap();
        let (reader, _) = tokio::io::split(stream);
        let mut reader = BufReader::new(reader);
        let mut line = String::new();
        let read_result = reader.read_line(&mut line).await;
        assert!(
            line.starts_with("-ERR max connections")
                || read_result.is_err()
                || read_result.unwrap() == 0,
            "expected rejection, got: {line}"
        );

        server.abort();
    }

    #[tokio::test]
    async fn exists_ttl_expire() {
        let port = find_available_port();
        let server = start_server(port).await;
        let mut client = TestClient::connect(&format!("127.0.0.1:{port}")).await;

        assert_eq!(client.command("SET mykey hello").await, "+OK\r\n");
        assert_eq!(client.command("EXISTS mykey").await, ":1\r\n");
        assert_eq!(client.command("EXISTS nokey").await, ":0\r\n");
        assert_eq!(client.command("TTL mykey").await, ":-1\r\n");
        assert_eq!(client.command("EXPIRE mykey 60").await, ":1\r\n");
        let ttl_resp = client.command("TTL mykey").await;
        assert!(
            ttl_resp.starts_with(':') && !ttl_resp.starts_with(":-"),
            "expected positive TTL, got {ttl_resp}"
        );
        assert_eq!(client.command("EXPIRE nokey 60").await, ":0\r\n");

        server.abort();
    }

    #[tokio::test]
    async fn flushdb() {
        let port = find_available_port();
        let server = start_server(port).await;
        let mut client = TestClient::connect(&format!("127.0.0.1:{port}")).await;

        assert_eq!(client.command("SET a 1").await, "+OK\r\n");
        assert_eq!(client.command("SET b 2").await, "+OK\r\n");
        assert_eq!(client.command("DBSIZE").await, ":2\r\n");
        assert_eq!(client.command("FLUSHDB").await, "+OK\r\n");
        assert_eq!(client.command("DBSIZE").await, ":0\r\n");

        server.abort();
    }

    #[tokio::test]
    async fn keys_command() {
        let port = find_available_port();
        let server = start_server(port).await;
        let mut client = TestClient::connect(&format!("127.0.0.1:{port}")).await;

        assert_eq!(client.command("SET alpha 1").await, "+OK\r\n");
        assert_eq!(client.command("SET beta 2").await, "+OK\r\n");
        // KEYS returns RESP array format
        let resp = client.command("KEYS").await;
        assert!(
            resp.starts_with("*2\r\n"),
            "expected array of 2, got: {resp}"
        );

        server.abort();
    }

    // --- MGET/MSET integration tests ---

    #[tokio::test]
    async fn e2e_snapshot_persistence() {
        use crate::storage::snapshot;

        // Pre-bind listener atomically to prevent port collision in parallel tests
        let tokio_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = tokio_listener.local_addr().unwrap().port();

        let snap_path = format!("target/test_e2e/snapshot_restart_{port}.zdb");
        // Ensure clean state
        let _ = std::fs::remove_file(&snap_path);
        if let Some(dir) = std::path::Path::new(&snap_path).parent() {
            let _ = std::fs::create_dir_all(dir);
        }

        // --- Phase 1: start server, write data, stop ---
        let engine = Arc::new(DashMapEngine::new());
        {
            let (tx, rx) = tokio::sync::watch::channel(false);
            let eng = engine.clone();
            let handle = tokio::spawn(async move {
                let _ = run_server_with_listener(
                    tokio_listener,
                    eng,
                    None,
                    rx,
                    Duration::from_secs(300),
                    0,
                    false,
                )
                .await;
            });
            tokio::time::sleep(Duration::from_millis(50)).await;

            let mut client = TestClient::connect(&format!("127.0.0.1:{port}")).await;
            assert_eq!(
                client.command("SET persist_key persist_val").await,
                "+OK\r\n"
            );
            assert_eq!(client.command("SET counter 42").await, "+OK\r\n");
            assert_eq!(client.command("INCR counter").await, ":43\r\n");
            assert_eq!(client.command("GET persist_key").await, "+persist_val\r\n");

            // Trigger shutdown
            tx.send(true).ok();
            handle.await.ok();
        }

        // Final snapshot (mirrors main.rs shutdown sequence)
        let count = snapshot::dump_snapshot(engine.as_ref(), &snap_path).unwrap();
        assert_eq!(count, 2);

        // --- Phase 2: restore from snapshot into a new engine ---
        let engine2 = Arc::new(DashMapEngine::new());
        let loaded = snapshot::load_snapshot(engine2.as_ref(), &snap_path).unwrap();
        assert_eq!(loaded, 2);
        assert_eq!(
            engine2.get("persist_key").unwrap().unwrap().data,
            bytes::Bytes::from("persist_val")
        );
        assert_eq!(
            engine2.get("counter").unwrap().unwrap().data,
            bytes::Bytes::from("43")
        );

        let _ = std::fs::remove_file(&snap_path);
    }

    #[tokio::test]
    async fn e2e_aof_persistence() {
        use crate::storage::aof;

        // Pre-bind listener atomically to prevent port collision in parallel tests
        let tokio_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = tokio_listener.local_addr().unwrap().port();

        let aof_path = format!("target/test_e2e/aof_restart_{port}.zdb");
        let _ = std::fs::remove_file(&aof_path);
        if let Some(dir) = std::path::Path::new(&aof_path).parent() {
            let _ = std::fs::create_dir_all(dir);
        }

        // --- Phase 1: start server with AOF, write data, stop ---
        {
            let engine = Arc::new(DashMapEngine::new());
            let aof_writer = Arc::new(
                aof::AofWriter::new(&aof_path, crate::config::FsyncPolicy::Always).unwrap(),
            );
            let (tx, rx) = tokio::sync::watch::channel(false);
            let eng = engine.clone();
            let aw = aof_writer.clone();
            let handle = tokio::spawn(async move {
                let _ = run_server_with_listener(
                    tokio_listener,
                    eng,
                    Some(aw),
                    rx,
                    Duration::from_secs(300),
                    0,
                    false,
                )
                .await;
            });
            tokio::time::sleep(Duration::from_millis(50)).await;

            let mut client = TestClient::connect(&format!("127.0.0.1:{port}")).await;
            assert_eq!(client.command("SET aof_key aof_val").await, "+OK\r\n");
            assert_eq!(client.command("SET num 10").await, "+OK\r\n");
            assert_eq!(client.command("INCR num").await, ":11\r\n");
            assert_eq!(client.command("DEL aof_key").await, ":1\r\n");

            tx.send(true).ok();
            handle.await.ok();
        }

        // --- Phase 2: replay AOF into a fresh engine ---
        let engine2 = DashMapEngine::new();
        let replayed = aof::replay_aof(&engine2, &aof_path).unwrap();
        assert_eq!(replayed, 4); // SET, SET, INCR, DEL
        assert!(engine2.get("aof_key").unwrap().is_none()); // was DELeted
        assert_eq!(
            engine2.get("num").unwrap().unwrap().data,
            bytes::Bytes::from("11")
        );

        let _ = std::fs::remove_file(&aof_path);
    }

    #[tokio::test]
    async fn mget_mset_inline() {
        let port = find_available_port();
        let server = start_server(port).await;
        let mut client = TestClient::connect(&format!("127.0.0.1:{port}")).await;

        // MSET with multiple pairs
        assert_eq!(client.command("MSET a 1 b 2 c 3").await, "+OK\r\n");

        // MGET returns array — read raw lines
        client
            .writer
            .write_all(b"MGET a missing c\r\n")
            .await
            .unwrap();
        client.writer.flush().await.unwrap();

        // *3\r\n
        let mut line = String::new();
        client.reader.read_line(&mut line).await.unwrap();
        assert_eq!(line, "*3\r\n");

        // $1\r\n1\r\n  (value for key "a")
        let mut line = String::new();
        client.reader.read_line(&mut line).await.unwrap();
        assert_eq!(line, "$1\r\n");
        let mut line = String::new();
        client.reader.read_line(&mut line).await.unwrap();
        assert_eq!(line, "1\r\n");

        // $-1\r\n  (nil for missing)
        let mut line = String::new();
        client.reader.read_line(&mut line).await.unwrap();
        assert_eq!(line, "$-1\r\n");

        // $1\r\n3\r\n  (value for key "c")
        let mut line = String::new();
        client.reader.read_line(&mut line).await.unwrap();
        assert_eq!(line, "$1\r\n");
        let mut line = String::new();
        client.reader.read_line(&mut line).await.unwrap();
        assert_eq!(line, "3\r\n");

        server.abort();
    }

    #[tokio::test]
    async fn resp_mget_mset() {
        let port = find_available_port();
        let server = start_server(port).await;
        let mut client = RespTestClient::connect(&format!("127.0.0.1:{port}")).await;

        // MSET via RESP
        assert_eq!(
            client
                .resp_command(&[b"MSET", b"k1", b"v1", b"k2", b"v2"])
                .await,
            "+OK\r\n"
        );

        // MGET via RESP — send raw and read full array response
        let frame = b"*4\r\n$4\r\nMGET\r\n$2\r\nk1\r\n$5\r\nnokey\r\n$2\r\nk2\r\n";
        client.writer.write_all(frame).await.unwrap();
        client.writer.flush().await.unwrap();

        // *3\r\n
        let mut line = String::new();
        client.reader.read_line(&mut line).await.unwrap();
        assert_eq!(line, "*3\r\n");

        // $2\r\nv1\r\n
        let mut line = String::new();
        client.reader.read_line(&mut line).await.unwrap();
        assert_eq!(line, "$2\r\n");
        let mut line = String::new();
        client.reader.read_line(&mut line).await.unwrap();
        assert_eq!(line, "v1\r\n");

        // $-1\r\n  (nil)
        let mut line = String::new();
        client.reader.read_line(&mut line).await.unwrap();
        assert_eq!(line, "$-1\r\n");

        // $2\r\nv2\r\n
        let mut line = String::new();
        client.reader.read_line(&mut line).await.unwrap();
        assert_eq!(line, "$2\r\n");
        let mut line = String::new();
        client.reader.read_line(&mut line).await.unwrap();
        assert_eq!(line, "v2\r\n");

        server.abort();
    }

    #[tokio::test]
    async fn resp_mset_overwrite() {
        let port = find_available_port();
        let server = start_server(port).await;
        let mut client = RespTestClient::connect(&format!("127.0.0.1:{port}")).await;

        // SET initial value
        assert_eq!(
            client.resp_command(&[b"SET", b"k", b"old"]).await,
            "+OK\r\n"
        );
        // MSET overwrites
        assert_eq!(
            client.resp_command(&[b"MSET", b"k", b"new"]).await,
            "+OK\r\n"
        );
        assert_eq!(client.resp_command(&[b"GET", b"k"]).await, "$3\r\nnew\r\n");

        server.abort();
    }
}
