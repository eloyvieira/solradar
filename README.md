# SolRadar

SolRadar is an experimental algorithmic trading project for the **Solana blockchain**, built with **Rust** and a microservices architecture.

The project focuses on monitoring liquidity pools, detecting arbitrage opportunities and market movements, and developing automated trading strategies.

**Status:** Work in Progress (WIP) — Under active development.

## Tech Stack

- Rust
- Solana RPC
- Jupiter API
- RabbitMQ
- MySQL
- Docker / Docker Compose

## Architecture

```text
         SOLANA / GECKOTERMINAL
                  |
               COLLECTOR
                  |
               RabbitMQ
                  |
       +----------+----------+
       |                     |
 ARBITRAGE DETECTOR     PUMP DETECTOR
       |                     |
       +----------+----------+
                  |
           SECURITY CHECK
                  |
               EXECUTOR
                  |
              SOLANA RPC
                  |
       +----------+----------+
       |                     |
 POSITION MONITOR       TRADE LEDGER
```

## Getting Started

```bash
cp .env.example .env
bash scripts/start.sh
```

## Development

```bash
cargo check --workspace
cargo test --workspace
```

## Logs

```bash
bash scripts/logs.sh collector arbitrage-detector pump-detector executor
```

## Disclaimer

This project is under development and is not ready for production trading.
