# --- Stage 1: Build ---
FROM rust:1.85-bookworm AS builder

WORKDIR /usr/src/zetdb

# Cache dependencies
COPY Cargo.toml Cargo.lock ./
RUN mkdir src && echo "fn main() {}" > src/main.rs && \
    cargo build --release && rm -rf src

# Build actual binary
COPY . .
RUN touch src/main.rs && cargo build --release

# --- Stage 2: Runtime ---
FROM debian:bookworm-slim

RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates && rm -rf /var/lib/apt/lists/*

COPY --from=builder /usr/src/zetdb/target/release/zetdb /usr/local/bin/zetdb

EXPOSE 6379
VOLUME ["/data"]

ENTRYPOINT ["zetdb", "--snapshot-path", "/data/dump.zdb", "--aof-path", "/data/appendonly.zdb"]
