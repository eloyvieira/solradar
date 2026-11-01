mod arbitrage;
mod pump;
mod dex_router;
mod transaction_builder;

use anyhow::Result;
use std::sync::Arc;
use tokio::task::JoinSet;

use solradar_common::{database, init};

#[tokio::main]
async fn main() -> Result<()> {
    init();

    let db = database().await?;
    let db = Arc::new(db);
    let mut tasks = JoinSet::new();


    let arbitrage_db = Arc::clone(&db);
    tasks.spawn(async move {
        arbitrage::run(arbitrage_db).await
    });


    let pump_db = Arc::clone(&db);
    tasks.spawn(async move {
        pump::run(pump_db).await
    });


    match tasks.join_next().await {
        Some(Ok(Ok(()))) => {
            anyhow::bail!("An executor stopped unexpectedly")
        }
        Some(Ok(Err(err))) => Err(err),
        Some(Err(err)) => Err(err.into()),
        None => anyhow::bail!("No executors started"),
    }
}