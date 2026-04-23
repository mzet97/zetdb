use std::sync::Arc;

use zetdb::config::Config;
use zetdb::server::tcp::run_server;
use zetdb::storage::aof;
use zetdb::storage::dashmap_engine::DashMapEngine;
use zetdb::storage::snapshot;
use zetdb::storage::ttl;

#[tokio::main]
async fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    let config = Config::default();
    let engine = Arc::new(DashMapEngine::new());

    // Restore from snapshot
    if config.snapshot.enabled {
        match snapshot::load_snapshot(engine.as_ref(), &config.snapshot.path) {
            Ok(count) => log::info!("restored {count} entries from snapshot"),
            Err(e) => log::warn!("snapshot restore failed: {e}"),
        }
    }

    // Replay AOF (incremental after snapshot)
    if config.aof.enabled {
        match aof::replay_aof(engine.as_ref(), &config.aof.path) {
            Ok(count) => log::info!("replayed {count} AOF commands"),
            Err(e) => log::warn!("aof replay failed: {e}"),
        }
    }

    let sweeper_engine = engine.clone();
    let sweep_interval = config.sweep_interval;
    tokio::spawn(async move {
        ttl::run_sweeper(sweeper_engine, sweep_interval).await;
    });

    // Background snapshotter
    if config.snapshot.enabled && config.snapshot.interval.as_millis() > 0 {
        let snap_engine = engine.clone();
        let snap_path = config.snapshot.path.clone();
        let snap_interval = config.snapshot.interval;
        tokio::spawn(async move {
            snapshot::run_snapshotter(snap_engine, snap_path, snap_interval).await;
        });
    }

    // AOF writer + background tasks
    let aof_writer = if config.aof.enabled {
        match aof::AofWriter::new(&config.aof.path, config.aof.fsync) {
            Ok(w) => {
                log::info!("aof enabled: {}", config.aof.path);
                let w = Arc::new(w);

                // Fsync ticker for EverySecond policy
                if matches!(config.aof.fsync, zetdb::config::FsyncPolicy::EverySecond) {
                    let fsync_w = w.clone();
                    tokio::spawn(async move { aof::run_aof_fsync(fsync_w).await });
                }

                // AOF rewriter
                let threshold = config.aof.rewrite_threshold_mb * 1024 * 1024;
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

    if let Err(e) = run_server(config, engine, aof_writer).await {
        eprintln!("[ERROR] ZetDB: {e}");
        std::process::exit(1);
    }
}
