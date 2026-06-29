use std::cell::RefCell;
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

/// Per-thread counters to avoid cache contention on AtomicU64.
/// Aggregated periodically to global metrics.
#[derive(Default)]
struct LocalCounters {
    commands_total: u64,
    commands_by_type: [u64; NUM_COMMAND_TYPES],
    keyspace_hits: u64,
    keyspace_misses: u64,
    errors_total: u64,
}

thread_local! {
    static LOCAL_COUNTERS: RefCell<LocalCounters> = RefCell::new(LocalCounters::default());
}

/// Batch size for local counter flush to global metrics.
/// Flushing every 64 commands balances cache locality vs contention.
const FLUSH_BATCH: u64 = 64;

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
    /// Record a command - uses thread-local counters to avoid cache contention.
    #[inline(always)]
    pub fn record_command(&self, cmd: CommandType) {
        LOCAL_COUNTERS.with(|local| {
            let mut local = local.borrow_mut();
            local.commands_total += 1;
            local.commands_by_type[cmd as usize] += 1;
            
            // Flush to global metrics every FLUSH_BATCH commands
            if local.commands_total % FLUSH_BATCH == 0 {
                self.commands_total.fetch_add(FLUSH_BATCH, Ordering::Relaxed);
                self.commands_by_type[cmd as usize].fetch_add(FLUSH_BATCH, Ordering::Relaxed);
                local.commands_total -= FLUSH_BATCH;
                local.commands_by_type[cmd as usize] -= FLUSH_BATCH;
            }
        });
    }

    pub fn command_count(&self, cmd: CommandType) -> u64 {
        // Sum global + all thread-local counters
        let global = self.commands_by_type[cmd as usize].load(Ordering::Relaxed);
        let local_sum: u64 = LOCAL_COUNTERS.with(|local| {
            local.borrow().commands_by_type[cmd as usize]
        });
        global + local_sum
    }

    pub fn total_commands(&self) -> u64 {
        let global = self.commands_total.load(Ordering::Relaxed);
        let local_sum: u64 = LOCAL_COUNTERS.with(|local| {
            local.borrow().commands_total
        });
        global + local_sum
    }

    pub fn uptime_secs(&self) -> u64 {
        self.start_time.elapsed().as_secs()
    }

    /// Record a cache hit - uses thread-local counters.
    #[inline(always)]
    pub fn record_hit(&self) {
        LOCAL_COUNTERS.with(|local| {
            let mut local = local.borrow_mut();
            local.keyspace_hits += 1;
            if local.keyspace_hits % FLUSH_BATCH == 0 {
                self.keyspace_hits.fetch_add(FLUSH_BATCH, Ordering::Relaxed);
                local.keyspace_hits -= FLUSH_BATCH;
            }
        });
    }

    /// Record a cache miss - uses thread-local counters.
    #[inline(always)]
    pub fn record_miss(&self) {
        LOCAL_COUNTERS.with(|local| {
            let mut local = local.borrow_mut();
            local.keyspace_misses += 1;
            if local.keyspace_misses % FLUSH_BATCH == 0 {
                self.keyspace_misses.fetch_add(FLUSH_BATCH, Ordering::Relaxed);
                local.keyspace_misses -= FLUSH_BATCH;
            }
        });
    }

    /// Record an error - uses thread-local counters.
    #[inline(always)]
    pub fn record_error(&self) {
        LOCAL_COUNTERS.with(|local| {
            let mut local = local.borrow_mut();
            local.errors_total += 1;
            if local.errors_total % FLUSH_BATCH == 0 {
                self.errors_total.fetch_add(FLUSH_BATCH, Ordering::Relaxed);
                local.errors_total -= FLUSH_BATCH;
            }
        });
    }

    pub fn connection_opened(&self) {
        self.connections_total.fetch_add(1, Ordering::Relaxed);
        self.connections_active.fetch_add(1, Ordering::Relaxed);
    }

    pub fn connection_closed(&self) {
        self.connections_active.fetch_sub(1, Ordering::Relaxed);
    }

    /// Flush all thread-local counters to global metrics.
    /// Call this before reading metrics (e.g., for INFO command).
    pub fn flush_all(&self) {
        LOCAL_COUNTERS.with(|local| {
            let mut local = local.borrow_mut();
            if local.commands_total > 0 {
                self.commands_total.fetch_add(local.commands_total, Ordering::Relaxed);
                local.commands_total = 0;
            }
            if local.keyspace_hits > 0 {
                self.keyspace_hits.fetch_add(local.keyspace_hits, Ordering::Relaxed);
                local.keyspace_hits = 0;
            }
            if local.keyspace_misses > 0 {
                self.keyspace_misses.fetch_add(local.keyspace_misses, Ordering::Relaxed);
                local.keyspace_misses = 0;
            }
            if local.errors_total > 0 {
                self.errors_total.fetch_add(local.errors_total, Ordering::Relaxed);
                local.errors_total = 0;
            }
            for i in 0..NUM_COMMAND_TYPES {
                if local.commands_by_type[i] > 0 {
                    self.commands_by_type[i].fetch_add(local.commands_by_type[i], Ordering::Relaxed);
                    local.commands_by_type[i] = 0;
                }
            }
        });
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
        m.flush_all();

        // After flush, total should include all commands (including unflushed batch)
        assert_eq!(m.total_commands(), 3);
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
        m.flush_all();

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
