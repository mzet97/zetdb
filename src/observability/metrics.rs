use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::OnceLock;
use std::time::Instant;

/// Index into the per-command-type counter array.
#[derive(Clone, Copy)]
#[repr(usize)]
pub enum CommandType {
    Ping = 0,
    Get = 1,
    Set = 2,
    Del = 3,
    Incr = 4,
    Info = 5,
    DbSize = 6,
    Exists = 7,
    Ttl = 8,
    Expire = 9,
    FlushDb = 10,
    Keys = 11,
    Mget = 12,
    Mset = 13,
}

const NUM_COMMAND_TYPES: usize = 14;

pub struct Metrics {
    pub commands_total: AtomicU64,
    commands_by_type: [AtomicU64; NUM_COMMAND_TYPES],
    pub connections_total: AtomicU64,
    pub connections_active: AtomicU64,
    pub keyspace_hits: AtomicU64,
    pub keyspace_misses: AtomicU64,
    pub errors_total: AtomicU64,
    pub start_time: Instant,
}

static METRICS: OnceLock<Metrics> = OnceLock::new();

pub fn metrics() -> &'static Metrics {
    METRICS.get_or_init(|| Metrics {
        commands_total: AtomicU64::new(0),
        commands_by_type: std::array::from_fn(|_| AtomicU64::new(0)),
        connections_total: AtomicU64::new(0),
        connections_active: AtomicU64::new(0),
        keyspace_hits: AtomicU64::new(0),
        keyspace_misses: AtomicU64::new(0),
        errors_total: AtomicU64::new(0),
        start_time: Instant::now(),
    })
}

impl Metrics {
    pub fn record_command(&self, cmd: CommandType) {
        self.commands_total.fetch_add(1, Ordering::Relaxed);
        self.commands_by_type[cmd as usize].fetch_add(1, Ordering::Relaxed);
    }

    pub fn command_count(&self, cmd: CommandType) -> u64 {
        self.commands_by_type[cmd as usize].load(Ordering::Relaxed)
    }

    pub fn uptime_secs(&self) -> u64 {
        self.start_time.elapsed().as_secs()
    }

    pub fn record_hit(&self) {
        self.keyspace_hits.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_miss(&self) {
        self.keyspace_misses.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_error(&self) {
        self.errors_total.fetch_add(1, Ordering::Relaxed);
    }

    pub fn connection_opened(&self) {
        self.connections_total.fetch_add(1, Ordering::Relaxed);
        self.connections_active.fetch_add(1, Ordering::Relaxed);
    }

    pub fn connection_closed(&self) {
        self.connections_active.fetch_sub(1, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn record_command_increments_total_and_per_type() {
        let m = Metrics::new_for_test();
        m.record_command(CommandType::Get);
        m.record_command(CommandType::Get);
        m.record_command(CommandType::Set);

        assert_eq!(m.commands_total.load(Ordering::Relaxed), 3);
        assert_eq!(m.command_count(CommandType::Get), 2);
        assert_eq!(m.command_count(CommandType::Set), 1);
        assert_eq!(m.command_count(CommandType::Ping), 0);
    }

    #[test]
    fn hit_miss_tracking() {
        let m = Metrics::new_for_test();
        m.record_hit();
        m.record_hit();
        m.record_miss();

        assert_eq!(m.keyspace_hits.load(Ordering::Relaxed), 2);
        assert_eq!(m.keyspace_misses.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn connection_tracking() {
        let m = Metrics::new_for_test();
        m.connection_opened();
        m.connection_opened();
        m.connection_closed();

        assert_eq!(m.connections_total.load(Ordering::Relaxed), 2);
        assert_eq!(m.connections_active.load(Ordering::Relaxed), 1);
    }

    impl Metrics {
        pub fn new_for_test() -> Self {
            Self {
                commands_total: AtomicU64::new(0),
                commands_by_type: std::array::from_fn(|_| AtomicU64::new(0)),
                connections_total: AtomicU64::new(0),
                connections_active: AtomicU64::new(0),
                keyspace_hits: AtomicU64::new(0),
                keyspace_misses: AtomicU64::new(0),
                errors_total: AtomicU64::new(0),
                start_time: Instant::now(),
            }
        }
    }
}
