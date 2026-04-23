use std::sync::Arc;
use std::time::Duration;

use crate::storage::dashmap_engine::DashMapEngine;

pub async fn run_sweeper(engine: Arc<DashMapEngine>, interval: Duration) {
    let mut ticker = tokio::time::interval(interval);
    loop {
        ticker.tick().await;
        engine.sweep_expired();
    }
}
