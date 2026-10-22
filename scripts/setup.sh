#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
if ! command -v docker >/dev/null; then echo 'Docker não instalado' >&2; exit 1; fi
if ! docker compose version >/dev/null; then echo 'Docker Compose v2 não instalado' >&2; exit 1; fi
if [[ ! -f .env ]]; then cp .env.example .env; echo 'Edite .env (senhas e URLs) antes de iniciar.'; exit 0; fi
echo 'Arquivo .env encontrado. Rode scripts/start.sh após conferir as credenciais.'
