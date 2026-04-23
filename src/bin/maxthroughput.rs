use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
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

/// Pre-populate keys for GET workload
async fn populate(addr: &str, n_keys: u64) {
    let stream = TcpStream::connect(addr).await.unwrap();
    let (reader, mut writer) = stream.into_split();
    let mut buf_reader = BufReader::new(reader);
    let mut line = String::new();

    // Send in batches to avoid buffer overflow
    let batch_size = 500;
    for chunk in (0..n_keys).collect::<Vec<_>>().chunks(batch_size) {
        for k in chunk {
            let cmd = format!("SET p:{k} payload:{k}\r\n");
            writer.write_all(cmd.as_bytes()).await.unwrap();
        }
        writer.flush().await.unwrap();
        for _ in chunk {
            line.clear();
            let _ = buf_reader.read_line(&mut line).await;
        }
    }
}

/// Spawn N writer clients and M reader clients, measure total ops/s
async fn mixed_workload(
    addr: &str,
    writers: usize,
    readers: usize,
    duration: Duration,
    n_keys: u64,
) -> (u64, u64, Duration) {
    let write_ops = Arc::new(AtomicU64::new(0));
    let read_ops = Arc::new(AtomicU64::new(0));
    let running = Arc::new(AtomicBool::new(true));
    let start = Instant::now();

    let mut handles = Vec::new();

    // Writer clients
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

                let cmd = format!("SET w:{key} v:{key}\r\n");
                key += 1;

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

                count += 1;
                if count % 500 == 0 {
                    write_ops.fetch_add(500, Ordering::Relaxed);
                }
            }
            write_ops.fetch_add(count % 500, Ordering::Relaxed);
        }));
    }

    // Reader clients
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

                let cmd = format!("GET p:{}\r\n", key % n_keys);
                key += 1;

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

                count += 1;
                if count % 500 == 0 {
                    read_ops.fetch_add(500, Ordering::Relaxed);
                }
            }
            read_ops.fetch_add(count % 500, Ordering::Relaxed);
        }));
    }

    tokio::time::sleep(duration).await;
    running.store(false, Ordering::Relaxed);

    for handle in handles {
        let _ = handle.await;
    }

    let elapsed = start.elapsed();
    (write_ops.load(Ordering::Relaxed), read_ops.load(Ordering::Relaxed), elapsed)
}

#[tokio::main]
async fn main() {
    let port = find_available_port();
    let addr = format!("127.0.0.1:{port}");
    let n_keys: u64 = 5000;
    let test_duration = Duration::from_secs(5);

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

    println!("ZetDB Max Throughput Benchmark (mixed workload)");
    println!("Duration: {}s per test | Pre-populated keys: {}", test_duration.as_secs(), n_keys);
    println!("{}", "=".repeat(100));
    println!();

    // Populate
    print!("Populating {n_keys} keys...");
    populate(&addr, n_keys).await;
    println!(" done.");
    println!();

    // Header
    println!(
        "{:<30} | {:>6} | {:>10} | {:>10} | {:>10} | {:>10}",
        "config", "total", "writes/s", "reads/s", "total/s", "peak"
    );
    println!("{}", "-".repeat(100));

    let mut peak_total = 0u64;
    let mut peak_label = String::new();

    // Test different write/read client ratios
    let configs: &[(usize, usize)] = &[
        (128, 0),    // write-only
        (0, 128),    // read-only
        (64, 64),    // 50/50
        (96, 32),    // 75/25
        (32, 96),    // 25/75
        (80, 80),    // 160 total
        (100, 100),  // 200 total
        (64, 128),   // 192 total, read-heavy
        (128, 64),   // 192 total, write-heavy
    ];

    for &(w, r) in configs {
        let label = if r == 0 {
            format!("WRITE ONLY ({}w)", w)
        } else if w == 0 {
            format!("READ ONLY ({}r)", r)
        } else {
            format!("MIXED {}w / {}r", w, r)
        };

        let (writes, reads, elapsed) = mixed_workload(&addr, w, r, test_duration, n_keys).await;
        let total = writes + reads;
        let ws = writes as f64 / elapsed.as_secs_f64();
        let rs = reads as f64 / elapsed.as_secs_f64();
        let ts = total as f64 / elapsed.as_secs_f64();

        let peak_marker = if total > peak_total {
            peak_total = total;
            peak_label = label.clone();
            " <<< PEAK"
        } else {
            ""
        };

        println!(
            "{:<30} | {:>6} | {:>10.0} | {:>10.0} | {:>10.0} | {:>10.0}{}",
            label,
            w + r,
            ws,
            rs,
            ts,
            ts,
            peak_marker
        );
    }

    let peak_ops = peak_total as f64 / test_duration.as_secs_f64();
    println!();
    println!("{}", "=".repeat(100));
    println!("PEAK: {} — {:.0} ops/s", peak_label, peak_ops);
}
