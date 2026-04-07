/// BchWallet: business logic combining HdWallet + Watchtower API.
///
/// Security model:
/// - Read operations (balance, UTXOs, history): via Watchtower REST API using wallet_hash
/// - Transaction signing: done locally using HD wallet keys
/// - Broadcast: via Watchtower REST API
/// - Key material NEVER leaves the machine
use anyhow::{Context, Result};

use crate::constants::{DEFAULT_FEE_RATE, FEE_RESERVE_SATS, SATS_PER_BCH};
use crate::transaction;
use crate::types::{
    BalanceResponse, BroadcastResult, CashTokenUtxo, FungibleToken, HistoryResponse,
    NftUtxo, SendResult,
};
use crate::watchtower::client::{
    HistoryParams, SubscribeRequest, WatchtowerClient,
};

use super::keys::{AddressSet, HdWallet};

/// Extract "short by N sats" from a transaction build error message.
fn extract_lacking_sats(err_msg: &str) -> Option<u64> {
    err_msg
        .split("short by ")
        .nth(1)
        .and_then(|s| s.split(' ').next())
        .and_then(|s| s.parse::<u64>().ok())
}

/// Options for fetching transaction history.
pub struct HistoryOptions {
    pub page: u32,
    pub record_type: String,
    pub token_id: String,
}

/// Parameters for sending an NFT.
pub struct NftSendParams {
    pub category: String,
    pub commitment: String,
    pub capability: String,
    pub txid: String,
    pub vout: u32,
    pub address: String,
    pub change_address: Option<String>,
}

/// Core BCH wallet: Watchtower for all API operations.
pub struct BchWallet {
    wallet_hash: String,
    hd_wallet: HdWallet,
    watchtower: WatchtowerClient,
    project_id: String,
}

impl BchWallet {
    /// Create a new BchWallet backed by Watchtower.
    pub fn new(project_id: &str, mnemonic: &str, path: &str, chipnet: bool) -> Result<Self> {
        let hd_wallet = HdWallet::new(mnemonic, path, chipnet)?;
        let wallet_hash = hd_wallet.wallet_hash().to_string();
        let watchtower = WatchtowerClient::new(chipnet);

        Ok(Self {
            wallet_hash,
            hd_wallet,
            watchtower,
            project_id: project_id.to_string(),
        })
    }

    /// Derive receiving + change addresses at index.
    pub fn get_address_set_at(&self, index: u32) -> Result<AddressSet> {
        self.hd_wallet.get_address_set_at(index)
    }

    /// Derive token-aware addresses at index.
    pub fn get_token_address_set_at(&self, index: u32) -> Result<AddressSet> {
        self.hd_wallet.get_token_address_set_at(index)
    }

    // ── Watchtower subscription ────────────────────────────────────────

    /// Register addresses and trigger UTXO scan with Watchtower.
    pub async fn ensure_synced(&self, address_count: u32) -> Result<()> {
        let _ = self.scan_addresses(0, address_count).await;
        let _ = self.scan_utxos(true).await;
        Ok(())
    }

    /// Subscribe a range of addresses to Watchtower for monitoring.
    pub async fn scan_addresses(&self, start_index: u32, count: u32) -> Result<()> {
        let end_index = start_index + count;
        for i in start_index..end_index {
            let addrs = self.hd_wallet.get_address_set_at(i)?;
            let _ = self
                .watchtower
                .subscribe(&SubscribeRequest {
                    address: addrs.receiving,
                    project_id: self.project_id.clone(),
                    wallet_hash: Some(self.wallet_hash.clone()),
                    wallet_index: Some(i),
                })
                .await;
            let _ = self
                .watchtower
                .subscribe(&SubscribeRequest {
                    address: addrs.change,
                    project_id: self.project_id.clone(),
                    wallet_hash: Some(self.wallet_hash.clone()),
                    wallet_index: Some(i),
                })
                .await;

            let token_addrs = self.hd_wallet.get_token_address_set_at(i)?;
            let _ = self
                .watchtower
                .subscribe(&SubscribeRequest {
                    address: token_addrs.receiving,
                    project_id: self.project_id.clone(),
                    wallet_hash: Some(self.wallet_hash.clone()),
                    wallet_index: Some(i),
                })
                .await;
            let _ = self
                .watchtower
                .subscribe(&SubscribeRequest {
                    address: token_addrs.change,
                    project_id: self.project_id.clone(),
                    wallet_hash: Some(self.wallet_hash.clone()),
                    wallet_index: Some(i),
                })
                .await;
        }
        Ok(())
    }

    /// Trigger a UTXO scan.
    pub async fn scan_utxos(&self, background: bool) -> Result<()> {
        self.watchtower
            .scan_utxos(&self.wallet_hash, background)
            .await
    }

    // ── Read operations (via Watchtower -- single wallet_hash query) ───

    /// Get wallet BCH balance.
    pub async fn get_balance(&self) -> Result<BalanceResponse> {
        let resp = self.watchtower.get_balance(&self.wallet_hash).await?;
        Ok(BalanceResponse {
            valid: resp.valid,
            wallet: resp.wallet,
            spendable: resp.spendable,
            balance: resp.balance,
        })
    }

    /// Get token balance.
    pub async fn get_token_balance(&self, token_id: &str) -> Result<BalanceResponse> {
        let resp = self
            .watchtower
            .get_token_balance(&self.wallet_hash, token_id)
            .await?;
        Ok(BalanceResponse {
            valid: resp.valid,
            wallet: resp.wallet,
            spendable: resp.spendable,
            balance: resp.balance,
        })
    }

    /// Get transaction history.
    pub async fn get_history(&self, opts: HistoryOptions) -> Result<HistoryResponse> {
        let params = HistoryParams {
            wallet_hash: self.wallet_hash.clone(),
            token_id: opts.token_id,
            page: opts.page,
            record_type: opts.record_type,
        };
        let resp = self.watchtower.get_history(&params).await?;
        Ok(HistoryResponse {
            history: resp.history.into_iter().map(|e| crate::types::HistoryEntry {
                record_type: e.record_type,
                txid: e.txid,
                amount: e.amount,
                tx_fee: e.tx_fee,
                senders: e.senders,
                recipients: e.recipients,
                date_created: e.date_created,
                tx_timestamp: e.tx_timestamp,
                usd_price: e.usd_price,
                market_prices: e.market_prices,
                attributes: e.attributes,
                token_changes: Vec::new(),
            }).collect(),
            page: resp.page,
            num_pages: resp.num_pages,
            has_next: resp.has_next,
        })
    }

    /// Get BCH (non-token) UTXOs.
    pub async fn get_bch_utxos(&self) -> Result<Vec<transaction::Utxo>> {
        self.watchtower.get_bch_utxos(&self.wallet_hash).await
    }

    /// Get CashToken UTXOs for a specific category.
    pub async fn get_cashtoken_utxos(&self, category: &str) -> Result<Vec<CashTokenUtxo>> {
        let wt_utxos = self
            .watchtower
            .get_cashtoken_utxos(&self.wallet_hash, category)
            .await?;
        Ok(wt_utxos
            .into_iter()
            .map(|u| CashTokenUtxo {
                txid: u.txid,
                vout: u.vout,
                value: u.value,
                address_path: u.address_path,
                token_amount: u.token_amount,
                commitment: u.commitment,
                capability: u.capability,
            })
            .collect())
    }

    /// List all fungible CashToken balances.
    pub async fn get_fungible_tokens(&self) -> Result<Vec<FungibleToken>> {
        let wt_tokens = self
            .watchtower
            .get_fungible_tokens(&self.wallet_hash)
            .await?;
        Ok(wt_tokens
            .into_iter()
            .map(|t| FungibleToken {
                id: t.id,
                category: t.category,
                name: t.name,
                symbol: t.symbol,
                decimals: t.decimals,
                image_url: t.image_url,
                balance: t.balance,
            })
            .collect())
    }

    /// Get info for a specific CashToken.
    pub async fn get_token_info(&self, category: &str) -> Result<Option<FungibleToken>> {
        let wt = self.watchtower.get_token_info(category).await?;
        Ok(wt.map(|t| FungibleToken {
            id: t.id,
            category: t.category,
            name: t.name,
            symbol: t.symbol,
            decimals: t.decimals,
            image_url: t.image_url,
            balance: t.balance,
        }))
    }

    /// Get NFT UTXOs, optionally filtered by category.
    pub async fn get_nft_utxos(&self, category: Option<&str>) -> Result<Vec<NftUtxo>> {
        let wt_nfts = self
            .watchtower
            .get_nft_utxos(&self.wallet_hash, category)
            .await?;
        Ok(wt_nfts
            .into_iter()
            .map(|n| NftUtxo {
                txid: n.txid,
                vout: n.vout,
                category: n.category,
                commitment: n.commitment,
                capability: n.capability,
                amount: n.amount,
                value: n.value,
            })
            .collect())
    }

    // ── Write operations (local signing + Watchtower broadcast) ──────

    /// Broadcast a raw transaction hex via Watchtower API.
    pub async fn broadcast(&self, tx_hex: &str) -> Result<BroadcastResult> {
        let wt_result = self.watchtower.broadcast(tx_hex).await?;
        Ok(BroadcastResult {
            txid: wt_result.txid,
            success: wt_result.success,
            error: wt_result.error,
        })
    }

    /// Send BCH to a recipient using native transaction building.
    pub async fn send_bch(
        &self,
        amount: f64,
        address: &str,
        change_address: Option<&str>,
    ) -> Result<SendResult> {
        let change_addr = match change_address {
            Some(addr) => addr.to_string(),
            None => self.hd_wallet.get_address_set_at(0)?.change,
        };

        let amount_sats = (amount * SATS_PER_BCH).round() as u64;
        if amount_sats == 0 {
            return Ok(SendResult {
                success: false,
                txid: None,
                error: Some("amount must be greater than zero".to_string()),
                lacking_sats: None,
            });
        }

        let utxos = self.get_bch_utxos().await?;
        if utxos.is_empty() {
            return Ok(SendResult {
                success: false,
                txid: None,
                error: Some("no spendable UTXOs found".to_string()),
                lacking_sats: Some(amount_sats),
            });
        }

        let outputs = vec![transaction::TxOutput {
            address: address.to_string(),
            value: amount_sats,
        }];

        let built = match transaction::build_p2pkh_transaction(
            &utxos, &outputs, &change_addr, &self.hd_wallet, DEFAULT_FEE_RATE,
        ) {
            Ok(tx) => tx,
            Err(e) => {
                let err_msg = e.to_string();
                return Ok(SendResult {
                    success: false, txid: None,
                    error: Some(err_msg.clone()),
                    lacking_sats: extract_lacking_sats(&err_msg),
                });
            }
        };

        let br = self.broadcast(&built.hex).await?;
        Ok(if br.success {
            SendResult { success: true, txid: br.txid.or(Some(built.txid)), error: None, lacking_sats: None }
        } else {
            SendResult { success: false, txid: None, error: br.error, lacking_sats: None }
        })
    }

    /// Send fungible CashTokens.
    pub async fn send_token(
        &self, category: &str, amount: u64, address: &str, change_address: Option<&str>,
    ) -> Result<SendResult> {
        let token_change_addr = match change_address {
            Some(addr) => addr.to_string(),
            None => self.hd_wallet.get_token_address_set_at(0)?.change,
        };
        let bch_change_addr = self.hd_wallet.get_address_set_at(0)?.change;

        let token_utxos = self.get_cashtoken_utxos(category).await
            .context("failed to fetch token UTXOs")?;
        let fungible_utxos: Vec<_> = token_utxos.iter().filter(|u| u.capability.is_none()).collect();

        if fungible_utxos.is_empty() {
            return Ok(SendResult {
                success: false, txid: None,
                error: Some("no fungible token UTXOs found for this category".to_string()),
                lacking_sats: None,
            });
        }

        let mut selected: Vec<&CashTokenUtxo> = Vec::new();
        let mut token_total: u64 = 0;
        for utxo in &fungible_utxos {
            selected.push(utxo);
            token_total += utxo.token_amount;
            if token_total >= amount { break; }
        }

        if token_total < amount {
            return Ok(SendResult {
                success: false, txid: None,
                error: Some(format!("insufficient token balance: have {}, need {}", token_total, amount)),
                lacking_sats: None,
            });
        }

        let category_bytes = transaction::decode_txid_to_bytes(category).context("invalid category hex")?;

        let mut outputs = vec![transaction::TokenTxOutput {
            address: address.to_string(),
            value: transaction::token_dust(),
            token: Some(transaction::TokenPrefix { category: category_bytes, nft: None, amount }),
        }];

        let token_change = token_total - amount;
        if token_change > 0 {
            outputs.push(transaction::TokenTxOutput {
                address: token_change_addr,
                value: transaction::token_dust(),
                token: Some(transaction::TokenPrefix { category: category_bytes, nft: None, amount: token_change }),
            });
        }

        let mut all_inputs: Vec<transaction::Utxo> = selected.iter().map(|u| transaction::Utxo {
            txid: u.txid.clone(), vout: u.vout, value: u.value,
            address_path: u.address_path.clone(),
            token: Some(transaction::TokenPrefix { category: category_bytes, nft: None, amount: u.token_amount }),
        }).collect();

        let bch_utxos = self.get_bch_utxos().await.context("failed to fetch BCH UTXOs")?;
        let mut sorted_bch = bch_utxos.clone();
        sorted_bch.sort_by(|a, b| b.value.cmp(&a.value));
        let output_bch: u64 = outputs.iter().map(|o| o.value).sum();
        for utxo in &sorted_bch {
            let input_bch: u64 = all_inputs.iter().map(|i| i.value).sum();
            if input_bch >= output_bch + FEE_RESERVE_SATS { break; }
            all_inputs.push(utxo.clone());
        }

        let built = match transaction::build_token_transaction(
            &all_inputs, &outputs, &bch_change_addr, &self.hd_wallet, DEFAULT_FEE_RATE,
        ) {
            Ok(tx) => tx,
            Err(e) => {
                let err_msg = e.to_string();
                return Ok(SendResult {
                    success: false, txid: None, error: Some(err_msg.clone()),
                    lacking_sats: extract_lacking_sats(&err_msg),
                });
            }
        };

        let br = self.broadcast(&built.hex).await?;
        Ok(if br.success {
            SendResult { success: true, txid: br.txid.or(Some(built.txid)), error: None, lacking_sats: None }
        } else {
            SendResult { success: false, txid: None, error: br.error, lacking_sats: None }
        })
    }

    /// Send an NFT (non-fungible CashToken).
    pub async fn send_nft(&self, params: NftSendParams) -> Result<SendResult> {
        let bch_change_addr = match &params.change_address {
            Some(addr) => addr.clone(),
            None => self.hd_wallet.get_address_set_at(0)?.change,
        };

        let token_utxos = self.get_cashtoken_utxos(&params.category).await
            .context("failed to fetch token UTXOs")?;
        let nft_utxo = token_utxos.iter()
            .find(|u| u.txid == params.txid && u.vout == params.vout)
            .ok_or_else(|| anyhow::anyhow!("NFT UTXO {}:{} not found in wallet", params.txid, params.vout))?;

        let category_bytes = transaction::decode_txid_to_bytes(&params.category).context("invalid category hex")?;
        let commitment_bytes = if params.commitment.is_empty() {
            Vec::new()
        } else {
            hex::decode(&params.commitment).context("invalid commitment hex")?
        };
        let capability = transaction::NftCapability::parse(&params.capability)?;

        let outputs = vec![transaction::TokenTxOutput {
            address: params.address.clone(),
            value: transaction::token_dust(),
            token: Some(transaction::TokenPrefix {
                category: category_bytes,
                nft: Some(transaction::NftData { capability, commitment: commitment_bytes }),
                amount: 0,
            }),
        }];

        let nft_commitment_bytes = if nft_utxo.commitment.is_empty() {
            Vec::new()
        } else {
            hex::decode(&nft_utxo.commitment).unwrap_or_default()
        };
        let nft_capability = transaction::NftCapability::parse(
            nft_utxo.capability.as_deref().unwrap_or("none")
        ).unwrap_or(transaction::NftCapability::None);

        let mut all_inputs = vec![transaction::Utxo {
            txid: nft_utxo.txid.clone(), vout: nft_utxo.vout, value: nft_utxo.value,
            address_path: nft_utxo.address_path.clone(),
            token: Some(transaction::TokenPrefix {
                category: category_bytes,
                nft: Some(transaction::NftData { capability: nft_capability, commitment: nft_commitment_bytes }),
                amount: nft_utxo.token_amount,
            }),
        }];

        let bch_utxos = self.get_bch_utxos().await.context("failed to fetch BCH UTXOs")?;
        let mut sorted_bch = bch_utxos.clone();
        sorted_bch.sort_by(|a, b| b.value.cmp(&a.value));
        let output_bch: u64 = outputs.iter().map(|o| o.value).sum();
        for utxo in &sorted_bch {
            let input_bch: u64 = all_inputs.iter().map(|i| i.value).sum();
            if input_bch >= output_bch + FEE_RESERVE_SATS { break; }
            all_inputs.push(utxo.clone());
        }

        let built = match transaction::build_token_transaction(
            &all_inputs, &outputs, &bch_change_addr, &self.hd_wallet, DEFAULT_FEE_RATE,
        ) {
            Ok(tx) => tx,
            Err(e) => {
                let err_msg = e.to_string();
                return Ok(SendResult {
                    success: false, txid: None, error: Some(err_msg.clone()),
                    lacking_sats: extract_lacking_sats(&err_msg),
                });
            }
        };

        let br = self.broadcast(&built.hex).await?;
        Ok(if br.success {
            SendResult { success: true, txid: br.txid.or(Some(built.txid)), error: None, lacking_sats: None }
        } else {
            SendResult { success: false, txid: None, error: br.error, lacking_sats: None }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::network::BCH_DERIVATION_PATH;

    const TEST_MNEMONIC: &str =
        "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";
    const TEST_PROJECT_ID: &str = "5348e8fd-c001-47c7-b97c-807f545cf44e";

    #[test]
    fn test_bch_wallet_new() {
        let wallet = BchWallet::new(TEST_PROJECT_ID, TEST_MNEMONIC, BCH_DERIVATION_PATH, false);
        assert!(wallet.is_ok());
    }

    #[test]
    fn test_bch_wallet_address_derivation() {
        let wallet = BchWallet::new(TEST_PROJECT_ID, TEST_MNEMONIC, BCH_DERIVATION_PATH, false).unwrap();
        let set = wallet.get_address_set_at(0).unwrap();
        assert!(set.receiving.starts_with("bitcoincash:q"));
        assert!(set.change.starts_with("bitcoincash:q"));
    }

    #[test]
    fn test_bch_wallet_token_address_derivation() {
        let wallet = BchWallet::new(TEST_PROJECT_ID, TEST_MNEMONIC, BCH_DERIVATION_PATH, false).unwrap();
        let set = wallet.get_token_address_set_at(0).unwrap();
        assert!(set.receiving.starts_with("bitcoincash:z"));
        assert!(set.change.starts_with("bitcoincash:z"));
    }

    #[test]
    fn test_bch_wallet_invalid_mnemonic() {
        let result = BchWallet::new(TEST_PROJECT_ID, "bad words here", BCH_DERIVATION_PATH, false);
        assert!(result.is_err());
    }

    #[test]
    fn test_chipnet_wallet() {
        let wallet = BchWallet::new(TEST_PROJECT_ID, TEST_MNEMONIC, BCH_DERIVATION_PATH, true).unwrap();
        let set = wallet.get_address_set_at(0).unwrap();
        assert!(set.receiving.starts_with("bchtest:q"));
    }
}
