# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- Hardening phase: zero unwrap in binary parsing, AOF race-free rewrite, write timeout, max_keys eviction

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
