/// Shared response types used across the CLI.
///
/// These were previously defined in the watchtower client module.
/// They remain as internal types that we convert mainnet API responses into,
/// keeping the CLI layer stable.
use serde::{Deserialize, Serialize};

/// BCH balance response.
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

/// Transaction history (paginated).
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

/// A single history entry.
/// A token amount change within a history entry.
#[derive(Debug, Clone, Default)]
pub struct TokenChange {
    pub category: String,
    pub amount: f64,      // fungible change (+ received, - sent)
    pub nft_amount: f64,  // NFT count change (+ received, - sent)
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
    /// Token amount changes (not serialized — populated from mainnet API)
    #[serde(skip)]
    pub token_changes: Vec<TokenChange>,
}

/// Fungible CashToken metadata + balance.
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

/// NFT UTXO info.
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

/// A CashToken UTXO with full token data and address path for signing.
#[derive(Debug, Clone)]
pub struct CashTokenUtxo {
    pub txid: String,
    pub vout: u32,
    pub value: u64,
    pub address_path: String,
    pub token_amount: u64,
    pub commitment: String,
    pub capability: Option<String>,
}

/// Result from sending BCH or tokens.
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

/// Result from broadcasting a raw transaction.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BroadcastResult {
    #[serde(default)]
    pub txid: Option<String>,
    #[serde(default)]
    pub success: bool,
    #[serde(default)]
    pub error: Option<String>,
}
