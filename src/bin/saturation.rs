use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;

use zetdb::config::Config;
use zetdb::server::tcp::run_server;
use zetdb::storage::dashmap_engine::DashMapEngine;

fn find_available_port() -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.local_addr().unwrap().port()
}

async fn saturation_bench(label: &str, addr: &str, clients: usize, duration: Duration, operation: &str) {
    let total_ops = Arc::new(AtomicU64::new(0));
    let running = Arc::new(std::sync::atomic::AtomicBool::new(true));
    let start = Instant::now();

    let mut handles = Vec::new();

    for client_id in 0..clients {
        let addr = addr.to_string();
        let total_ops = total_ops.clone();
        let running = running.clone();
        let op = operation.to_string();

        let handle = tokio::spawn(async move {
            let stream = TcpStream::connect(&addr).await.unwrap();
            let (reader, mut writer) = stream.into_split();
            let mut buf_reader = BufReader::new(reader);
            let mut line = String::new();
            let mut local_count = 0u64;
            let mut key_counter = client_id as u64 * 1_000_000;

            loop {
                if !running.load(Ordering::Relaxed) {
                    break;
                }

                let cmd = match op.as_str() {
                    "SET" => {
                        let k = key_counter;
                        key_counter += 1;
                        format!("SET sat:{k} val:{k}\r\n")
                    }
                    "GET" => {
                        let k = key_counter % 1000;
                        key_counter += 1;
                        format!("GET sat:{k}\r\n")
                    }
                    _ => unreachable!(),
                };

                if writer.write_all(cmd.as_bytes()).await.is_err() {
                    break;
                }
                if writer.flush().await.is_err() {
                    break;
                }

                line.clear();
                if buf_reader.read_line(&mut line).await.is_err() {
                    break;
                }

                local_count += 1;
                if local_count % 1000 == 0 {
                    total_ops.fetch_add(1000, Ordering::Relaxed);
                }
            }

            total_ops.fetch_add(local_count % 1000, Ordering::Relaxed);
        });

        handles.push(handle);
    }

    tokio::time::sleep(duration).await;
    running.store(false, Ordering::Relaxed);

    for handle in handles {
        let _ = handle.await;
    }

    let elapsed = start.elapsed();
    let ops = total_ops.load(Ordering::Relaxed);
    let ops_per_sec = ops as f64 / elapsed.as_secs_f64();

    println!(
        "{label:35} | {clients:>3} clients | {dur:>2}s | {ops:>8} ops | {ops_per_sec:>10.0} ops/s",
        dur = duration.as_secs()
    );
}

async fn presaturate(addr: &str, n_keys: u64) {
    let stream = TcpStream::connect(addr).await.unwrap();
    let (reader, mut writer) = stream.into_split();
    let mut buf_reader = BufReader::new(reader);
    let mut line = String::new();

    for k in 0..n_keys {
        let cmd = format!("SET sat:{k} val:{k}\r\n");
        writer.write_all(cmd.as_bytes()).await.unwrap();
    }
    writer.flush().await.unwrap();

    for _ in 0..n_keys {
        line.clear();
        let _ = buf_reader.read_line(&mut line).await;
    }
}

#[tokio::main]
async fn main() {
    let port = find_available_port();
    let addr = format!("127.0.0.1:{port}");

    let config = Config {
        bind_addr: "127.0.0.1".into(),
        port,
        read_timeout: Duration::from_secs(300),
        ..Default::default()
    };
    let engine = Arc::new(DashMapEngine::new());

    let server_engine = engine.clone();
    tokio::spawn(async move {
        let _ = run_server(config, server_engine, None).await;
    });
    tokio::time::sleep(Duration::from_millis(100)).await;

    println!("ZetDB Saturation Benchmark");
    println!("{}", "=".repeat(85));
    println!();

    // --- WRITE SATURATION ---
    println!("--- WRITE (SET) SATURATION ---");
    println!("{:35} | {:>3} {:>8} | {:>8}     | {:>10}", "test", "c", "dur", "total", "ops/s");
    println!("{}", "-".repeat(85));

    for &clients in &[1, 2, 4, 8, 16, 32, 64] {
        saturation_bench(
            &format!("SET ({clients} clients)"),
            &addr,
            clients,
            Duration::from_secs(3),
            "SET",
        )
        .await;
    }

    println!();

    // --- READ SATURATION ---
    println!("--- READ (GET) SATURATION ---");
    println!("Pre-populating 1000 keys for GET test...");
    presaturate(&addr, 1000).await;
    println!();

    println!("{:35} | {:>3} {:>8} | {:>8}     | {:>10}", "test", "c", "dur", "total", "ops/s");
    println!("{}", "-".repeat(85));

    for &clients in &[1, 2, 4, 8, 16, 32, 64] {
        saturation_bench(
            &format!("GET ({clients} clients)"),
            &addr,
            clients,
            Duration::from_secs(3),
            "GET",
        )
        .await;
    }

    println!();
    println!("{}", "=".repeat(85));
    println!("Done.");
}
