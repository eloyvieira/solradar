use anyhow::{
	Context,Result
};
use sqlx::{
	mysql::MySqlPoolOptions,MySqlPool
};
pub async fn connect() -> Result<MySqlPool> {
	let url=std::env::var("DATABASE_URL").context("DATABASE_URL não definida")?;
	Ok(
		MySqlPoolOptions::new()
						.max_connections(8)
						.connect(&url)
						.await?
	)
}
