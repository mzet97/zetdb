# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.2.0] - 2026-11-16

### Added

- Hardening phase: zero unwrap in binary parsing, AOF race-free rewrite, write timeout, max_keys eviction
- Tests for max_keys eviction with MSET (H5)
- Dependency updates: tokio 1.52.3, bytes 1.12.0, dashmap 6.2.1, log 0.4.33, env_logger 0.11.11, memchr 2.8.2, serde_json 1.0.150
- CI/CD updates: GitHub Actions v6/v8, Docker rust:1.96-bookworm
- Hardened AOF replay and snapshot load against truncated files
- Improved error logging for AOF replay and snapshot load failures

### Fixed

- Clippy errors on Rust 1.96 (`manual_range_contains` in TTL asserts, `needless_borrows_for_generic_args` in AOF test)
- `cargo fmt` compliance across `src/` and `src/bin/`
- Docker image now binds `0.0.0.0` so published ports accept traffic (default `127.0.0.1` was unreachable from the host)

### Changed

- Dependencies: tokio 1.54.1, clap 4.6.7, serde 1.0.229, serde_json 1.0.154, bytes 1.12.1, memchr 2.8.3, crc32fast 1.5.5, libc 0.2.191, log 0.4.35, itoa 1.0.19, memmap2 0.9.12 (all caret-compatible, no `Cargo.toml` changes)
- Dockerfile builder `rust:1.96-bookworm` → `rust:1.98-bookworm`
- GitHub Actions: checkout v7, upload-artifact v7, attest-build-provenance v4, login-action v4, build-push-action v7, codeql-action v4, trivy-action pinned to 0.35.0 (was `@master`); new CI `audit` job running `cargo audit`

### Security

- `cargo audit` clean: 66 crates, 0 advisories; audit now enforced on every push/PR via CI

### Performance

- **Session handler optimizations** (WSL benchmark):
  - Fast path for write_timeout=0 (skips tokio::time::timeout overhead)
  - Inline protocol fast path (try_parse_frame_inline) - avoids RESP detection per command
  - Pre-allocation of write buffer based on estimated response count
  - Benchmark now runs with write_timeout=0 for zero timeout overhead
  - Results: SET +12.6% (1.74M → 1.96M ops/s), GET +2.6% (3.08M → 3.16M ops/s)
  - Note: Performance still ~75% below MVP baseline (12.16M GET / 7.63M SET) due to additional features (RESP protocol, AOF, metrics, more commands)

- **Zero-allocation parser (parser_borrowed.rs)**:
  - Created `ParsedCommand<'a>` with borrowed keys (zero-allocation hot path)
  - Inline parser avoids String allocation for keys and values
  - Manual decimal parsing for RESP integers (faster than from_utf8 + parse)
  - Commands reordered by frequency (GET/SET first)
  - AOF lazy evaluation: only computes to_aof_entry() when AOF is enabled
  - Fixed consumed calculation to correctly advance buffer per command (supports pipelining)
  - Fixed MGET/MSET parsing to handle multiple keys/pairs correctly
  - All 168 tests passing with zero-allocation parser

- **WSL Benchmark Results** (8 cores, 23GB RAM):
  - SET peak: **4.58M ops/s** (pipe=500, clients=16) - 60% of MVP baseline (7.63M)
  - GET peak: **17.4M ops/s** (pipe=500, clients=32) - **143% of MVP baseline (12.16M)**
  - MIXED peak: **5.24M total ops/s** (16w/16r pipe=200)
  - Note: GET now exceeds MVP baseline by 43% with optimized benchmark client

- **Redis Comparison** (RESP protocol, Ubuntu 24.04, 4 cores):
  - Redis 7.0.15: SET 1.05M ops/s, GET 1.85M ops/s
  - ZetDB: SET 1.09M ops/s, GET 2.26M ops/s
  - ZetDB is ~4% faster than Redis for SET and ~22% faster for GET on Linux native
  - Note: Both use RESP protocol in this comparison; ZetDB inline protocol is significantly faster

- **Redis Comparison WSL** (RESP protocol, 8 cores, 23GB RAM):
  - Redis 7.0.15: SET 288k ops/s (pipe=200, clients=32), GET 978k ops/s (pipe=100, clients=4)
  - ZetDB: SET 2.71M ops/s (pipe=500, clients=32), GET 3.75M ops/s (pipe=500, clients=32)
  - ZetDB is ~9.4x faster than Redis for SET and ~3.8x faster for GET on WSL with RESP protocol
  - Note: Redis WSL performance is lower than Linux native likely due to WSL networking overhead

- **Benchmark Client Fix**:
  - Fixed `redis_compare` not draining full RESP bulk-string responses (`$len\r\n<data>\r\n`)
  - Previous client read only the first line, causing connection backpressure and server livelock during GET pre-population
  - Added `read_resp_response()` helper to consume complete RESP frames

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

[unreleased]: https://github.com/mzet97/zetdb/compare/v0.2.0...HEAD
[0.2.0]: https://github.com/mzet97/zetdb/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/mzet97/zetdb/releases/tag/v0.1.0
