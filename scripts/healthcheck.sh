#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
docker compose --env-file .env -f deploy/docker-compose.yml ps
printf '\nPool count / snapshots / signals / orders:\n'
docker compose --env-file .env -f deploy/docker-compose.yml exec -T mysql sh -c 'MYSQL_PWD="$MYSQL_PASSWORD" mysql -usolradar solradar -e "SELECT (SELECT COUNT(*) FROM solana_pools) pools,(SELECT COUNT(*) FROM solana_pool_snapshots) snapshots,(SELECT COUNT(*) FROM trade_signals) signals,(SELECT COUNT(*) FROM trade_orders) orders;"'
