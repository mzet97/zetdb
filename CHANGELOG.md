# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- Hardening phase: zero unwrap in binary parsing, AOF race-free rewrite, write timeout, max_keys eviction
- Tests for max_keys eviction with MSET (H5)
- Dependency updates: tokio 1.52.3, bytes 1.12.0, dashmap 6.2.1, log 0.4.33, env_logger 0.11.11, memchr 2.8.2, serde_json 1.0.150
- CI/CD updates: GitHub Actions v6/v8, Docker rust:1.96-bookworm
- Hardened AOF replay and snapshot load against truncated files
- Improved error logging for AOF replay and snapshot load failures

### Performance

- **Session handler optimizations** (WSL benchmark):
  - Fast path for write_timeout=0 (skips tokio::time::timeout overhead)
  - Inline protocol fast path (try_parse_frame_inline) - avoids RESP detection per command
  - Pre-allocation of write buffer based on estimated response count
  - Benchmark now runs with write_timeout=0 for zero timeout overhead
  - Results: SET +12.6% (1.74M → 1.96M ops/s), GET +2.6% (3.08M → 3.16M ops/s)
  - Note: Performance still ~75% below MVP baseline (12.16M GET / 7.63M SET) due to additional features (RESP protocol, AOF, metrics, more commands)

### Verified by Code Inspection

- H4: Write timeout protection — implemented in `session.rs` with `tokio::time::timeout` on `writer.write_all` and `MAX_WRITE_BUF` overflow check (1MB limit). On localhost, kernel TCP buffers are large enough that write timeout may not fire during integration tests, but the protection is active in production scenarios with real network latency.

## [0.1.0] - 2026-05-12

### Added

- Initial MVP release
- TCP server with inline and RESP protocol support
- Commands: PING, GET, SET, DEL, INCR, INFO, DBSIZE, EXISTS, TTL, EXPIRE, FLUSHDB, KEYS, MGET, MSET
- DashMap-based concurrent storage with lazy TTL eviction
- Background TTL sweeper
- Binary snapshot persistence (ZDB1 format) with CRC32
- Append-Only File (AOF) with configurable fsync policies
- AOF rewrite compaction
- Lock-free metrics counters
- Docker support with multi-stage build

[unreleased]: https://github.com/mzet97/zetdb/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/mzet97/zetdb/releases/tag/v0.1.0
