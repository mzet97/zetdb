#![allow(clippy::needless_range_loop)]

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

use zetdb::config::Config;
use zetdb::server::tcp::run_server;
use zetdb::storage::dashmap_engine::DashMapEngine;

fn find_available_port() -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.local_addr().unwrap().port()
}

/// Build a reusable command batch buffer for the entire pipeline.
/// Returns (batch_buffer, expected_response_count).
fn build_batch(op: &str, pipeline_size: usize, key_counter: &mut u64, _client_id: u64) -> Vec<u8> {
    let mut batch = Vec::with_capacity(pipeline_size * 32); // rough estimate
    let mut itoa_buf = itoa::Buffer::new();
    for _ in 0..pipeline_size {
        match op {
            "SET" => {
                let k = *key_counter;
                *key_counter += 1;
                let k_str = itoa_buf.format(k);
                batch.extend_from_slice(b"SET pipe:");
                batch.extend_from_slice(k_str.as_bytes());
                batch.extend_from_slice(b" val:");
                batch.extend_from_slice(k_str.as_bytes());
                batch.extend_from_slice(b"\r\n");
            }
            "GET" => {
                let k = *key_counter % 1000;
                *key_counter += 1;
                let k_str = itoa_buf.format(k);
                batch.extend_from_slice(b"GET pipe:");
                batch.extend_from_slice(k_str.as_bytes());
                batch.extend_from_slice(b"\r\n");
            }
            _ => unreachable!(),
        }
    }
    batch
}

/// Pipelined benchmark: send `pipeline_size` commands in one batch, then read all responses.
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
            let mut stream = TcpStream::connect(&addr).await.unwrap();
            let mut local_count = 0u64;
            let mut key_counter = client_id as u64 * 1_000_000;
            
            // Pre-allocate read buffer (enough for all responses)
            let mut read_buf = vec![0u8; pipeline_size * 32];
            let mut read_pos = 0usize;

            loop {
                if !running.load(Ordering::Relaxed) {
                    break;
                }

                // Build and send multiple batches before reading responses
                let mut batch = Vec::with_capacity(pipeline_size * 32 * 4);
                for _ in 0..4 {
                    let single_batch = build_batch(&op, pipeline_size, &mut key_counter, client_id as u64);
                    batch.extend_from_slice(&single_batch);
                }
                
                // Write entire batch at once
                if stream.write_all(&batch).await.is_err() {
                    return;
                }
                if stream.flush().await.is_err() {
                    return;
                }

                // Read all responses - count newlines to know when we got all responses
                let total_responses = pipeline_size * 4;
                let mut responses_received = 0usize;
                while responses_received < total_responses {
                    // Read more data if needed
                    if read_pos == 0 {
                        match stream.read(&mut read_buf).await {
                            Ok(0) => return,
                            Ok(n) => {
                                // Count newlines in received data
                                for i in 0..n {
                                    if read_buf[i] == b'\n' {
                                        responses_received += 1;
                                    }
                                }
                            }
                            Err(_) => return,
                        }
                    } else {
                        // We have leftover data from previous read
                        // For simplicity, just read more
                        match stream.read(&mut read_buf[read_pos..]).await {
                            Ok(0) => return,
                            Ok(n) => {
                                for i in read_pos..read_pos + n {
                                    if read_buf[i] == b'\n' {
                                        responses_received += 1;
                                    }
                                }
                                read_pos += n;
                            }
                            Err(_) => return,
                        }
                    }
                }

                local_count += (pipeline_size * 4) as u64;
                if local_count % 10_000 == 0 {
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
    _n_keys: u64,
) {
    let write_ops = Arc::new(AtomicU64::new(0));
    let read_ops = Arc::new(AtomicU64::new(0));
    let running = Arc::new(std::sync::atomic::AtomicBool::new(true));
    let start = Instant::now();

    let mut handles = Vec::new();

    // Writers
    for client_id in 0..writers {
        let addr = addr.to_string();
        let write_ops = write_ops.clone();
        let running = running.clone();

        handles.push(tokio::spawn(async move {
            let mut stream = TcpStream::connect(&addr).await.unwrap();
            let mut local_count = 0u64;
            let mut key_counter = client_id as u64 * 1_000_000;
            let mut read_buf = vec![0u8; pipeline_size * 32];

            loop {
                if !running.load(Ordering::Relaxed) {
                    break;
                }

                let batch = build_batch("SET", pipeline_size, &mut key_counter, client_id as u64);
                if stream.write_all(&batch).await.is_err() {
                    return;
                }
                if stream.flush().await.is_err() {
                    return;
                }

                let mut responses_received = 0usize;
                while responses_received < pipeline_size {
                    match stream.read(&mut read_buf).await {
                        Ok(0) => return,
                        Ok(n) => {
                            for i in 0..n {
                                if read_buf[i] == b'\n' {
                                    responses_received += 1;
                                }
                            }
                        }
                        Err(_) => return,
                    }
                }

                local_count += pipeline_size as u64;
                if local_count % 10_000 == 0 {
                    write_ops.fetch_add(10_000, Ordering::Relaxed);
                }
            }

            write_ops.fetch_add(local_count % 10_000, Ordering::Relaxed);
        }));
    }

    // Readers
    for client_id in 0..readers {
        let addr = addr.to_string();
        let read_ops = read_ops.clone();
        let running = running.clone();

        handles.push(tokio::spawn(async move {
            let mut stream = TcpStream::connect(&addr).await.unwrap();
            let mut local_count = 0u64;
            let mut key_counter = client_id as u64 * 1_000_000;
            let mut read_buf = vec![0u8; pipeline_size * 32];

            loop {
                if !running.load(Ordering::Relaxed) {
                    break;
                }

                let batch = build_batch("GET", pipeline_size, &mut key_counter, client_id as u64);
                if stream.write_all(&batch).await.is_err() {
                    return;
                }
                if stream.flush().await.is_err() {
                    return;
                }

                let mut responses_received = 0usize;
                while responses_received < pipeline_size {
                    match stream.read(&mut read_buf).await {
                        Ok(0) => return,
                        Ok(n) => {
                            for i in 0..n {
                                if read_buf[i] == b'\n' {
                                    responses_received += 1;
                                }
                            }
                        }
                        Err(_) => return,
                    }
                }

                local_count += pipeline_size as u64;
                if local_count % 10_000 == 0 {
                    read_ops.fetch_add(10_000, Ordering::Relaxed);
                }
            }

            read_ops.fetch_add(local_count % 10_000, Ordering::Relaxed);
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
    let wps = w as f64 / elapsed.as_secs_f64();
    let rps = r as f64 / elapsed.as_secs_f64();
    let tps = (w + r) as f64 / elapsed.as_secs_f64();

    println!(
        "{label:50} | {:>10.0} w/s | {:>10.0} r/s | {:>12.0} total/s",
        wps, rps, tps
    );
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
        write_timeout_secs: 0, // Disable write timeout for benchmark (zero overhead)
        sweep_interval_secs: 1,
        max_connections: 0,
        max_keys: 0,
        metrics_enabled: false,
        snapshot: Default::default(),
        aof: Default::default(),
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

    // --- WRITE (SET) — Peak scenarios only ---
    println!("--- WRITE (SET) — Pipelined ---");
    println!("{:50} | {:>10}     | {:>12}", "test", "total", "ops/s");
    println!("{}", "-".repeat(95));

    // Peak scenarios: pipe=50,100,200,500 with clients=16,32
    for &pipe in &[50, 100, 200, 500] {
        for &clients in &[16, 32] {
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

    // --- READ (GET) — Peak scenarios only ---
    println!("--- READ (GET) — Pipelined ---");
    println!("Pre-populating {n_keys} keys...");
    println!("{:50} | {:>10}     | {:>12}", "test", "total", "ops/s");
    println!("{}", "-".repeat(95));

    // Pre-populate keys for GET benchmark
    {
        let mut stream = TcpStream::connect(&addr).await.unwrap();
        let mut batch = Vec::with_capacity(8192);
        for k in 0..n_keys {
            batch.extend_from_slice(format!("SET pipe:{k} val:{k}\r\n").as_bytes());
            if batch.len() > 4096 {
                stream.write_all(&batch).await.unwrap();
                batch.clear();
            }
        }
        if !batch.is_empty() {
            stream.write_all(&batch).await.unwrap();
        }
        stream.flush().await.unwrap();
        // Read responses
        let mut buf = vec![0u8; 8192];
        let mut responses = 0u64;
        while responses < n_keys {
            match stream.read(&mut buf).await {
                Ok(0) => break,
                Ok(n) => {
                    for byte in buf.iter().take(n) {
                        if *byte == b'\n' {
                            responses += 1;
                        }
                    }
                }
                Err(_) => break,
            }
        }
    }

    for &pipe in &[50, 100, 200, 500] {
        for &clients in &[16, 32] {
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

    // --- MIXED — Peak scenarios only ---
    println!("--- MIXED — Pipelined ---");
    println!(
        "{:50} | {:>10}     | {:>10}     | {:>12}",
        "test", "writes/s", "reads/s", "total/s"
    );
    println!("{}", "-".repeat(95));

    for &pipe in &[50, 100, 200] {
        for &(w, r) in &[(8, 8), (16, 16)] {
            pipeline_mixed(
                &format!("MIXED pipe={pipe} w={w} r={r}"),
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
    println!("Benchmark complete.");
}
