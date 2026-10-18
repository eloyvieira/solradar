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
- Run unattended as a Linux **systemd** service, with automatic restart and startup at boot.

## Requirements

- Ubuntu/Debian Linux server (systemd), or another OS with equivalent tooling
- Rust stable (`cargo` and `rustc`)
- MySQL 8+ (or compatible MariaDB)
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

You can install the database locally or use an existing MySQL server. If installing locally:

```bash
sudo apt install -y mysql-server
sudo systemctl enable --now mysql
```

## 2. Create the database and a dedicated database user

From the project directory:

```bash
sudo mysql < sql/schema.sql
sudo mysql
```

Then run in the MySQL console, replacing the example password:

```sql
CREATE USER IF NOT EXISTS 'solradar_user'@'127.0.0.1' IDENTIFIED BY 'REPLACE_WITH_STRONG_PASSWORD';
GRANT SELECT, INSERT, UPDATE ON solradar.* TO 'solradar_user'@'127.0.0.1';
EXIT;
```

If using a remote database, create the user for the appropriate client host and ensure network access is securely restricted.

## 3. Configure and compile

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

**Important:** URL-encode reserved characters in the MySQL password inside `DATABASE_URL` (for example `@` becomes `%40`). `MAX_PAGES` is clamped to 1–10 by the current code. Respect the provider's current request quotas. The application waits between pages, but other requests and provider rules may affect rate limits.

Test in the foreground:

```bash
cargo run --release
```

Press `Ctrl+C` to stop the foreground run before configuring systemd.

## 4. Install as a systemd background service

Run these commands **from the project's directory** after building and editing `.env`:

```bash
# Create a restricted service account and deployment folder.
sudo useradd --system --home-dir /opt/solradar --shell /usr/sbin/nologin solradar 2>/dev/null || true
sudo install -d -o solradar -g solradar -m 750 /opt/solradar

# Deploy compiled binary and private environment configuration.
sudo install -o root -g root -m 755 target/release/solradar /opt/solradar/solradar
sudo install -o root -g solradar -m 640 .env /opt/solradar/.env

# Register, start, and enable automatic start on boot.
sudo install -o root -g root -m 644 deploy/solradar.service /etc/systemd/system/solradar.service
sudo systemctl daemon-reload
sudo systemctl enable --now solradar
```

The service runs in the background, restarts after failures, and starts automatically following a server reboot.

### Service management

```bash
sudo systemctl status solradar                 # Current status
sudo journalctl -u solradar -f                # Live logs
sudo journalctl -u solradar -n 100 --no-pager # Last 100 log lines
sudo systemctl restart solradar              # Restart after changes
sudo systemctl stop solradar                 # Stop
sudo systemctl disable --now solradar        # Stop and disable at boot
```

### Deploying an update

From your source checkout:

```bash
cargo build --release
sudo systemctl stop solradar
sudo install -o root -g root -m 755 target/release/solradar /opt/solradar/solradar
sudo install -o root -g solradar -m 640 .env /opt/solradar/.env
sudo systemctl start solradar
```

If you modify the `.service` file, reinstall it and run `sudo systemctl daemon-reload` before restarting.

## Database tables

- `solana_pools`: latest pool attributes; primary key = pool address.
- `solana_pool_snapshots`: historical observations, deduplicated per pool/timestamp, including `volume_acceleration_5m` and `pump_candidate`.

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
