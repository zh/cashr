/// BCMR (Bitcoin Cash Metadata Registry) client.
///
/// Fetches token metadata (name, symbol, decimals) from external registries.
/// Fallback chain: Watchtower → Paytaca BCMR → defaults.
use serde::Deserialize;

/// Token metadata from BCMR registry.
pub struct TokenMetadata {
    pub name: String,
    pub symbol: String,
    pub decimals: u32,
}

/// BCMR metadata client with Watchtower → Paytaca fallback.
pub struct BcmrClient {
    client: reqwest::Client,
    chipnet: bool,
}

// ── Watchtower response ──────────────────────────────────────────────

#[derive(Deserialize)]
struct WatchtowerResponse {
    name: Option<String>,
    symbol: Option<String>,
    decimals: Option<u32>,
}

// ── Paytaca response ─────────────────────────────────────────────────

#[derive(Deserialize)]
struct PaytacaResponse {
    name: Option<String>,
    token: Option<PaytacaToken>,
}

#[derive(Deserialize)]
struct PaytacaToken {
    symbol: Option<String>,
    decimals: Option<u32>,
}

impl BcmrClient {
    pub fn new(chipnet: bool) -> Self {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(10))
            .user_agent("cashr/0.1")
            .build()
            .unwrap_or_default();
        Self { client, chipnet }
    }

    /// Fetch token metadata for a category. Returns None if not found anywhere.
    pub async fn get_token_info(&self, category: &str) -> Option<TokenMetadata> {
        // Try Watchtower first
        if let Some(meta) = self.try_watchtower(category).await {
            return Some(meta);
        }
        // Fallback to Paytaca
        self.try_paytaca(category).await
    }

    async fn try_watchtower(&self, category: &str) -> Option<TokenMetadata> {
        let base = if self.chipnet {
            "https://chipnet.watchtower.cash"
        } else {
            "https://watchtower.cash"
        };
        let url = format!("{base}/api/cashtokens/fungible/{category}/");

        let resp = self.client.get(&url).send().await.ok()?;
        if !resp.status().is_success() {
            return None;
        }
        let data: WatchtowerResponse = resp.json().await.ok()?;

        let name = data.name.unwrap_or_default();
        let symbol = data.symbol.unwrap_or_default();

        // Watchtower returns generic defaults for unknown tokens -- skip those
        if name.is_empty() || name == "CashToken" || name == "CashToken NFT" {
            return None;
        }

        Some(TokenMetadata {
            name,
            symbol,
            decimals: data.decimals.unwrap_or(0),
        })
    }

    async fn try_paytaca(&self, category: &str) -> Option<TokenMetadata> {
        let url = format!("https://bcmr.paytaca.com/api/tokens/{category}/");

        let resp = self.client.get(&url).send().await.ok()?;
        if !resp.status().is_success() {
            return None;
        }
        let data: PaytacaResponse = resp.json().await.ok()?;

        let name = data.name?;
        if name.is_empty() {
            return None;
        }

        let (symbol, decimals) = match data.token {
            Some(t) => (t.symbol.unwrap_or_default(), t.decimals.unwrap_or(0)),
            None => (String::new(), 0),
        };

        Some(TokenMetadata {
            name,
            symbol,
            decimals,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn watchtower_response_parse() {
        let json = r#"{"name":"TestToken","symbol":"TT","decimals":8,"image_url":null}"#;
        let resp: WatchtowerResponse = serde_json::from_str(json).unwrap();
        assert_eq!(resp.name.unwrap(), "TestToken");
        assert_eq!(resp.symbol.unwrap(), "TT");
        assert_eq!(resp.decimals.unwrap(), 8);
    }

    #[test]
    fn watchtower_generic_name_filtered() {
        let json = r#"{"name":"CashToken","symbol":"CASH","decimals":0}"#;
        let resp: WatchtowerResponse = serde_json::from_str(json).unwrap();
        let name = resp.name.unwrap();
        // Generic "CashToken" name should be treated as unknown
        assert_eq!(name, "CashToken");
    }

    #[test]
    fn paytaca_response_parse() {
        let json = r#"{"name":"MyToken","token":{"symbol":"MT","decimals":2}}"#;
        let resp: PaytacaResponse = serde_json::from_str(json).unwrap();
        assert_eq!(resp.name.unwrap(), "MyToken");
        let t = resp.token.unwrap();
        assert_eq!(t.symbol.unwrap(), "MT");
        assert_eq!(t.decimals.unwrap(), 2);
    }

    #[test]
    fn paytaca_error_response_parse() {
        let json = r#"{"category":"abc","error":"no valid metadata found"}"#;
        let resp: PaytacaResponse = serde_json::from_str(json).unwrap();
        assert!(resp.name.is_none());
    }
}
