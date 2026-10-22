# SolRadar 

```bash
cp .env.example .env
bash scripts/start.sh
bash scripts/logs.sh collector pump-detector security-analyzer
bash scripts/healthcheck.sh
```

```bash
cargo test --workspace
cargo check --workspace
# ou bash scripts/check.sh
```

docker compose --env-file .env -f deploy/docker-compose.yml up -d --build collector
docker compose -f deploy/docker-compose.yml restart collector
docker compose -f deploy/docker-compose.yml stop collector
docker compose -f deploy/docker-compose.yml logs -f collector
docker compose -f deploy/docker-compose.yml ps