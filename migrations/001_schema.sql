-- phpMyAdmin SQL Dump
-- version 5.2.1
-- https://www.phpmyadmin.net/
--
-- Host: localhost
-- Generation Time: Oct 27, 2026 at 10:38 PM
-- Server version: 10.6.23-MariaDB-0ubuntu0.22.04.1
-- PHP Version: 8.1.34

SET SQL_MODE = "NO_AUTO_VALUE_ON_ZERO";
START TRANSACTION;
SET time_zone = "+00:00";


/*!40101 SET @OLD_CHARACTER_SET_CLIENT=@@CHARACTER_SET_CLIENT */;
/*!40101 SET @OLD_CHARACTER_SET_RESULTS=@@CHARACTER_SET_RESULTS */;
/*!40101 SET @OLD_COLLATION_CONNECTION=@@COLLATION_CONNECTION */;
/*!40101 SET NAMES utf8mb4 */;

--
-- Database: `solradar`
--

-- --------------------------------------------------------

--
-- Table structure for table `positions`
--

CREATE TABLE `positions` (
  `id` char(36) NOT NULL,
  `order_id` char(36) NOT NULL,
  `token_address` varchar(80) NOT NULL,
  `pool_address` varchar(80) NOT NULL,
  `quantity` decimal(36,18) NOT NULL,
  `entry_price_usd` decimal(36,18) NOT NULL,
  `peak_price_usd` decimal(36,18) NOT NULL,
  `stop_price_usd` decimal(36,18) NOT NULL,
  `status` varchar(20) NOT NULL DEFAULT 'OPEN',
  `created_at` datetime(6) NOT NULL DEFAULT current_timestamp(6),
  `updated_at` datetime(6) NOT NULL DEFAULT current_timestamp(6) ON UPDATE current_timestamp(6)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci;

-- --------------------------------------------------------

--
-- Table structure for table `signals_arbitrage`
--

CREATE TABLE `signals_arbitrage` (
  `id` char(36) NOT NULL,
  `token_address` varchar(80) NOT NULL,
  `pool_address` varchar(80) NOT NULL,
  `secondary_pool` varchar(80) NOT NULL,
  `quote_token_address` varchar(80) NOT NULL,
  `price_a` decimal(36,18) NOT NULL,
  `price_b` decimal(36,18) NOT NULL,
  `spread_pct` decimal(18,8) NOT NULL,
  `observed_at` datetime(6) NOT NULL,
  `created_at` datetime(6) NOT NULL DEFAULT current_timestamp(6),
  `reason` text DEFAULT NULL,
  `status` varchar(30) NOT NULL DEFAULT 'OBSERVED',
  `executable` tinyint(1) NOT NULL DEFAULT 0
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci;

-- --------------------------------------------------------

--
-- Table structure for table `signals_pump`
--

CREATE TABLE `signals_pump` (
  `id` char(36) NOT NULL,
  `token_address` varchar(80) NOT NULL,
  `pool_address` varchar(80) NOT NULL,
  `observed_at` datetime(6) NOT NULL,
  `reason` text NOT NULL,
  `status` varchar(30) NOT NULL DEFAULT 'NEW',
  `created_at` datetime(6) NOT NULL DEFAULT current_timestamp(6)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci;

-- --------------------------------------------------------

--
-- Table structure for table `solana_pools`
--

CREATE TABLE `solana_pools` (
  `pool_address` varchar(80) NOT NULL,
  `token_address` varchar(80) NOT NULL,
  `quote_token_address` varchar(80) DEFAULT NULL,
  `dex_id` varchar(100) DEFAULT NULL,
  `token_symbol` varchar(80) DEFAULT NULL,
  `token_name` varchar(255) DEFAULT NULL,
  `pair_name` varchar(255) DEFAULT NULL,
  `price_usd` decimal(36,18) NOT NULL DEFAULT 0.000000000000000000,
  `volume_m5_usd` decimal(36,12) NOT NULL DEFAULT 0.000000000000,
  `volume_h1_usd` decimal(36,12) NOT NULL DEFAULT 0.000000000000,
  `volume_h24_usd` decimal(36,12) NOT NULL DEFAULT 0.000000000000,
  `liquidity_usd` decimal(36,12) NOT NULL DEFAULT 0.000000000000,
  `buys_m5` int(10) UNSIGNED NOT NULL DEFAULT 0,
  `sells_m5` int(10) UNSIGNED NOT NULL DEFAULT 0,
  `change_m5_pct` decimal(18,8) NOT NULL DEFAULT 0.00000000,
  `fee_pct` decimal(12,8) DEFAULT NULL,
  `fee_type` varchar(20) DEFAULT NULL,
  `fee_updated_at` datetime DEFAULT NULL,
  `network_fee_lamports` bigint(20) UNSIGNED DEFAULT NULL,
  `network_fee_updated_at` datetime(6) DEFAULT NULL,
  `first_seen` datetime(6) NOT NULL,
  `last_seen` datetime(6) NOT NULL,
  `pool_created_at` datetime DEFAULT NULL
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci;

-- --------------------------------------------------------

--
-- Table structure for table `solana_pool_snapshots`
--

CREATE TABLE `solana_pool_snapshots` (
  `pool_address` varchar(80) NOT NULL,
  `captured_at` datetime NOT NULL,
  `price_usd` decimal(36,18) NOT NULL DEFAULT 0.000000000000000000,
  `volume_m5_usd` decimal(36,12) NOT NULL DEFAULT 0.000000000000,
  `volume_h1_usd` decimal(36,12) NOT NULL DEFAULT 0.000000000000,
  `volume_h24_usd` decimal(36,12) NOT NULL DEFAULT 0.000000000000,
  `liquidity_usd` decimal(36,12) NOT NULL DEFAULT 0.000000000000,
  `buys_m5` int(10) UNSIGNED NOT NULL DEFAULT 0,
  `sells_m5` int(10) UNSIGNED NOT NULL DEFAULT 0,
  `change_m5_pct` decimal(18,8) NOT NULL DEFAULT 0.00000000,
  `volume_acceleration_5m` decimal(24,8) DEFAULT NULL,
  `pump_candidate` tinyint(1) NOT NULL DEFAULT 0,
  `volume_m15_usd` decimal(36,12) DEFAULT NULL,
  `volume_m30_usd` decimal(36,12) DEFAULT NULL,
  `volume_h6_usd` decimal(36,12) DEFAULT NULL,
  `change_m15_pct` decimal(18,8) DEFAULT NULL,
  `change_m30_pct` decimal(18,8) DEFAULT NULL,
  `change_h1_pct` decimal(18,8) DEFAULT NULL,
  `change_h6_pct` decimal(18,8) DEFAULT NULL,
  `change_h24_pct` decimal(18,8) DEFAULT NULL,
  `buys_m15` int(10) UNSIGNED DEFAULT NULL,
  `sells_m15` int(10) UNSIGNED DEFAULT NULL,
  `buys_m30` int(10) UNSIGNED DEFAULT NULL,
  `sells_m30` int(10) UNSIGNED DEFAULT NULL,
  `buys_h1` int(10) UNSIGNED DEFAULT NULL,
  `sells_h1` int(10) UNSIGNED DEFAULT NULL,
  `buys_h24` int(10) UNSIGNED DEFAULT NULL,
  `sells_h24` int(10) UNSIGNED DEFAULT NULL,
  `buyers_m5` int(10) UNSIGNED DEFAULT NULL,
  `sellers_m5` int(10) UNSIGNED DEFAULT NULL,
  `buyers_m15` int(10) UNSIGNED DEFAULT NULL,
  `sellers_m15` int(10) UNSIGNED DEFAULT NULL,
  `buyers_m30` int(10) UNSIGNED DEFAULT NULL,
  `sellers_m30` int(10) UNSIGNED DEFAULT NULL,
  `buyers_h1` int(10) UNSIGNED DEFAULT NULL,
  `sellers_h1` int(10) UNSIGNED DEFAULT NULL,
  `buyers_h24` int(10) UNSIGNED DEFAULT NULL,
  `sellers_h24` int(10) UNSIGNED DEFAULT NULL,
  `fdv_usd` decimal(36,12) DEFAULT NULL,
  `market_cap_usd` decimal(36,12) DEFAULT NULL
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci;

-- --------------------------------------------------------

--
-- Table structure for table `token_security`
--

CREATE TABLE `token_security` (
  `token_address` varchar(80) NOT NULL,
  `status` enum('APPROVED','REJECTED','PENDING') NOT NULL DEFAULT 'PENDING',
  `mint_authority_revoked` tinyint(1) DEFAULT NULL,
  `freeze_authority_revoked` tinyint(1) DEFAULT NULL,
  `notes` text NOT NULL,
  `checked_at` datetime(6) NOT NULL,
  `expires_at` datetime(6) NOT NULL
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci;

-- --------------------------------------------------------

--
-- Table structure for table `trade_executions`
--

CREATE TABLE `trade_executions` (
  `id` bigint(20) UNSIGNED NOT NULL,
  `order_id` char(36) NOT NULL,
  `tx_signature` varchar(150) DEFAULT NULL,
  `status` varchar(30) NOT NULL,
  `fee_sol` decimal(30,12) DEFAULT NULL,
  `amount_in` decimal(36,18) DEFAULT NULL,
  `amount_out` decimal(36,18) DEFAULT NULL,
  `recorded_at` datetime(6) NOT NULL DEFAULT current_timestamp(6)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci;

-- --------------------------------------------------------

--
-- Table structure for table `trade_orders`
--

CREATE TABLE `trade_orders` (
  `id` char(36) NOT NULL,
  `signal_id` char(36) DEFAULT NULL,
  `strategy` varchar(20) NOT NULL,
  `token_address` varchar(80) NOT NULL,
  `pool_address` varchar(80) NOT NULL,
  `status` varchar(30) NOT NULL,
  `mode` varchar(20) NOT NULL,
  `amount_in` decimal(36,18) DEFAULT NULL,
  `amount_out` decimal(36,18) DEFAULT NULL,
  `tx_signature` varchar(150) DEFAULT NULL,
  `error_message` text DEFAULT NULL,
  `created_at` datetime(6) NOT NULL DEFAULT current_timestamp(6),
  `updated_at` datetime(6) NOT NULL DEFAULT current_timestamp(6) ON UPDATE current_timestamp(6)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci;

-- --------------------------------------------------------

--
-- Table structure for table `trade_order_logs`
--

CREATE TABLE `trade_order_logs` (
  `id` bigint(20) UNSIGNED NOT NULL,
  `signal_id` char(36) NOT NULL,
  `order_id` char(36) DEFAULT NULL,
  `step` varchar(60) NOT NULL,
  `status` varchar(20) NOT NULL,
  `details` text DEFAULT NULL,
  `created_at` datetime(6) NOT NULL
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci;

-- --------------------------------------------------------

--
-- Table structure for table `trade_results`
--

CREATE TABLE `trade_results` (
  `order_id` char(36) NOT NULL,
  `status` varchar(30) NOT NULL,
  `realized_pnl_usd` decimal(36,12) DEFAULT NULL,
  `notes` text DEFAULT NULL,
  `reconciled_at` datetime(6) NOT NULL DEFAULT current_timestamp(6)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci;

--
-- Indexes for dumped tables
--

--
-- Indexes for table `positions`
--
ALTER TABLE `positions`
  ADD PRIMARY KEY (`id`);

--
-- Indexes for table `signals_arbitrage`
--
ALTER TABLE `signals_arbitrage`
  ADD PRIMARY KEY (`id`),
  ADD UNIQUE KEY `uq_arbitrage` (`pool_address`,`secondary_pool`,`observed_at`),
  ADD KEY `idx_token_time` (`token_address`,`observed_at`);

--
-- Indexes for table `signals_pump`
--
ALTER TABLE `signals_pump`
  ADD PRIMARY KEY (`id`),
  ADD UNIQUE KEY `uq_signal` (`pool_address`,`observed_at`),
  ADD KEY `idx_token_time` (`token_address`,`observed_at`);

--
-- Indexes for table `solana_pools`
--
ALTER TABLE `solana_pools`
  ADD PRIMARY KEY (`pool_address`),
  ADD KEY `idx_token` (`token_address`),
  ADD KEY `idx_last_seen` (`last_seen`);

--
-- Indexes for table `solana_pool_snapshots`
--
ALTER TABLE `solana_pool_snapshots`
  ADD PRIMARY KEY (`pool_address`,`captured_at`),
  ADD KEY `idx_captured` (`captured_at`);

--
-- Indexes for table `token_security`
--
ALTER TABLE `token_security`
  ADD PRIMARY KEY (`token_address`);

--
-- Indexes for table `trade_executions`
--
ALTER TABLE `trade_executions`
  ADD PRIMARY KEY (`id`),
  ADD KEY `idx_order` (`order_id`);

--
-- Indexes for table `trade_orders`
--
ALTER TABLE `trade_orders`
  ADD PRIMARY KEY (`id`),
  ADD UNIQUE KEY `uq_order_signal` (`signal_id`);

--
-- Indexes for table `trade_order_logs`
--
ALTER TABLE `trade_order_logs`
  ADD PRIMARY KEY (`id`),
  ADD KEY `idx_signal_id` (`signal_id`),
  ADD KEY `idx_order_id` (`order_id`),
  ADD KEY `idx_created_at` (`created_at`);

--
-- Indexes for table `trade_results`
--
ALTER TABLE `trade_results`
  ADD PRIMARY KEY (`order_id`);

--
-- AUTO_INCREMENT for dumped tables
--

--
-- AUTO_INCREMENT for table `trade_executions`
--
ALTER TABLE `trade_executions`
  MODIFY `id` bigint(20) UNSIGNED NOT NULL AUTO_INCREMENT;

--
-- AUTO_INCREMENT for table `trade_order_logs`
--
ALTER TABLE `trade_order_logs`
  MODIFY `id` bigint(20) UNSIGNED NOT NULL AUTO_INCREMENT;
COMMIT;

/*!40101 SET CHARACTER_SET_CLIENT=@OLD_CHARACTER_SET_CLIENT */;
/*!40101 SET CHARACTER_SET_RESULTS=@OLD_CHARACTER_SET_RESULTS */;
/*!40101 SET COLLATION_CONNECTION=@OLD_COLLATION_CONNECTION */;
