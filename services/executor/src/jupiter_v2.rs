use anyhow::{bail, Context, Result};
use serde::Deserialize;
use std::collections::HashMap;

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ApiAccount {
    pub pubkey: String,
    pub is_signer: bool,
    pub is_writable: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ApiInstruction {
    pub program_id: String,
    pub accounts: Vec<ApiAccount>,
    pub data: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RouteSwapInfo {
    pub amm_key: String,
    pub input_mint: String,
    pub output_mint: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RouteHop {
    pub swap_info: RouteSwapInfo,
    #[serde(default)]
    pub bps: Option<u32>,
    #[serde(default)]
    pub percent: Option<f64>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BuildResponse {
    pub input_mint: String,
    pub output_mint: String,
    pub in_amount: String,
    pub out_amount: String,
    pub other_amount_threshold: String,
    pub route_plan: Vec<RouteHop>,
    #[serde(default)]
    pub setup_instructions: Vec<ApiInstruction>,
    pub swap_instruction: ApiInstruction,
    #[serde(default)]
    pub cleanup_instruction: Option<ApiInstruction>,
    #[serde(default)]
    pub other_instructions: Vec<ApiInstruction>,
    #[serde(default)]
    pub addresses_by_lookup_table_address: Option<HashMap<String, Vec<String>>>,
}

impl BuildResponse {
    pub fn amount_out(&self) -> Result<u64> { Ok(self.out_amount.parse()?) }
    pub fn min_out(&self) -> Result<u64> { Ok(self.other_amount_threshold.parse()?) }

    pub fn verify_exact_pool(&self, pool: &str, input: &str, output: &str, amount: u64) -> Result<()> {
        if self.input_mint != input || self.output_mint != output || self.in_amount.parse::<u64>()? != amount {
            bail!("Jupiter returned a value/pair different from what was requested");
        }
        if self.route_plan.len() != 1 { bail!("Multi-hop or split route not accepted"); }
        let hop = &self.route_plan[0];
        if hop.swap_info.amm_key != pool || hop.swap_info.input_mint != input || hop.swap_info.output_mint != output {
            bail!("Jupiter selected a different pool than {}", pool);
        }
        if hop.bps != Some(10_000) && hop.percent != Some(100.0) {
            bail!("Route percentage is not 100%");
        }
        if self.min_out()? == 0 { bail!("Quote with a minimum departure requirement of zero"); }
        Ok(())
    }
}

pub async fn build(
    client: &reqwest::Client, 
    api_key: &str, 
    wallet: &str,
    input: &str, 
    output: &str, 
    amount: u64, 
    slippage_bps: u16,
) -> Result<BuildResponse> {

    if amount == 0 || slippage_bps == 0 || slippage_bps > 500 {
        bail!("Amount/slippage invalid");
    }
    let amount_str = amount.to_string();
    let slippage = slippage_bps.to_string();
    let response = client.get("https://api.jup.ag/swap/v2/build")
        .header("x-api-key", api_key)
        .query(&[
            ("inputMint", input), 
            ("outputMint", output),
            ("amount", amount_str.as_str()), 
            ("taker", wallet),
            ("slippageBps", slippage.as_str()),
            ("dexes", "Whirlpool"),
        ])
        .send()
        .await
        .context("Failed Jupiter /build")?;
    let status = response.status();
    if !status.is_success() {
        bail!("Jupiter /build HTTP {}: {}", status, response.text().await.unwrap_or_default());
    }
    
    let body = response.text().await?;
    match serde_json::from_str::<BuildResponse>(&body) {
        Ok(data) => Ok(data),
        Err(err) => {
            bail!(
                "JSON /build invalid: {} | response: {}",
                err,
                body.chars().take(4000).collect::<String>()
            );
        }
    }

}
