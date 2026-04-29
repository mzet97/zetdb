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

/// Pipelined benchmark: send `pipeline_size` commands, then read `pipeline_size` responses.
async fn pipeline_bench(
    label: &str,
    addr: &str,
    clients: usize,
    duration: Duration,
    pipeline_size: usize,
    operation: &str,
) {
    let total_ops = Arc::new(AtomicU64::new(0));
    let running = Arc::new(std::sync::atomic::AtomicBool::new(true));
    let start = Instant::now();

    let mut handles = Vec::new();

    for client_id in 0..clients {
        let addr = addr.to_string();
        let total_ops = total_ops.clone();
        let running = running.clone();
        let op = operation.to_string();

        handles.push(tokio::spawn(async move {
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

                // --- WRITE PHASE: send pipeline_size commands ---
                for _ in 0..pipeline_size {
                    let cmd = match op.as_str() {
                        "SET" => {
                            let k = key_counter;
                            key_counter += 1;
                            format!("SET pipe:{k} val:{k}\r\n")
                        }
                        "GET" => {
                            let k = key_counter % 1000;
                            key_counter += 1;
                            format!("GET pipe:{k}\r\n")
                        }
                        _ => unreachable!(),
                    };
                    if writer.write_all(cmd.as_bytes()).await.is_err() {
                        return;
                    }
                }
                if writer.flush().await.is_err() {
                    return;
                }

                // --- READ PHASE: read pipeline_size responses ---
                for _ in 0..pipeline_size {
                    line.clear();
                    if buf_reader.read_line(&mut line).await.is_err() {
                        return;
                    }
                }

                local_count += pipeline_size as u64;
                if local_count.is_multiple_of(10_000) {
                    total_ops.fetch_add(10_000, Ordering::Relaxed);
                }
            }

            total_ops.fetch_add(local_count % 10_000, Ordering::Relaxed);
        }));
    }

    tokio::time::sleep(duration).await;
    running.store(false, Ordering::Relaxed);

    for handle in handles {
        let _ = handle.await;
    }

    let elapsed = start.elapsed();
    let ops = total_ops.load(Ordering::Relaxed);
    let ops_per_sec = ops as f64 / elapsed.as_secs_f64();

    println!("{label:50} | {ops:>10} ops | {ops_per_sec:>12.0} ops/s");
}

/// Mixed pipelined workload: writers + readers concurrently
async fn pipeline_mixed(
    label: &str,
    addr: &str,
    writers: usize,
    readers: usize,
    duration: Duration,
    pipeline_size: usize,
    n_keys: u64,
) {
    let write_ops = Arc::new(AtomicU64::new(0));
    let read_ops = Arc::new(AtomicU64::new(0));
    let running = Arc::new(std::sync::atomic::AtomicBool::new(true));
    let start = Instant::now();

    let mut handles = Vec::new();

    for wid in 0..writers {
        let addr = addr.to_string();
        let write_ops = write_ops.clone();
        let running = running.clone();
        let base = (wid as u64) * 10_000_000;

        handles.push(tokio::spawn(async move {
            let stream = TcpStream::connect(&addr).await.unwrap();
            let (reader, mut writer) = stream.into_split();
            let mut buf_reader = BufReader::new(reader);
            let mut line = String::new();
            let mut count = 0u64;
            let mut key = base;

            loop {
                if !running.load(Ordering::Relaxed) {
                    break;
                }

                for _ in 0..pipeline_size {
                    let cmd = format!("SET pw:{key} v:{key}\r\n");
                    key += 1;
                    if writer.write_all(cmd.as_bytes()).await.is_err() {
                        return;
                    }
                }
                if writer.flush().await.is_err() {
                    return;
                }

                for _ in 0..pipeline_size {
                    line.clear();
                    if buf_reader.read_line(&mut line).await.is_err() {
                        return;
                    }
                }

                count += pipeline_size as u64;
                if count.is_multiple_of(10_000) {
                    write_ops.fetch_add(10_000, Ordering::Relaxed);
                }
            }
            write_ops.fetch_add(count % 10_000, Ordering::Relaxed);
        }));
    }

    for _ in 0..readers {
        let addr = addr.to_string();
        let read_ops = read_ops.clone();
        let running = running.clone();

        handles.push(tokio::spawn(async move {
            let stream = TcpStream::connect(&addr).await.unwrap();
            let (reader, mut writer) = stream.into_split();
            let mut buf_reader = BufReader::new(reader);
            let mut line = String::new();
            let mut count = 0u64;
            let mut key = 0u64;

            loop {
                if !running.load(Ordering::Relaxed) {
                    break;
                }

                for _ in 0..pipeline_size {
                    let cmd = format!("GET pp:{}\r\n", key % n_keys);
                    key += 1;
                    if writer.write_all(cmd.as_bytes()).await.is_err() {
                        return;
                    }
                }
                if writer.flush().await.is_err() {
                    return;
                }

                for _ in 0..pipeline_size {
                    line.clear();
                    if buf_reader.read_line(&mut line).await.is_err() {
                        return;
                    }
                }

                count += pipeline_size as u64;
                if count.is_multiple_of(10_000) {
                    read_ops.fetch_add(10_000, Ordering::Relaxed);
                }
            }
            read_ops.fetch_add(count % 10_000, Ordering::Relaxed);
        }));
    }

    tokio::time::sleep(duration).await;
    running.store(false, Ordering::Relaxed);

    for handle in handles {
        let _ = handle.await;
    }

    let elapsed = start.elapsed();
    let w = write_ops.load(Ordering::Relaxed);
    let r = read_ops.load(Ordering::Relaxed);
    let ws = w as f64 / elapsed.as_secs_f64();
    let rs = r as f64 / elapsed.as_secs_f64();
    let ts = (w + r) as f64 / elapsed.as_secs_f64();

    println!("{label:50} | {ws:>10.0} w/s | {rs:>10.0} r/s | {ts:>12.0} total/s");
}

async fn prepopulate(addr: &str, n_keys: u64) {
    let stream = TcpStream::connect(addr).await.unwrap();
    let (reader, mut writer) = stream.into_split();
    let mut buf_reader = BufReader::new(reader);
    let mut line = String::new();

    let batch = 500;
    for chunk in (0..n_keys).collect::<Vec<_>>().chunks(batch) {
        for k in chunk {
            let cmd = format!("SET pp:{k} val:{k}\r\n");
            writer.write_all(cmd.as_bytes()).await.unwrap();
        }
        writer.flush().await.unwrap();
        for _ in chunk {
            line.clear();
            let _ = buf_reader.read_line(&mut line).await;
        }
    }
}

#[tokio::main]
async fn main() {
    let port = find_available_port();
    let addr = format!("127.0.0.1:{port}");
    let duration = Duration::from_secs(5);
    let n_keys: u64 = 5000;

    let config = Config {
        bind_addr: "127.0.0.1".into(),
        port,
        read_timeout_secs: 300,
        ..Default::default()
    };
    let engine = Arc::new(DashMapEngine::new());

    let server_engine = engine.clone();
    tokio::spawn(async move {
        let _ = run_server(config, server_engine, None).await;
    });
    tokio::time::sleep(Duration::from_millis(100)).await;

    println!("ZetDB Pipeline Benchmark");
    println!("Duration: {}s per test", duration.as_secs());
    println!("{}", "=".repeat(95));
    println!();

    // --- WRITE (SET) with different pipeline sizes ---
    println!("--- WRITE (SET) — Pipelined ---");
    println!("{:50} | {:>10}     | {:>12}", "test", "total", "ops/s");
    println!("{}", "-".repeat(95));

    for &pipe in &[1, 10, 50, 100, 200, 500] {
        for &clients in &[1, 4, 16, 32] {
            pipeline_bench(
                &format!("SET pipe={pipe} clients={clients}"),
                &addr,
                clients,
                duration,
                pipe,
                "SET",
            )
            .await;
        }
    }

    println!();

    // --- READ (GET) with different pipeline sizes ---
    println!("--- READ (GET) — Pipelined ---");
    println!("Pre-populating {n_keys} keys...");
    prepopulate(&addr, n_keys).await;
    println!();
    println!("{:50} | {:>10}     | {:>12}", "test", "total", "ops/s");
    println!("{}", "-".repeat(95));

    for &pipe in &[1, 10, 50, 100, 200, 500] {
        for &clients in &[1, 4, 16, 32] {
            pipeline_bench(
                &format!("GET pipe={pipe} clients={clients}"),
                &addr,
                clients,
                duration,
                pipe,
                "GET",
            )
            .await;
        }
    }

    println!();

    // --- MIXED with best pipeline sizes ---
    println!("--- MIXED — Pipelined ---");
    println!(
        "{:50} | {:>10}     | {:>10}     | {:>12}",
        "test", "writes/s", "reads/s", "total/s"
    );
    println!("{}", "-".repeat(110));

    for &pipe in &[50, 100, 200] {
        for &(w, r) in &[(16, 16), (32, 16), (16, 32), (32, 32)] {
            pipeline_mixed(
                &format!("MIXED {w}w/{r}r pipe={pipe}"),
                &addr,
                w,
                r,
                duration,
                pipe,
                n_keys,
            )
            .await;
        }
    }

    println!();
    println!("{}", "=".repeat(95));
    println!("Done.");
}
