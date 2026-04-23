use serde::Serialize;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;
use std::env;

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;

// ---------------------------------------------------------------------------
// Data structures
// ---------------------------------------------------------------------------

#[derive(Serialize)]
struct BenchResult {
    target: String,
    platform: String,
    operation: String,
    pipeline_size: usize,
    clients: usize,
    total_ops: u64,
    duration_secs: f64,
    ops_per_sec: f64,
}

// ---------------------------------------------------------------------------
// RESP helpers
// ---------------------------------------------------------------------------

/// Build a RESP array frame from arguments.
fn resp_frame(args: &[&[u8]]) -> Vec<u8> {
    let mut buf = Vec::with_capacity(64 * args.len());
    buf.extend_from_slice(format!("*{}\r\n", args.len()).as_bytes());
    for arg in args {
        buf.extend_from_slice(format!("${}\r\n", arg.len()).as_bytes());
        buf.extend_from_slice(arg);
        buf.extend_from_slice(b"\r\n");
    }
    buf
}

/// Build SET command in RESP.
fn resp_set(key: &[u8], val: &[u8]) -> Vec<u8> {
    resp_frame(&[b"SET", key, val])
}

/// Build GET command in RESP.
fn resp_get(key: &[u8]) -> Vec<u8> {
    resp_frame(&[b"GET", key])
}

// ---------------------------------------------------------------------------
// Benchmark core
// ---------------------------------------------------------------------------

/// Run a pipelined benchmark against a RESP-compatible server.
/// Returns total operations completed.
async fn resp_pipeline_bench(
    addr: &str,
    num_clients: usize,
    pipeline_size: usize,
    operation: &str, // "SET" or "GET"
    duration: Duration,
) -> u64 {
    let operation = operation.to_string();
    let running = Arc::new(AtomicBool::new(true));
    let total_ops = Arc::new(AtomicU64::new(0));

    let mut handles = Vec::with_capacity(num_clients);

    for client_id in 0..num_clients {
        let running = running.clone();
        let total_ops = total_ops.clone();
        let addr = addr.to_string();
        let op = operation.clone();

        handles.push(tokio::spawn(async move {
            let stream = TcpStream::connect(&addr).await.unwrap();
            let (read_half, write_half) = tokio::io::split(stream);
            let mut writer = tokio::io::BufWriter::new(write_half);
            let mut reader = BufReader::new(read_half);

            let mut local_count: u64 = 0;
            let mut key_counter: u64 = client_id as u64 * 1_000_000;

            // Pre-build pipeline batch
            let mut batch = Vec::with_capacity(256 * pipeline_size);
            let mut key_buf = [0u8; 32];
            let mut val_buf = [0u8; 32];

            loop {
                if !running.load(Ordering::Relaxed) {
                    break;
                }

                batch.clear();
                for _ in 0..pipeline_size {
                    let n = write_key(&mut key_buf, key_counter);
                    match op.as_str() {
                        "SET" => {
                            let v = write_key(&mut val_buf, key_counter);
                            batch.extend_from_slice(&resp_set(&key_buf[..n], &val_buf[..v]));
                        }
                        "GET" => {
                            batch.extend_from_slice(&resp_get(&key_buf[..n % 1000]));
                        }
                        _ => unreachable!(),
                    }
                    key_counter += 1;
                }

                writer.write_all(&batch).await.unwrap();
                writer.flush().await.unwrap();

                // Read responses
                let mut line = String::new();
                for _ in 0..pipeline_size {
                    line.clear();
                    reader.read_line(&mut line).await.unwrap();
                }

                local_count += pipeline_size as u64;
                if local_count % 10_000 == 0 {
                    total_ops.fetch_add(10_000, Ordering::Relaxed);
                }
            }

            total_ops.fetch_add(local_count % 10_000, Ordering::Relaxed);
        }));
    }

    tokio::time::sleep(duration).await;
    running.store(false, Ordering::Relaxed);

    for h in handles {
        h.await.unwrap();
    }

    total_ops.load(Ordering::Relaxed)
}

fn write_key(buf: &mut [u8], counter: u64) -> usize {
    let s = format!("k:{}", counter);
    let bytes = s.as_bytes();
    let len = bytes.len().min(buf.len());
    buf[..len].copy_from_slice(&bytes[..len]);
    len
}

/// Pre-populate keys for GET benchmarks.
async fn prepopulate(addr: &str, n_keys: usize) {
    let stream = TcpStream::connect(addr).await.unwrap();
    let (read_half, write_half) = tokio::io::split(stream);
    let mut writer = tokio::io::BufWriter::new(write_half);
    let mut reader = BufReader::new(read_half);

    // Send in batches of 500
    let mut batch = Vec::with_capacity(64 * 500);
    let mut remaining = n_keys;
    let mut idx: usize = 0;

    while remaining > 0 {
        let chunk = remaining.min(500);
        batch.clear();
        for _ in 0..chunk {
            let key = format!("k:{}", idx);
            let val = format!("v:{}", idx);
            batch.extend_from_slice(&resp_set(key.as_bytes(), val.as_bytes()));
            idx += 1;
        }
        writer.write_all(&batch).await.unwrap();
        writer.flush().await.unwrap();

        let mut line = String::new();
        for _ in 0..chunk {
            line.clear();
            reader.read_line(&mut line).await.unwrap();
        }
        remaining -= chunk;
    }
}

// ---------------------------------------------------------------------------
// Main
// ---------------------------------------------------------------------------

fn print_usage() {
    eprintln!("Usage: redis_compare --target <zetdb|redis> --host <addr> --port <port> --format <text|json> [--duration secs]");
}

fn detect_platform() -> String {
    if cfg!(target_os = "windows") {
        "windows".into()
    } else if cfg!(target_os = "linux") {
        "linux".into()
    } else {
        "unknown".into()
    }
}

#[tokio::main]
async fn main() {
    let args: Vec<String> = env::args().collect();

    let mut target = "zetdb".to_string();
    let mut host = "127.0.0.1".to_string();
    let mut port: u16 = 6379;
    let mut format = "text".to_string();
    let mut bench_duration = 5u64;

    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--target" => { target = args[i + 1].clone(); i += 2; }
            "--host" => { host = args[i + 1].clone(); i += 2; }
            "--port" => { port = args[i + 1].parse().unwrap(); i += 2; }
            "--format" => { format = args[i + 1].clone(); i += 2; }
            "--duration" => { bench_duration = args[i + 1].parse().unwrap(); i += 2; }
            "--help" | "-h" => { print_usage(); return; }
            _ => { eprintln!("Unknown arg: {}", args[i]); print_usage(); return; }
        }
    }

    let addr = format!("{}:{}", host, port);
    let platform = detect_platform();
    let duration = Duration::from_secs(bench_duration);

    let pipe_sizes = [1, 10, 50, 100, 200, 500];
    let client_counts = [1, 4, 16, 32];

    let mut results: Vec<BenchResult> = Vec::new();

    // --- SET ---
    if format == "text" {
        println!("--- SET (WRITE) --- target={} platform={} ---", target, platform);
        println!("{:<50} | {:>12} | {:>12}", "test", "total", "ops/s");
        println!("{}", "-".repeat(82));
    }

    for &pipe in &pipe_sizes {
        for &clients in &client_counts {
            let total = resp_pipeline_bench(&addr, clients, pipe, "SET", duration).await;
            let ops = total as f64 / duration.as_secs_f64();

            let r = BenchResult {
                target: target.clone(),
                platform: platform.clone(),
                operation: "SET".into(),
                pipeline_size: pipe,
                clients,
                total_ops: total,
                duration_secs: duration.as_secs_f64(),
                ops_per_sec: ops,
            };

            if format == "text" {
                println!(
                    "SET pipe={} clients={:<4}                   | {:>10} ops | {:>12.0} ops/s",
                    pipe, clients, total, ops
                );
            }
            results.push(r);
        }
    }

    // --- GET ---
    // Pre-populate keys
    if format == "text" {
        println!("\nPre-populating 5000 keys for GET...");
    }
    prepopulate(&addr, 5000).await;

    if format == "text" {
        println!("--- GET (READ) --- target={} platform={} ---", target, platform);
        println!("{:<50} | {:>12} | {:>12}", "test", "total", "ops/s");
        println!("{}", "-".repeat(82));
    }

    for &pipe in &pipe_sizes {
        for &clients in &client_counts {
            let total = resp_pipeline_bench(&addr, clients, pipe, "GET", duration).await;
            let ops = total as f64 / duration.as_secs_f64();

            let r = BenchResult {
                target: target.clone(),
                platform: platform.clone(),
                operation: "GET".into(),
                pipeline_size: pipe,
                clients,
                total_ops: total,
                duration_secs: duration.as_secs_f64(),
                ops_per_sec: ops,
            };

            if format == "text" {
                println!(
                    "GET pipe={} clients={:<4}                   | {:>10} ops | {:>12.0} ops/s",
                    pipe, clients, total, ops
                );
            }
            results.push(r);
        }
    }

    // --- Output ---
    if format == "json" {
        println!("{}", serde_json::to_string_pretty(&results).unwrap());
    }
}
