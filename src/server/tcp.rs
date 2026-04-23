use std::sync::Arc;

use tokio::net::TcpListener;

use crate::config::Config;
use crate::server::session::handle_session;
use crate::storage::engine::KvEngine;

pub async fn run_server(config: Config, engine: Arc<dyn KvEngine>) -> Result<(), Box<dyn std::error::Error>> {
    let listener = TcpListener::bind((config.bind_addr.as_str(), config.port)).await?;
    let local_addr = listener.local_addr()?;
    let read_timeout = config.read_timeout;

    log::info!("ZetDB listening on {local_addr}");
    log::info!("read timeout: {read_timeout:?}");

    loop {
        let (stream, peer) = listener.accept().await?;
        let engine = engine.clone();
        tokio::spawn(async move {
            handle_session(stream, peer, engine, read_timeout).await;
        });
    }
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
            let _ = run_server(config, engine).await;
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
            read_timeout: Duration::from_millis(100),
            sweep_interval: Duration::from_secs(1),
        };
        let engine = Arc::new(DashMapEngine::new());
        let server = tokio::spawn(async move {
            let _ = run_server(config, engine).await;
        });
        tokio::time::sleep(Duration::from_millis(50)).await;

        let mut client = TestClient::connect(&format!("127.0.0.1:{port}")).await;
        assert_eq!(client.command("PING").await, "+PONG\r\n");

        // Wait for timeout to trigger
        tokio::time::sleep(Duration::from_millis(200)).await;

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
}
