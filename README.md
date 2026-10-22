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
docker compose --env-file .env -f deploy/docker-compose.yml up -d --force-recreate collector
docker compose --env-file .env -f deploy/docker-compose.yml up -d --build arbitrage-detector pump-detector

docker compose -f deploy/docker-compose.yml restart collector
docker compose -f deploy/docker-compose.yml stop collector

docker compose --env-file .env -f deploy/docker-compose.yml ps
docker compose --env-file .env -f deploy/docker-compose.yml logs -f collector
docker compose --env-file .env -f deploy/docker-compose.yml logs -f rabbitmq
docker compose --env-file .env -f deploy/docker-compose.yml logs -f arbitrage-detector pump-detector



        SOLANA / GECKOTERMINAL
                  |
                  v
              COLLECTOR
                  |
             RabbitMQ
                  |
      +-----------+-----------+
      |                       |
      v                       v
ARBITRAGE DETECTOR        PUMP DETECTOR
      |                       |
      +-----------+-----------+
                  |
                  v
          SECURITY CHECK <--------+
                  |               |
                  |          MySQL
                  |               ^
                  |               |
                  |       SECURITY ANALYZER
                  |               ^
                  v               |
               EXECUTOR           |
                  |               |
            SOLANA RPC       SOLANA RPC
                  |
         +--------+--------+
         |                 |
         v                 v
  POSITION MONITOR     TRADE LEDGER
         |
    Trailing Stop
         |
         v
      EXECUTOR
  (Venda da posição)