use std::sync::Arc;

use zetdb::config::Config;
use zetdb::server::tcp::run_server;
use zetdb::storage::dashmap_engine::DashMapEngine;
use zetdb::storage::ttl;

#[tokio::main]
async fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    let config = Config::default();
    let engine = Arc::new(DashMapEngine::new());

    let sweeper_engine = engine.clone();
    let sweep_interval = config.sweep_interval;
    tokio::spawn(async move {
        ttl::run_sweeper(sweeper_engine, sweep_interval).await;
    });

    if let Err(e) = run_server(config, engine).await {
        eprintln!("[ERROR] ZetDB: {e}");
        std::process::exit(1);
    }
}
