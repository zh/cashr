/// Electrumx REST client for fulcrum-rust API.
///
/// Handles balance, UTXOs (including CashTokens), and broadcast via the
/// lightweight electrumx wrapper, bypassing the heavier mainnet-cash REST API.
use anyhow::{bail, Context, Result};
use serde::Deserialize;

// ── Token data from Electrum protocol 1.5 ────────────────────────────

/// CashToken data attached to a UTXO (protocol 1.5+).
#[derive(Debug, Clone, Deserialize)]
pub struct TokenData {
    pub category: String,
    #[serde(default)]
    pub amount: String,
    #[serde(default)]
    pub nft: Option<NftData>,
}

/// NFT-specific data within a CashToken UTXO.
#[derive(Debug, Clone, Deserialize)]
pub struct NftData {
    pub capability: String,
    #[serde(default)]
    pub commitment: String,
}

// ── UTXO types ───────────────────────────────────────────────────────

/// A UTXO returned by the electrumx API (with optional CashToken data).
#[derive(Debug, Clone)]
pub struct ElectrumxUtxo {
    pub txid: String,
    pub vout: u32,
    pub value: u64,
    pub token_data: Option<TokenData>,
}

// ── Deserialization structs ──────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct BalanceInner {
    confirmed: i64,
    unconfirmed: i64,
}

#[derive(Debug, Deserialize)]
struct BalanceResponse {
    success: bool,
    balance: Option<BalanceInner>,
    error: Option<String>,
}

#[derive(Debug, Deserialize)]
struct UtxoItem {
    tx_hash: String,
    tx_pos: u32,
    value: u64,
    #[allow(dead_code)]
    height: i64,
    token_data: Option<TokenData>,
}

#[derive(Debug, Deserialize)]
struct UtxosResponse {
    success: bool,
    utxos: Option<Vec<UtxoItem>>,
    error: Option<String>,
}

#[derive(Debug, Deserialize)]
struct BroadcastResponse {
    success: bool,
    txid: Option<serde_json::Value>,
    error: Option<String>,
}

// ── Client ───────────────────────────────────────────────────────────

/// Client for the fulcrum-rust electrumx REST wrapper.
#[derive(Clone)]
pub struct ElectrumxClient {
    client: reqwest::Client,
    base_url: String,
}

impl ElectrumxClient {
    /// Create a new client for the given base URL.
    pub fn new(base_url: &str) -> Self {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(15))
            .build()
            .unwrap_or_default();
        Self {
            client,
            base_url: base_url.trim_end_matches('/').to_string(),
        }
    }

    /// Get balance for a single address. Returns (confirmed, unconfirmed) in satoshis.
    pub async fn get_balance(&self, address: &str) -> Result<(i64, i64)> {
        let url = format!("{}/v1/electrumx/balance/{}", self.base_url, address);
        let resp: BalanceResponse = self
            .client
            .get(&url)
            .send()
            .await
            .context("electrumx balance request failed")?
            .json()
            .await
            .context("electrumx balance parse failed")?;

        if !resp.success {
            bail!(
                "electrumx balance error: {}",
                resp.error.unwrap_or_default()
            );
        }
        let b = resp.balance.unwrap_or(BalanceInner {
            confirmed: 0,
            unconfirmed: 0,
        });
        Ok((b.confirmed, b.unconfirmed))
    }

    /// Fetch raw UTXOs from the electrumx endpoint for an address.
    async fn fetch_utxos(&self, address: &str) -> Result<Vec<ElectrumxUtxo>> {
        let url = format!("{}/v1/electrumx/utxos/{}", self.base_url, address);
        let resp: UtxosResponse = self
            .client
            .get(&url)
            .send()
            .await
            .context("electrumx utxos request failed")?
            .json()
            .await
            .context("electrumx utxos parse failed")?;

        if !resp.success {
            bail!("electrumx utxos error: {}", resp.error.unwrap_or_default());
        }
        Ok(resp
            .utxos
            .unwrap_or_default()
            .into_iter()
            .map(|u| ElectrumxUtxo {
                txid: u.tx_hash,
                vout: u.tx_pos,
                value: u.value,
                token_data: u.token_data,
            })
            .collect())
    }

    /// Get BCH-only UTXOs (no token data) for an address.
    pub async fn get_utxos(&self, address: &str) -> Result<Vec<ElectrumxUtxo>> {
        let all = self.fetch_utxos(address).await?;
        Ok(all.into_iter().filter(|u| u.token_data.is_none()).collect())
    }

    /// Get ALL UTXOs including those with CashToken data.
    pub async fn get_all_utxos(&self, address: &str) -> Result<Vec<ElectrumxUtxo>> {
        self.fetch_utxos(address).await
    }

    /// Broadcast a raw transaction hex. Returns the txid on success.
    pub async fn broadcast(&self, tx_hex: &str) -> Result<String> {
        let url = format!("{}/v1/electrumx/tx/broadcast", self.base_url);
        let resp: BroadcastResponse = self
            .client
            .post(&url)
            .json(&serde_json::json!({ "txHex": tx_hex }))
            .send()
            .await
            .context("electrumx broadcast request failed")?
            .json()
            .await
            .context("electrumx broadcast parse failed")?;

        if !resp.success {
            bail!(
                "electrumx broadcast error: {}",
                resp.error.unwrap_or_default()
            );
        }
        match resp.txid {
            Some(serde_json::Value::String(s)) => Ok(s),
            Some(v) => Ok(v.to_string().trim_matches('"').to_string()),
            None => bail!("electrumx broadcast returned no txid"),
        }
    }

    /// Probe if the server is reachable (health check endpoint).
    pub async fn probe(&self) -> bool {
        let url = format!("{}/v1/electrumx/", self.base_url);
        let probe_client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(5))
            .build()
            .unwrap_or_default();
        probe_client.get(&url).send().await.is_ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_balance_response() {
        let json = r#"{"success":true,"balance":{"confirmed":50000,"unconfirmed":1000}}"#;
        let resp: BalanceResponse = serde_json::from_str(json).unwrap();
        assert!(resp.success);
        let b = resp.balance.unwrap();
        assert_eq!(b.confirmed, 50000);
        assert_eq!(b.unconfirmed, 1000);
    }

    #[test]
    fn parse_utxos_response_plain() {
        let json = r#"{"success":true,"utxos":[{"tx_hash":"abc123","tx_pos":0,"value":1000,"height":800000}]}"#;
        let resp: UtxosResponse = serde_json::from_str(json).unwrap();
        assert!(resp.success);
        let utxos = resp.utxos.unwrap();
        assert_eq!(utxos.len(), 1);
        assert_eq!(utxos[0].tx_hash, "abc123");
        assert_eq!(utxos[0].value, 1000);
        assert!(utxos[0].token_data.is_none());
    }

    #[test]
    fn parse_utxos_response_with_fungible_token() {
        let json = r#"{"success":true,"utxos":[{
            "tx_hash":"def456","tx_pos":1,"value":800,"height":945296,
            "token_data":{"category":"ea38c6a264","amount":"30"}
        }]}"#;
        let resp: UtxosResponse = serde_json::from_str(json).unwrap();
        let utxos = resp.utxos.unwrap();
        assert_eq!(utxos.len(), 1);
        let td = utxos[0].token_data.as_ref().unwrap();
        assert_eq!(td.category, "ea38c6a264");
        assert_eq!(td.amount, "30");
        assert!(td.nft.is_none());
    }

    #[test]
    fn parse_utxos_response_with_nft() {
        let json = r#"{"success":true,"utxos":[{
            "tx_hash":"nft789","tx_pos":0,"value":1000,"height":945296,
            "token_data":{"category":"909427e2f7","amount":"0","nft":{"capability":"none","commitment":"98"}}
        }]}"#;
        let resp: UtxosResponse = serde_json::from_str(json).unwrap();
        let utxos = resp.utxos.unwrap();
        let td = utxos[0].token_data.as_ref().unwrap();
        assert_eq!(td.category, "909427e2f7");
        let nft = td.nft.as_ref().unwrap();
        assert_eq!(nft.capability, "none");
        assert_eq!(nft.commitment, "98");
    }

    #[test]
    fn parse_broadcast_response() {
        let json = r#"{"success":true,"txid":"deadbeef1234"}"#;
        let resp: BroadcastResponse = serde_json::from_str(json).unwrap();
        assert!(resp.success);
        assert_eq!(
            resp.txid.unwrap(),
            serde_json::Value::String("deadbeef1234".into())
        );
    }

    #[test]
    fn parse_error_response() {
        let json = r#"{"success":false,"error":"invalid address"}"#;
        let resp: BalanceResponse = serde_json::from_str(json).unwrap();
        assert!(!resp.success);
        assert_eq!(resp.error.unwrap(), "invalid address");
    }
}
