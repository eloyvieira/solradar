# SolRadar — Solana Pump Monitor (Rust + MySQL)

SolRadar queries GeckoTerminal for Solana liquidity pools with **24-hour trading volume >= $300,000 USD**. It stores each pool's latest state and historical snapshots in MySQL, and flags potential pump activity using an experimental signal.

> This is a data collection and research tool. It does not execute trades. Coverage is limited to pools returned by the queried API pages.

## Features

- Discover high-volume Solana pools using GeckoTerminal top-pool pages.
- Save the latest state to `solana_pools` (upsert by pool address).
- Save time-series observations to `solana_pool_snapshots`.
- Track 5-minute, 1-hour, and 24-hour volume, price, liquidity, buy/sell counts, and 5-minute price changes.
- Flag `pump_candidate` using experimental volume acceleration and price/buy-pressure criteria.
- Poll on an adjustable schedule (default: **60 seconds**).
- Run unattended under **PM2**, with automatic restart and startup after server reboot.

## Requirements

- Ubuntu/Debian Linux server with a `p2` user account
- Rust stable (`cargo` and `rustc`)
- MySQL 8+ (or compatible MariaDB)
- Node.js and npm (to install PM2)
- Internet access to the GeckoTerminal API

## 1. Install build dependencies and Rust (Ubuntu/Debian)

```bash
sudo apt update
sudo apt install -y build-essential pkg-config ca-certificates curl unzip default-libmysqlclient-dev mysql-client
# Install Rust using the official rustup instructions: https://rustup.rs
# Open a new shell, then verify:
rustc --version
cargo --version
```

## 2. Configure and compile

```bash
cp .env.example .env
nano .env
cargo build --release
```

Example `.env`:

```dotenv
DATABASE_URL=mysql://solradar_user:REPLACE_WITH_STRONG_PASSWORD@127.0.0.1:3306/solradar
MIN_VOLUME_H24_USD=300000
MAX_PAGES=3
REQUEST_SPACING_SECONDS=7
SCAN_INTERVAL_SECONDS=60
MIN_LIQUIDITY_USD=0
```

Test in the foreground:

```bash
cargo run --release
```

### Install PM2

Install Node.js and npm if needed, then:

```bash
sudo npm install -g pm2
```

### Start SolRadar

Make sure `.env` is configured and the release binary has been built. Assuming your project is at `/home/p2/solradar`:

```bash
cd /home/p2/solradar
cargo build --release
pm2 start ./target/release/solradar --name solradar --interpreter none --cwd /home/p2/solradar
```

`--interpreter none` tells PM2 to run the compiled Rust executable directly.

### Manage the process

```bash
pm2 status               # Check running processes
pm2 logs solradar        # Follow application logs
pm2 restart solradar     # Restart the application
pm2 stop solradar        # Stop the application
pm2 delete solradar      # Remove the process from PM2
```

### Enable startup after reboot

As the `p2` user, run:

```bash
pm2 startup
```

Copy and run the **exact `sudo` command printed by PM2** to configure the startup service for the `p2` user. Then run:

```bash
pm2 save
```

The saved process list is restored when the server boots. If you add or delete PM2 processes later, run `pm2 save` again.

### Deploy updates

```bash
cd /home/p2/solradar
cargo build --release
pm2 restart solradar
```

When editing `.env`, restart the application to reload its dotenv configuration:

```bash
pm2 restart solradar
```

## Example SQL queries

Recent historical data:

```sql
SELECT p.token_symbol, p.pool_address, s.captured_at,
       s.volume_h24_usd, s.volume_m5_usd, s.price_usd, s.volume_acceleration_5m
FROM solana_pool_snapshots s
JOIN solana_pools p USING(pool_address)
ORDER BY s.captured_at DESC LIMIT 50;
```

Potential pump alerts:

```sql
SELECT p.token_symbol, p.pool_address, s.captured_at,
       s.volume_acceleration_5m, s.change_m5_pct
FROM solana_pool_snapshots s
JOIN solana_pools p USING(pool_address)
WHERE s.pump_candidate = 1
ORDER BY s.captured_at DESC LIMIT 50;
```

## Important limitations

- **Incomplete discovery:** top-pool API pagination is not a network-wide scan. The program queries at most 10 pages per cycle.
- **Pool-level volume:** values describe one pool, not the token's combined volume across all exchanges/pools.
- **History gaps:** only pools present in a polling cycle receive a snapshot for that cycle.
- **API quotas and caching:** the provider may cache values for around a minute and enforce rate limits; confirm current limits with the provider.
- **Experimental signal:** the pump flag is a research heuristic, not a guaranteed trading signal. No orders are placed.
- **Rolling windows:** comparing two overlapping 5-minute volume readings does not measure exact net inflow or dollar-denominated buys.
- **Expansion:** for broader coverage, add newly created/trending pool discovery, paid market feeds, or on-chain indexing.
