use anyhow::{Result,Context};
use serde_json::{json,Value};
use solradar_common::{database,config};
use sqlx::Row;
use std::time::Duration;

#[tokio::main]async fn main()->Result<()>{
	solradar_common::init();
	let db=database().await?;
	let url=std::env::var("SOLANA_RPC_URL").context("SOLANA_RPC_URL faltando")?;
	let client=reqwest::Client::builder().timeout(Duration::from_secs(15)).build()?;
	loop{
		let tokens=sqlx::query("SELECT DISTINCT p.token_address FROM solana_pools p LEFT JOIN token_security s ON s.token_address=p.token_address WHERE s.token_address IS NULL OR s.expires_at<UTC_TIMESTAMP() LIMIT 100").fetch_all(&db).await?;
		for r in tokens{
			let token:String=r.get("token_address");
			let payload=json!({
				"jsonrpc":"2.0",
				"id":1,
				"method":"getAccountInfo",
				"params":[
					token,
					{"encoding":"jsonParsed"}
				]
			});
			let mut status="PENDING";
			let mut mint:Option<bool>=None;
			let mut freeze:Option<bool>=None;
			let mut notes="RPC inconclusivo ou token não suportado".to_string();
			if let Ok(resp)=client.post(&url).json(&payload).send().await{
				if let Ok(j)=resp.json::<Value>().await{
					let acct=&j["result"]["value"];
					let info=&acct["data"]["parsed"]["info"];
					let owner=acct["owner"].as_str().unwrap_or("");
					let is_legacy=owner=="TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA";
					if info.is_object()&&acct["data"]["parsed"]["type"]=="mint"{
						mint=Some(info["mintAuthority"].is_null());freeze=Some(info["freezeAuthority"].is_null());
						if is_legacy{
							status=if mint==Some(true)&&freeze==Some(true){"APPROVED"}else{"REJECTED"};
							notes="Aprovação parcial: somente autoridades SPL; concentração, LP e risco de mercado não verificados".into();
						}
						else{
							notes="Token-2022 ou programa desconhecido: extensões e riscos requerem verificação adicional".into();
						}
					}
				}
			}
			
			sqlx::query("INSERT INTO token_security(token_address,status,mint_authority_revoked,freeze_authority_revoked,notes,checked_at,expires_at) VALUES(?,?,?,?,?,UTC_TIMESTAMP(6),DATE_ADD(UTC_TIMESTAMP(6),INTERVAL 15 MINUTE)) ON DUPLICATE KEY UPDATE status=VALUES(status),mint_authority_revoked=VALUES(mint_authority_revoked),freeze_authority_revoked=VALUES(freeze_authority_revoked),notes=VALUES(notes),checked_at=VALUES(checked_at),expires_at=VALUES(expires_at)")
				.bind(&token)
				.bind(status)
				.bind(mint)
				.bind(freeze)
				.bind(&notes)
				.execute(&db)
				.await?;
		
			tokio::time::sleep(Duration::from_millis(config("SECURITY_RPC_SPACING_MS",200_u64))).await;
		}
		tokio::time::sleep(Duration::from_secs(config("SECURITY_SCAN_SECONDS",60_u64))).await;
	}
}
