use std::sync::Arc;

use clap::Parser;
use zetdb::config::Config;
use zetdb::server::tcp::run_server_with_shutdown;
use zetdb::storage::aof;
use zetdb::storage::dashmap_engine::DashMapEngine;
use zetdb::storage::snapshot;
use zetdb::storage::ttl;

#[tokio::main]
async fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    let config = Config::parse();
    let engine = Arc::new(DashMapEngine::new());
    let snapshot_enabled = config.snapshot.enabled();
    let snapshot_path = config.snapshot.path().to_string();

    // Restore from snapshot
    if snapshot_enabled {
        match snapshot::load_snapshot(engine.as_ref(), &snapshot_path) {
            Ok(count) => log::info!("restored {count} entries from snapshot"),
            Err(e) => log::warn!("snapshot restore failed: {e}"),
        }
    }

    // Replay AOF (incremental after snapshot)
    if config.aof.enabled() {
        match aof::replay_aof(engine.as_ref(), config.aof.path()) {
            Ok(count) => log::info!("replayed {count} AOF commands"),
            Err(e) => log::warn!("aof replay failed: {e}"),
        }
    }

    let sweeper_engine = engine.clone();
    let sweep_interval = config.sweep_interval();
    let sweeper_handle = tokio::spawn(async move {
        ttl::run_sweeper(sweeper_engine, sweep_interval).await;
    });

    // Background snapshotter
    let snap_handle = if snapshot_enabled && config.snapshot.interval().as_millis() > 0 {
        let snap_engine = engine.clone();
        let snap_path = config.snapshot.path().to_string();
        let snap_interval = config.snapshot.interval();
        Some(tokio::spawn(async move {
            snapshot::run_snapshotter(snap_engine, snap_path, snap_interval).await;
        }))
    } else {
        None
    };

    // AOF writer + background tasks
    let aof_writer = if config.aof.enabled() {
        match aof::AofWriter::new(config.aof.path(), config.aof.fsync()) {
            Ok(w) => {
                log::info!("aof enabled: {}", config.aof.path());
                let w = Arc::new(w);

                // Fsync ticker for EverySecond policy
                if config.aof.fsync().is_every_second() {
                    let fsync_w = w.clone();
                    tokio::spawn(async move { aof::run_aof_fsync(fsync_w).await });
                }

                // AOF rewriter
                let threshold = config.aof.rewrite_threshold_mb() * 1024 * 1024;
                let rewrite_engine = engine.clone();
                let rewrite_w = w.clone();
                tokio::spawn(async move {
                    aof::run_aof_rewriter(
                        rewrite_engine,
                        rewrite_w,
                        threshold,
                        std::time::Duration::from_secs(60),
                    )
                    .await
                });

                Some(w)
            }
            Err(e) => {
                log::error!("aof open failed: {e}");
                None
            }
        }
    } else {
        None
    };

    // Graceful shutdown: ctrl_c signals the server to stop accepting
    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
    let server_handle = tokio::spawn(run_server_with_shutdown(
        config,
        engine.clone(),
        aof_writer,
        shutdown_rx,
    ));

    // Wait for SIGINT / Ctrl+C
    tokio::signal::ctrl_c()
        .await
        .expect("failed to install ctrl-c handler");
    log::info!("received shutdown signal");

    // Signal server to stop accepting and drain
    shutdown_tx.send(true).ok();
    server_handle.await.ok();

    // Stop background tasks
    sweeper_handle.abort();
    if let Some(h) = snap_handle {
        h.abort();
    }

    // Final snapshot before exit
    if snapshot_enabled {
        match snapshot::dump_snapshot(engine.as_ref(), &snapshot_path) {
            Ok(count) => log::info!("final snapshot: {count} entries saved"),
            Err(e) => log::error!("final snapshot failed: {e}"),
        }
    }

    log::info!("ZetDB shutdown complete");
}
