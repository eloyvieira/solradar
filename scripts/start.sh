#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
[[ -f .env ]] || { echo '.env ausente; execute scripts/setup.sh'; exit 1; }
docker compose --env-file .env -f deploy/docker-compose.yml up -d --build "$@"
