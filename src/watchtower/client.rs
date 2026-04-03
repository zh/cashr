/// Typed async HTTP client for all Watchtower REST API endpoints.
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::network;
use crate::transaction::Utxo;

/// Watchtower REST API client.
pub struct WatchtowerClient {
    client: reqwest::Client,
    base_url: String,
}

// ── Request/Response types ───────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubscribeRequest {
    pub addresses: AddressSetPayload,
    pub project_id: String,
    pub wallet_hash: String,
    pub address_index: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AddressSetPayload {
    pub receiving: String,
    pub change: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubscribeResponse {
    pub success: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BalanceResponse {
    #[serde(default)]
    pub valid: bool,
    #[serde(default)]
    pub wallet: String,
    #[serde(default)]
    pub spendable: f64,
    #[serde(default)]
    pub balance: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoryParams {
    pub wallet_hash: String,
    #[serde(default)]
    pub token_id: String,
    #[serde(default = "default_page")]
    pub page: u32,
    #[serde(default = "default_record_type")]
    pub record_type: String,
}

fn default_page() -> u32 {
    1
}

fn default_record_type() -> String {
    "all".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoryResponse {
    #[serde(default)]
    pub history: Vec<HistoryEntry>,
    #[serde(default)]
    pub page: String,
    #[serde(default)]
    pub num_pages: u32,
    #[serde(default)]
    pub has_next: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoryEntry {
    #[serde(default)]
    pub record_type: String,
    #[serde(default)]
    pub txid: String,
    #[serde(default)]
    pub amount: f64,
    #[serde(default)]
    pub tx_fee: f64,
    #[serde(default)]
    pub senders: serde_json::Value,
    #[serde(default)]
    pub recipients: serde_json::Value,
    #[serde(default)]
    pub date_created: String,
    #[serde(default)]
    pub tx_timestamp: String,
    #[serde(default)]
    pub usd_price: f64,
    #[serde(default)]
    pub market_prices: serde_json::Value,
    #[serde(default)]
    pub attributes: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FungibleToken {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub category: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub symbol: String,
    #[serde(default)]
    pub decimals: u32,
    #[serde(default, rename = "image_url")]
    pub image_url: String,
    #[serde(default)]
    pub balance: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NftUtxo {
    #[serde(default)]
    pub txid: String,
    #[serde(default)]
    pub vout: u32,
    #[serde(default, rename = "tokenid")]
    pub category: String,
    #[serde(default)]
    pub commitment: String,
    #[serde(default)]
    pub capability: String,
    #[serde(default)]
    pub amount: f64,
    #[serde(default)]
    pub value: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SendRequest {
    pub sender: SenderInfo,
    pub recipients: Vec<Recipient>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub token: Option<TokenParam>,
    #[serde(rename = "changeAddress", skip_serializing_if = "Option::is_none")]
    pub change_address: Option<String>,
    pub wallet: WalletAuth,
    pub broadcast: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SenderInfo {
    #[serde(rename = "walletHash")]
    pub wallet_hash: String,
    pub mnemonic: String,
    #[serde(rename = "derivationPath")]
    pub derivation_path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WalletAuth {
    pub mnemonic: String,
    #[serde(rename = "derivationPath")]
    pub derivation_path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Recipient {
    pub address: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub amount: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none", rename = "tokenAmount")]
    pub token_amount: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenParam {
    #[serde(rename = "tokenId")]
    pub token_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub commitment: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub capability: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub txid: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vout: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SendResult {
    pub success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub txid: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", rename = "lackingSats")]
    pub lacking_sats: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AddressScanRequest {
    pub address_sets: Vec<AddressScanEntry>,
    pub wallet_hash: String,
    pub project_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AddressScanEntry {
    pub address_index: u32,
    pub addresses: AddressSetPayload,
}

// ── Paginated response wrapper ───────────────────────────────────────

#[derive(Debug, Clone, Deserialize)]
struct PaginatedResponse<T> {
    results: Vec<T>,
    next: Option<String>,
}

// ── Internal raw types for Watchtower API parsing ────────────────────

#[derive(Debug, Deserialize)]
struct RawFungibleToken {
    #[serde(default)]
    id: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    symbol: String,
    #[serde(default)]
    decimals: u32,
    #[serde(default)]
    image_url: String,
    #[serde(default)]
    balance: f64,
}

#[derive(Debug, Deserialize)]
struct RawUtxoResponse {
    #[serde(default)]
    utxos: Vec<RawUtxo>,
}

#[derive(Debug, Deserialize)]
struct RawUtxo {
    #[serde(default)]
    txid: String,
    #[serde(default)]
    vout: u32,
    #[serde(default)]
    tokenid: Option<String>,
    #[serde(default)]
    commitment: Option<String>,
    #[serde(default)]
    capability: Option<String>,
    #[serde(default)]
    is_cashtoken: bool,
    #[serde(default)]
    amount: f64,
    #[serde(default)]
    value: f64,
    #[serde(default)]
    address_path: Option<String>,
    #[serde(default)]
    wallet_index: Option<String>,
}

/// Result from broadcasting a transaction.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BroadcastResult {
    #[serde(default)]
    pub txid: Option<String>,
    #[serde(default)]
    pub success: bool,
    #[serde(default)]
    pub error: Option<String>,
}

/// Internal broadcast response (Watchtower returns just txid on success).
#[derive(Debug, Deserialize)]
struct RawBroadcastResponse {
    #[serde(default)]
    txid: Option<String>,
}

#[derive(Debug, Deserialize)]
struct LastAddressIndexResponse {
    address: Option<LastAddressIndexAddr>,
}

#[derive(Debug, Deserialize)]
struct LastAddressIndexAddr {
    address_index: Option<u32>,
}

// ── Client implementation ────────────────────────────────────────────

impl WatchtowerClient {
    /// Create a new Watchtower client for the given network.
    pub fn new(chipnet: bool) -> Self {
        let base_url = network::watchtower_api_url(chipnet).to_string();
        Self {
            client: reqwest::Client::new(),
            base_url,
        }
    }

    /// Create a client with a custom base URL (for testing).
    #[cfg(test)]
    pub fn with_base_url(base_url: &str) -> Self {
        Self {
            client: reqwest::Client::new(),
            base_url: base_url.to_string(),
        }
    }

    fn url(&self, path: &str) -> String {
        format!("{}/{}", self.base_url, path)
    }

    /// Subscribe an address pair for Watchtower monitoring.
    pub async fn subscribe(&self, data: &SubscribeRequest) -> Result<SubscribeResponse> {
        let resp = self
            .client
            .post(self.url("subscribe/"))
            .json(data)
            .send()
            .await
            .context("subscribe request failed")?;
        let result = resp
            .json::<SubscribeResponse>()
            .await
            .context("failed to parse subscribe response")?;
        Ok(result)
    }

    /// Get wallet balance.
    pub async fn get_balance(&self, wallet_hash: &str) -> Result<BalanceResponse> {
        let resp = self
            .client
            .get(self.url(&format!("balance/wallet/{}/", wallet_hash)))
            .send()
            .await
            .context("balance request failed")?;
        let result = resp
            .json::<BalanceResponse>()
            .await
            .context("failed to parse balance response")?;
        Ok(result)
    }

    /// Get token balance.
    pub async fn get_token_balance(
        &self,
        wallet_hash: &str,
        token_id: &str,
    ) -> Result<BalanceResponse> {
        let resp = self
            .client
            .get(self.url(&format!(
                "balance/wallet/{}/{}/",
                wallet_hash, token_id
            )))
            .send()
            .await
            .context("token balance request failed")?;
        let result = resp
            .json::<BalanceResponse>()
            .await
            .context("failed to parse token balance response")?;
        Ok(result)
    }

    /// Get transaction history.
    pub async fn get_history(&self, params: &HistoryParams) -> Result<HistoryResponse> {
        let resp = self
            .client
            .get(self.url(&format!(
                "history/wallet/{}/",
                params.wallet_hash
            )))
            .query(&[
                ("page", params.page.to_string()),
                ("record_type", params.record_type.clone()),
                ("token_id", params.token_id.clone()),
            ])
            .send()
            .await
            .context("history request failed")?;
        let result = resp
            .json::<HistoryResponse>()
            .await
            .context("failed to parse history response")?;
        Ok(result)
    }

    /// Get last used address index.
    pub async fn get_last_address_index(&self, wallet_hash: &str) -> Result<Option<u32>> {
        let resp = self
            .client
            .get(self.url(&format!(
                "last-address-index/wallet/{}/",
                wallet_hash
            )))
            .send()
            .await
            .context("last address index request failed")?;
        let data = resp
            .json::<LastAddressIndexResponse>()
            .await
            .context("failed to parse last address index response")?;
        Ok(data.address.and_then(|a| a.address_index))
    }

    /// Bulk-subscribe a range of address sets.
    pub async fn scan_addresses(&self, data: &AddressScanRequest) -> Result<()> {
        self.client
            .post(self.url("wallet/address-scan/"))
            .json(data)
            .send()
            .await
            .context("address scan request failed")?;
        Ok(())
    }

    /// Trigger a UTXO scan.
    pub async fn scan_utxos(&self, wallet_hash: &str, background: bool) -> Result<()> {
        let mut url = self.url(&format!("utxo/wallet/{}/scan/", wallet_hash));
        if background {
            url.push_str("?background=true");
        }
        self.client
            .get(&url)
            .send()
            .await
            .context("UTXO scan request failed")?;
        Ok(())
    }

    /// List fungible CashTokens (paginated -- follows all pages).
    pub async fn get_fungible_tokens(&self, wallet_hash: &str) -> Result<Vec<FungibleToken>> {
        let mut all_tokens = Vec::new();
        let mut url = self.url("cashtokens/fungible/");
        let mut first_page = true;

        loop {
            let resp = if first_page {
                first_page = false;
                self.client
                    .get(&url)
                    .query(&[
                        ("wallet_hash", wallet_hash),
                        ("has_balance", "true"),
                        ("limit", "100"),
                    ])
                    .send()
                    .await
                    .context("fungible tokens request failed")?
            } else {
                self.client
                    .get(&url)
                    .send()
                    .await
                    .context("fungible tokens pagination request failed")?
            };

            let page: PaginatedResponse<RawFungibleToken> = resp
                .json()
                .await
                .context("failed to parse fungible tokens response")?;

            for raw in page.results {
                let category = extract_category(&raw.id);
                all_tokens.push(FungibleToken {
                    id: raw.id.clone(),
                    category,
                    name: if raw.name.is_empty() {
                        "Unknown Token".to_string()
                    } else {
                        raw.name
                    },
                    symbol: raw.symbol,
                    decimals: raw.decimals,
                    image_url: raw.image_url,
                    balance: raw.balance,
                });
            }

            match page.next {
                Some(next_url) if !next_url.is_empty() => {
                    // The next field is an absolute URL; use it directly
                    url = next_url;
                }
                _ => break,
            }
        }

        Ok(all_tokens)
    }

    /// Get metadata for a single fungible CashToken.
    pub async fn get_token_info(&self, category: &str) -> Result<Option<FungibleToken>> {
        let resp = self
            .client
            .get(self.url(&format!("cashtokens/fungible/{}/", category)))
            .send()
            .await
            .context("token info request failed")?;

        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }

        let raw: RawFungibleToken = resp
            .json()
            .await
            .context("failed to parse token info response")?;

        let cat = extract_category(&raw.id);
        Ok(Some(FungibleToken {
            id: raw.id.clone(),
            category: if cat.is_empty() {
                category.to_string()
            } else {
                cat
            },
            name: if raw.name.is_empty() {
                "Unknown Token".to_string()
            } else {
                raw.name
            },
            symbol: raw.symbol,
            decimals: raw.decimals,
            image_url: raw.image_url,
            balance: raw.balance,
        }))
    }

    /// Get NFT UTXOs for a wallet, optionally filtered by category.
    pub async fn get_nft_utxos(
        &self,
        wallet_hash: &str,
        category: Option<&str>,
    ) -> Result<Vec<NftUtxo>> {
        let resp = self
            .client
            .get(self.url(&format!("utxo/wallet/{}/", wallet_hash)))
            .query(&[("is_cashtoken", "true")])
            .send()
            .await
            .context("NFT UTXOs request failed")?;

        let data: RawUtxoResponse = resp
            .json()
            .await
            .context("failed to parse UTXO response")?;

        let mut nfts = Vec::new();
        for utxo in data.utxos {
            // NFTs have a non-null capability
            if !utxo.is_cashtoken {
                continue;
            }
            let cap = match &utxo.capability {
                Some(c) => c.clone(),
                None => continue,
            };
            if let Some(filter_cat) = category {
                if utxo.tokenid.as_deref() != Some(filter_cat) {
                    continue;
                }
            }
            nfts.push(NftUtxo {
                txid: utxo.txid,
                vout: utxo.vout,
                category: utxo.tokenid.unwrap_or_default(),
                commitment: utxo.commitment.unwrap_or_default(),
                capability: cap,
                amount: utxo.amount,
                value: utxo.value,
            });
        }

        Ok(nfts)
    }

    /// Get BCH (non-token) UTXOs for a wallet, suitable for spending.
    pub async fn get_bch_utxos(&self, wallet_hash: &str) -> Result<Vec<Utxo>> {
        let resp = self
            .client
            .get(self.url(&format!("utxo/wallet/{}/", wallet_hash)))
            .send()
            .await
            .context("BCH UTXO request failed")?;

        let data: RawUtxoResponse = resp
            .json()
            .await
            .context("failed to parse UTXO response")?;

        let mut utxos = Vec::new();
        for raw in data.utxos {
            // Skip CashToken UTXOs -- we only want pure BCH UTXOs
            if raw.is_cashtoken {
                continue;
            }
            // Skip dust UTXOs
            let value = raw.value as u64;
            if value < 546 {
                continue;
            }
            // Determine address path from address_path or wallet_index
            let address_path = raw
                .address_path
                .or(raw.wallet_index)
                .unwrap_or_else(|| "0/0".to_string());

            utxos.push(Utxo {
                txid: raw.txid,
                vout: raw.vout,
                value,
                address_path,
            });
        }

        Ok(utxos)
    }

    /// Broadcast a raw transaction hex via Watchtower.
    pub async fn broadcast(&self, tx_hex: &str) -> Result<BroadcastResult> {
        let payload = serde_json::json!({ "transaction": tx_hex });

        let resp = self
            .client
            .post(self.url("broadcast/"))
            .json(&payload)
            .send()
            .await
            .context("broadcast request failed")?;

        let status = resp.status();
        if status.is_success() {
            let raw: RawBroadcastResponse = resp
                .json()
                .await
                .context("failed to parse broadcast response")?;
            Ok(BroadcastResult {
                txid: raw.txid,
                success: true,
                error: None,
            })
        } else {
            let body = resp.text().await.unwrap_or_default();
            Ok(BroadcastResult {
                txid: None,
                success: false,
                error: Some(format!("broadcast failed ({}): {}", status, body)),
            })
        }
    }

    /// Send BCH transaction via Watchtower.
    pub async fn send_bch(&self, data: &SendRequest) -> Result<SendResult> {
        let resp = self
            .client
            .post(self.url("bch/send/"))
            .json(data)
            .send()
            .await
            .context("send BCH request failed")?;
        let result = resp
            .json::<SendResult>()
            .await
            .context("failed to parse send response")?;
        Ok(result)
    }
}

/// Extract category from Watchtower token ID format "ct/<hex>".
fn extract_category(id: &str) -> String {
    if let Some(hex) = id.strip_prefix("ct/") {
        hex.to_string()
    } else {
        id.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_category() {
        assert_eq!(extract_category("ct/abc123"), "abc123");
        assert_eq!(extract_category("plain"), "plain");
        assert_eq!(extract_category(""), "");
    }

    #[test]
    fn test_send_request_serialization() {
        let req = SendRequest {
            sender: SenderInfo {
                wallet_hash: "hash".to_string(),
                mnemonic: "mnemonic".to_string(),
                derivation_path: "m/44'/145'/0'".to_string(),
            },
            recipients: vec![Recipient {
                address: "addr".to_string(),
                amount: Some(0.001),
                token_amount: None,
            }],
            token: None,
            change_address: Some("change".to_string()),
            wallet: WalletAuth {
                mnemonic: "mnemonic".to_string(),
                derivation_path: "m/44'/145'/0'".to_string(),
            },
            broadcast: true,
        };
        let json = serde_json::to_value(&req).unwrap();
        assert_eq!(json["sender"]["walletHash"], "hash");
        assert_eq!(json["sender"]["derivationPath"], "m/44'/145'/0'");
        assert_eq!(json["changeAddress"], "change");
        assert_eq!(json["wallet"]["derivationPath"], "m/44'/145'/0'");
    }

    #[test]
    fn test_balance_response_deserialization() {
        let json = r#"{"valid": true, "wallet": "hash", "spendable": 0.001, "balance": 0.002}"#;
        let resp: BalanceResponse = serde_json::from_str(json).unwrap();
        assert!(resp.valid);
        assert_eq!(resp.wallet, "hash");
        assert!((resp.spendable - 0.001).abs() < f64::EPSILON);
    }

    #[test]
    fn test_send_result_deserialization() {
        let json = r#"{"success": true, "txid": "abc123"}"#;
        let result: SendResult = serde_json::from_str(json).unwrap();
        assert!(result.success);
        assert_eq!(result.txid, Some("abc123".to_string()));
    }

    #[test]
    fn test_token_param_serialization() {
        let param = TokenParam {
            token_id: "cat123".to_string(),
            commitment: Some("ff".to_string()),
            capability: Some("minting".to_string()),
            txid: None,
            vout: None,
        };
        let json = serde_json::to_value(&param).unwrap();
        assert_eq!(json["tokenId"], "cat123");
        assert!(json.get("txid").is_none());
    }
}
