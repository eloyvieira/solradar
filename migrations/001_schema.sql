-- MySQL 8.0+ / MariaDB recente: ajuste conforme sua instalação.
CREATE TABLE IF NOT EXISTS solana_pools (
 pool_address VARCHAR(80) PRIMARY KEY, token_address VARCHAR(80) NOT NULL,
 quote_token_address VARCHAR(80) DEFAULT NULL, dex_id VARCHAR(100) DEFAULT NULL,
 token_symbol VARCHAR(80), token_name VARCHAR(255), pair_name VARCHAR(255),
 price_usd DECIMAL(36,18) NOT NULL DEFAULT 0, volume_m5_usd DECIMAL(36,12) NOT NULL DEFAULT 0,
 volume_h1_usd DECIMAL(36,12) NOT NULL DEFAULT 0,volume_h24_usd DECIMAL(36,12) NOT NULL DEFAULT 0,
 liquidity_usd DECIMAL(36,12) NOT NULL DEFAULT 0,
 buys_m5 INT UNSIGNED NOT NULL DEFAULT 0,sells_m5 INT UNSIGNED NOT NULL DEFAULT 0,
 change_m5_pct DECIMAL(18,8) NOT NULL DEFAULT 0,
 first_seen DATETIME(6) NOT NULL,last_seen DATETIME(6) NOT NULL,
 INDEX idx_token (token_address),INDEX idx_last_seen (last_seen)
);

CREATE TABLE IF NOT EXISTS solana_pool_snapshots (
 pool_address VARCHAR(80) NOT NULL,captured_at DATETIME NOT NULL,
 price_usd DECIMAL(36,18) NOT NULL DEFAULT 0,
 volume_m5_usd DECIMAL(36,12) NOT NULL DEFAULT 0,volume_h1_usd DECIMAL(36,12) NOT NULL DEFAULT 0,
 volume_h24_usd DECIMAL(36,12) NOT NULL DEFAULT 0,liquidity_usd DECIMAL(36,12) NOT NULL DEFAULT 0,
 buys_m5 INT UNSIGNED NOT NULL DEFAULT 0,sells_m5 INT UNSIGNED NOT NULL DEFAULT 0,
 change_m5_pct DECIMAL(18,8) NOT NULL DEFAULT 0,
 volume_acceleration_5m DECIMAL(24,8) NULL,pump_candidate BOOLEAN NOT NULL DEFAULT FALSE,
 PRIMARY KEY (pool_address,captured_at),INDEX idx_captured (captured_at)
);
CREATE TABLE IF NOT EXISTS token_security (
 token_address VARCHAR(80) PRIMARY KEY,status ENUM('APPROVED','REJECTED','PENDING') NOT NULL DEFAULT 'PENDING',
 mint_authority_revoked BOOLEAN NULL,freeze_authority_revoked BOOLEAN NULL,
 notes TEXT NOT NULL,checked_at DATETIME(6) NOT NULL,expires_at DATETIME(6) NOT NULL
);
CREATE TABLE IF NOT EXISTS trade_signals (
 id CHAR(36) PRIMARY KEY,strategy VARCHAR(20) NOT NULL,token_address VARCHAR(80) NOT NULL,
 pool_address VARCHAR(80) NOT NULL,secondary_pool VARCHAR(80) NULL,
 observed_at DATETIME(6) NOT NULL,reason TEXT NOT NULL,
 status VARCHAR(30) NOT NULL DEFAULT 'NEW',created_at DATETIME(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
 INDEX idx_token_time(token_address,observed_at),
 UNIQUE KEY uq_signal(strategy,pool_address,observed_at)
);
CREATE TABLE IF NOT EXISTS trade_orders (
 id CHAR(36) PRIMARY KEY,signal_id CHAR(36) NULL,strategy VARCHAR(20) NOT NULL,
 token_address VARCHAR(80) NOT NULL,pool_address VARCHAR(80) NOT NULL,
 status VARCHAR(30) NOT NULL,mode VARCHAR(20) NOT NULL,
 amount_in DECIMAL(36,18) NULL,amount_out DECIMAL(36,18) NULL,
 tx_signature VARCHAR(150) NULL,error_message TEXT NULL,
 created_at DATETIME(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),updated_at DATETIME(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6) ON UPDATE CURRENT_TIMESTAMP(6),
 UNIQUE KEY uq_order_signal(signal_id)
);
CREATE TABLE IF NOT EXISTS positions (
 id CHAR(36) PRIMARY KEY,order_id CHAR(36) NOT NULL,token_address VARCHAR(80) NOT NULL,
 pool_address VARCHAR(80) NOT NULL,quantity DECIMAL(36,18) NOT NULL,
 entry_price_usd DECIMAL(36,18) NOT NULL,peak_price_usd DECIMAL(36,18) NOT NULL,
 stop_price_usd DECIMAL(36,18) NOT NULL,status VARCHAR(20) NOT NULL DEFAULT 'OPEN',
 created_at DATETIME(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),updated_at DATETIME(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6) ON UPDATE CURRENT_TIMESTAMP(6)
);
CREATE TABLE IF NOT EXISTS trade_executions (
 id BIGINT UNSIGNED PRIMARY KEY AUTO_INCREMENT,order_id CHAR(36) NOT NULL,
 tx_signature VARCHAR(150) NULL,status VARCHAR(30) NOT NULL,
 fee_sol DECIMAL(30,12) NULL,amount_in DECIMAL(36,18) NULL,amount_out DECIMAL(36,18) NULL,
 recorded_at DATETIME(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),INDEX idx_order(order_id)
);
CREATE TABLE IF NOT EXISTS trade_results (
 order_id CHAR(36) PRIMARY KEY,status VARCHAR(30) NOT NULL,
 realized_pnl_usd DECIMAL(36,12) NULL,notes TEXT NULL,
 reconciled_at DATETIME(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6)
);
