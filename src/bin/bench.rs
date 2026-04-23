use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;

use zetdb::config::Config;
use zetdb::server::tcp::run_server;
use zetdb::storage::dashmap_engine::DashMapEngine;

struct BenchClient {
    writer: tokio::io::WriteHalf<TcpStream>,
    reader: BufReader<tokio::io::ReadHalf<TcpStream>>,
}

impl BenchClient {
    async fn connect(addr: &str) -> Self {
        let stream = TcpStream::connect(addr).await.unwrap();
        let (reader, writer) = tokio::io::split(stream);
        Self {
            writer,
            reader: BufReader::new(reader),
        }
    }

    async fn command(&mut self, cmd: &str) -> Duration {
        let start = Instant::now();
        self.writer
            .write_all(format!("{cmd}\r\n").as_bytes())
            .await
            .unwrap();
        self.writer.flush().await.unwrap();
        let mut line = String::new();
        self.reader.read_line(&mut line).await.unwrap();
        start.elapsed()
    }
}

fn find_available_port() -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.local_addr().unwrap().port()
}

fn percentile(mut latencies: Vec<Duration>, p: f64) -> Duration {
    latencies.sort();
    let idx = ((p / 100.0) * latencies.len() as f64).floor() as usize;
    let idx = idx.min(latencies.len() - 1);
    latencies[idx]
}

async fn run_bench(label: &str, addr: &str, n: usize, operation: &str) {
    let mut client = BenchClient::connect(addr).await;

    // Warmup
    for _ in 0..10 {
        client.command("PING").await;
    }

    let mut latencies: Vec<Duration> = Vec::with_capacity(n);
    let start = Instant::now();

    for i in 0..n {
        let cmd = match operation {
            "SET" => format!("SET benchkey:{i} value:{i}"),
            "GET" => format!("GET benchkey:{i}"),
            _ => panic!("Unknown operation: {operation}"),
        };
        let latency = client.command(&cmd).await;
        latencies.push(latency);
    }

    let total = start.elapsed();
    let throughput = n as f64 / total.as_secs_f64();

    println!(
        "{label:30} | {n:>6} ops | {throughput:>10.0} ops/s | p50: {:>6?} | p95: {:>6?} | p99: {:>6?}",
        percentile(latencies.clone(), 50.0),
        percentile(latencies.clone(), 95.0),
        percentile(latencies.clone(), 99.0),
    );
}

#[tokio::main]
async fn main() {
    let port = find_available_port();
    let addr = format!("127.0.0.1:{port}");

    let config = Config {
        bind_addr: "127.0.0.1".into(),
        port,
        ..Default::default()
    };
    let engine = Arc::new(DashMapEngine::new());

    let server_engine = engine.clone();
    tokio::spawn(async move {
        let _ = run_server(config, server_engine).await;
    });
    tokio::time::sleep(Duration::from_millis(100)).await;

    println!("ZetDB Benchmark");
    println!("{}\n", "=".repeat(80));

    for &n in &[100, 1_000, 10_000] {
        run_bench(&format!("SET (n={n})"), &addr, n, "SET").await;
    }
    println!();

    for &n in &[100, 1_000, 10_000] {
        run_bench(&format!("GET (n={n})"), &addr, n, "GET").await;
    }

    println!("\n{}", "=".repeat(80));
    println!("Done.");
}
