/// CPU affinity utilities for pinning threads to specific cores.
/// This improves cache locality and reduces context switching overhead.
/// Pin the current thread to a specific CPU core.
/// This is a no-op on non-Linux platforms.
#[cfg(target_os = "linux")]
pub fn pin_thread_to_cpu(cpu_id: usize) {
    use libc::{cpu_set_t, sched_setaffinity, CPU_SET, CPU_ZERO};
    
    unsafe {
        let mut cpuset: cpu_set_t = std::mem::zeroed();
        CPU_ZERO(&mut cpuset);
        CPU_SET(cpu_id, &mut cpuset);
        
        let pid = 0; // 0 means current thread
        let result = sched_setaffinity(pid, std::mem::size_of::<cpu_set_t>(), &cpuset);
        
        if result != 0 {
            log::warn!("Failed to pin thread to CPU {}: errno={}", cpu_id, *libc::__errno_location());
        }
    }
}

/// Pin the current thread to a specific CPU core.
/// No-op on non-Linux platforms.
#[cfg(not(target_os = "linux"))]
pub fn pin_thread_to_cpu(_cpu_id: usize) {
    // No-op on non-Linux platforms
}

/// Get the number of available CPU cores.
pub fn num_cpus() -> usize {
    std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1)
}

/// Pin a set of worker threads to specific CPU cores in a round-robin fashion.
/// This is useful for the Tokio runtime worker threads.
pub fn pin_worker_threads(num_workers: usize) {
    let num_cpus = num_cpus();
    
    for i in 0..num_workers {
        let cpu_id = i % num_cpus;
        pin_thread_to_cpu(cpu_id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_num_cpus() {
        let n = num_cpus();
        assert!(n >= 1);
        println!("Available CPUs: {}", n);
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn test_pin_thread() {
        // Just verify it doesn't panic
        pin_thread_to_cpu(0);
    }
}
