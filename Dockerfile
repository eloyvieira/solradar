FROM rust:1.88-bookworm AS builder
WORKDIR /build

COPY Cargo.toml Cargo.toml
COPY crates/ crates/
COPY services/ services/

RUN cargo build --release --workspace
FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates && rm -rf /var/lib/apt/lists/*

COPY --from=builder /build/target/release/collector /usr/local/bin/collector
COPY --from=builder /build/target/release/arbitrage-detector /usr/local/bin/arbitrage-detector
COPY --from=builder /build/target/release/pump-detector /usr/local/bin/pump-detector
COPY --from=builder /build/target/release/security-analyzer /usr/local/bin/security-analyzer
COPY --from=builder /build/target/release/executor /usr/local/bin/executor
COPY --from=builder /build/target/release/position-monitor /usr/local/bin/position-monitor
COPY --from=builder /build/target/release/trade-ledger /usr/local/bin/trade-ledger
CMD ["collector"]
