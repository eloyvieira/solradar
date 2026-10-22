#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
echo 'Atenção: aplicar schema somente em banco novo ou após backup e revisão das migrations.'
read -r -p 'Digite APLICAR para prosseguir: ' confirm
[[ "$confirm" == 'APLICAR' ]] || exit 1
docker compose --env-file .env -f deploy/docker-compose.yml exec -T mysql sh -c 'MYSQL_PWD="$MYSQL_PASSWORD" mysql -usolradar solradar' < migrations/001_schema.sql
