CREATE DATABASE IF NOT EXISTS solana_monitor CHARACTER SET utf8mb4 COLLATE utf8mb4_unicode_ci;
USE solana_monitor;

CREATE TABLE IF NOT EXISTS solana_pools (
  pool_address VARCHAR(64) NOT NULL PRIMARY KEY,
  token_address VARCHAR(64) NULL,
  token_symbol VARCHAR(100) NULL,
  token_name VARCHAR(255) NULL,
  pair_name VARCHAR(255) NULL,
  price_usd DECIMAL(36,18) NULL,
  volume_m5_usd DECIMAL(24,4) NULL,
  volume_h1_usd DECIMAL(24,4) NULL,
  volume_h24_usd DECIMAL(24,4) NOT NULL,
  liquidity_usd DECIMAL(24,4) NULL,
  buys_m5 INT UNSIGNED NULL,
  sells_m5 INT UNSIGNED NULL,
  change_m5_pct DECIMAL(16,6) NULL,
  first_seen DATETIME(3) NOT NULL,
  last_seen DATETIME(3) NOT NULL,
  INDEX idx_volume (volume_h24_usd),
  INDEX idx_token (token_address),
  INDEX idx_last_seen (last_seen)
);

CREATE TABLE IF NOT EXISTS solana_pool_snapshots (
  id BIGINT UNSIGNED NOT NULL AUTO_INCREMENT PRIMARY KEY,
  pool_address VARCHAR(64) NOT NULL,
  captured_at DATETIME(3) NOT NULL,
  price_usd DECIMAL(36,18) NULL,
  volume_m5_usd DECIMAL(24,4) NULL,
  volume_h1_usd DECIMAL(24,4) NULL,
  volume_h24_usd DECIMAL(24,4) NOT NULL,
  liquidity_usd DECIMAL(24,4) NULL,
  buys_m5 INT UNSIGNED NULL,
  sells_m5 INT UNSIGNED NULL,
  change_m5_pct DECIMAL(16,6) NULL,
  volume_acceleration_5m DECIMAL(18,6) NULL,
  pump_candidate BOOLEAN NOT NULL DEFAULT FALSE,
  UNIQUE KEY uniq_pool_minute (pool_address, captured_at),
  INDEX idx_pool_time (pool_address, captured_at),
  INDEX idx_pump_time (pump_candidate, captured_at),
  CONSTRAINT fk_snapshot_pool FOREIGN KEY (pool_address) REFERENCES solana_pools(pool_address)
);
