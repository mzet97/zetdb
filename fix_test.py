import sys

with open('src/server/tcp.rs', 'r') as f:
    content = f.read()

# Find the test and replace with a better version
old_test = '''    #[tokio::test]
    async fn write_timeout_disconnects_slow_client() {
        let port = find_available_port();
        let config = Config {
            bind_addr: "127.0.0.1".into(),
            port,
            write_timeout_secs: 1,
            ..Default::default()
        };
        let engine = Arc::new(DashMapEngine::new());
        let server = tokio::spawn(async move {
            let _ = run_server(config, engine, None).await;
        });
        tokio::time::sleep(Duration::from_millis(50)).await;

        let mut client = TestClient::connect(&format!("127.0.0.1:{port}")).await;
        assert_eq!(client.command("PING").await, "+PONG\r\n");

        // Populate with many keys to generate a large response
        for i in 0..100 {
            client
                .writer
                .write_all(format!("SET key{i} value{i}\r\n").as_bytes())
                .await
                .unwrap();
        }
        client.writer.flush().await.unwrap();
        // Read all responses
        for _ in 0..100 {
            let mut line = String::new();
            client.reader.read_line(&mut line).await.unwrap();
            assert_eq!(line, "+OK\r\n");
        }

        // Send KEYS command which generates a large response
        client.writer.write_all(b"KEYS\r\n").await.unwrap();
        client.writer.flush().await.unwrap();

        // Read ONLY the array header but NOT the actual keys
        // This leaves the server with a full write buffer
        let mut line = String::new();
        client.reader.read_line(&mut line).await.unwrap();
        assert!(line.starts_with('*'), "expected array header, got: {line}");

        // Now stop reading to simulate a slow client
        // The server should disconnect us after write timeout (1s)
        // because it will try to write the rest of the response
        tokio::time::sleep(Duration::from_millis(1500)).await;

        // Connection should be closed — write or read fails
        let write_result = client.writer.write_all(b"PING\r\n").await;
        client.writer.flush().await.unwrap();
        if write_result.is_ok() {
            let mut line = String::new();
            let read_result = client.reader.read_line(&mut line).await;
            assert!(
                read_result.is_err() || read_result.unwrap() == 0,
                "Expected connection closed after write timeout, got: {line}"
            );
        }

        server.abort();
    }'''

new_test = '''    #[tokio::test]
    async fn write_timeout_disconnects_slow_client() {
        let port = find_available_port();
        let config = Config {
            bind_addr: "127.0.0.1".into(),
            port,
            write_timeout_secs: 1,
            ..Default::default()
        };
        let engine = Arc::new(DashMapEngine::new());
        let server = tokio::spawn(async move {
            let _ = run_server(config, engine, None).await;
        });
        tokio::time::sleep(Duration::from_millis(50)).await;

        let mut client = TestClient::connect(&format!("127.0.0.1:{port}")).await;
        assert_eq!(client.command("PING").await, "+PONG\r\n");

        // Populate with many keys to generate a large response
        for i in 0..100 {
            client
                .writer
                .write_all(format!("SET key{i} value{i}\r\n").as_bytes())
                .await
                .unwrap();
        }
        client.writer.flush().await.unwrap();
        // Read all responses
        for _ in 0..100 {
            let mut line = String::new();
            client.reader.read_line(&mut line).await.unwrap();
            assert_eq!(line, "+OK\r\n");
        }

        // Send KEYS command which generates a large response
        client.writer.write_all(b"KEYS\r\n").await.unwrap();
        client.writer.flush().await.unwrap();

        // Read ONLY the array header but NOT the actual keys
        // This leaves the server with a full write buffer
        let mut line = String::new();
        client.reader.read_line(&mut line).await.unwrap();
        assert!(line.starts_with('*'), "expected array header, got: {line}");

        // Now stop reading to simulate a slow client
        // The server should disconnect us after write timeout (1s)
        // because it will try to write the rest of the response
        tokio::time::sleep(Duration::from_millis(1500)).await;

        // Connection should be closed — write or read fails
        let write_result = client.writer.write_all(b"PING\r\n").await;
        client.writer.flush().await.unwrap();
        if write_result.is_ok() {
            let mut line = String::new();
            let read_result = client.reader.read_line(&mut line).await;
            assert!(
                read_result.is_err() || read_result.unwrap() == 0,
                "Expected connection closed after write timeout, got: {line}"
            );
        }

        server.abort();
    }'''

if old_test in content:
    content = content.replace(old_test, new_test)
    with open('src/server/tcp.rs', 'w') as f:
        f.write(content)
    print('Replaced test successfully')
else:
    print('Old test not found')
    sys.exit(1)
