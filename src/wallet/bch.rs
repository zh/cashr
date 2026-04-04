/// BchWallet: business logic combining HdWallet + WatchtowerClient.
///
/// Provides all BCH wallet operations: balance, send, tokens, history.
use anyhow::{Context, Result};

use crate::transaction;
use crate::watchtower::client::{
    BalanceResponse, CashTokenUtxo, FungibleToken, HistoryParams, HistoryResponse, NftUtxo,
    SendResult, SubscribeRequest, WatchtowerClient,
};

use super::keys::{AddressSet, HdWallet};

/// Core BCH wallet combining key derivation with Watchtower API.
pub struct BchWallet {
    chipnet: bool,
    wallet_hash: String,
    hd_wallet: HdWallet,
    watchtower: WatchtowerClient,
    project_id: String,
    mnemonic: String,
    derivation_path: String,
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

impl BchWallet {
    /// Create a new BchWallet.
    pub fn new(project_id: &str, mnemonic: &str, path: &str, chipnet: bool) -> Result<Self> {
        let hd_wallet = HdWallet::new(mnemonic, path, chipnet)?;
        let wallet_hash = hd_wallet.wallet_hash().to_string();
        let watchtower = WatchtowerClient::new(chipnet);

        Ok(Self {
            chipnet,
            wallet_hash,
            hd_wallet,
            watchtower,
            project_id: project_id.to_string(),
            mnemonic: mnemonic.to_string(),
            derivation_path: path.to_string(),
        })
    }

    pub fn wallet_hash(&self) -> &str {
        &self.wallet_hash
    }

    pub fn is_chipnet(&self) -> bool {
        self.chipnet
    }

    /// Derive receiving + change addresses at index (delegates to HdWallet).
    pub fn get_address_set_at(&self, index: u32) -> Result<AddressSet> {
        self.hd_wallet.get_address_set_at(index)
    }

    /// Derive token-aware addresses at index.
    pub fn get_token_address_set_at(&self, index: u32) -> Result<AddressSet> {
        self.hd_wallet.get_token_address_set_at(index)
    }

    /// Subscribe a new address set with Watchtower for monitoring.
    pub async fn get_new_address_set(&self, index: u32) -> Result<Option<AddressSet>> {
        let addresses = self.hd_wallet.get_address_set_at(index)?;
        // Subscribe receiving address
        let recv_result = self
            .watchtower
            .subscribe(&SubscribeRequest {
                address: addresses.receiving.clone(),
                project_id: self.project_id.clone(),
                wallet_hash: Some(self.wallet_hash.clone()),
                wallet_index: Some(index),
            })
            .await?;
        // Subscribe change address
        let _ = self
            .watchtower
            .subscribe(&SubscribeRequest {
                address: addresses.change.clone(),
                project_id: self.project_id.clone(),
                wallet_hash: Some(self.wallet_hash.clone()),
                wallet_index: Some(index),
            })
            .await;
        if recv_result.success {
            Ok(Some(addresses))
        } else {
            Ok(None)
        }
    }

    /// Register addresses and trigger UTXO scan with Watchtower.
    /// Called automatically before balance-sensitive operations.
    pub async fn ensure_synced(&self, address_count: u32) -> Result<()> {
        let _ = self.scan_addresses(0, address_count).await;
        let _ = self.scan_utxos(true).await;
        Ok(())
    }

    /// Get wallet BCH balance.
    pub async fn get_balance(&self) -> Result<BalanceResponse> {
        self.watchtower.get_balance(&self.wallet_hash).await
    }

    /// Get token balance.
    pub async fn get_token_balance(&self, token_id: &str) -> Result<BalanceResponse> {
        self.watchtower
            .get_token_balance(&self.wallet_hash, token_id)
            .await
    }

    /// Get transaction history.
    pub async fn get_history(&self, opts: HistoryOptions) -> Result<HistoryResponse> {
        let params = HistoryParams {
            wallet_hash: self.wallet_hash.clone(),
            token_id: opts.token_id,
            page: opts.page,
            record_type: opts.record_type,
        };
        self.watchtower.get_history(&params).await
    }

    /// Get last used address index from Watchtower.
    pub async fn get_last_address_index(&self) -> Result<Option<u32>> {
        self.watchtower
            .get_last_address_index(&self.wallet_hash)
            .await
    }

    /// Subscribe a range of addresses to Watchtower for monitoring.
    /// Registers both regular (q-prefix) and token-aware (z-prefix) addresses.
    pub async fn scan_addresses(&self, start_index: u32, count: u32) -> Result<()> {
        let end_index = start_index + count;
        for i in start_index..end_index {
            // Regular addresses (q-prefix)
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

            // Token-aware addresses (z-prefix)
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

    /// Send BCH to a recipient using native transaction building.
    ///
    /// 1. Fetches UTXOs from Watchtower
    /// 2. Builds and signs transaction locally
    /// 3. Broadcasts via Watchtower
    pub async fn send_bch(
        &self,
        amount: f64,
        address: &str,
        change_address: Option<&str>,
    ) -> Result<SendResult> {
        let change_addr = match change_address {
            Some(addr) => addr.to_string(),
            None => {
                let set = self.hd_wallet.get_address_set_at(0)?;
                set.change
            }
        };

        // Convert BCH to satoshis
        let amount_sats = (amount * 1e8).round() as u64;
        if amount_sats == 0 {
            return Ok(SendResult {
                success: false,
                txid: None,
                error: Some("amount must be greater than zero".to_string()),
                lacking_sats: None,
            });
        }

        // Fetch UTXOs
        let utxos = self
            .watchtower
            .get_bch_utxos(&self.wallet_hash)
            .await?;

        if utxos.is_empty() {
            return Ok(SendResult {
                success: false,
                txid: None,
                error: Some("no spendable UTXOs found".to_string()),
                lacking_sats: Some(amount_sats),
            });
        }

        // Build outputs
        let outputs = vec![transaction::TxOutput {
            address: address.to_string(),
            value: amount_sats,
        }];

        // Build and sign the transaction
        let built = match transaction::build_p2pkh_transaction(
            &utxos,
            &outputs,
            &change_addr,
            &self.hd_wallet,
            1.2, // fee rate: 1.2 sats/byte
        ) {
            Ok(tx) => tx,
            Err(e) => {
                let err_msg = e.to_string();
                // Try to extract lacking sats from error message
                let lacking = if err_msg.contains("short by") {
                    err_msg
                        .split("short by ")
                        .nth(1)
                        .and_then(|s| s.split(' ').next())
                        .and_then(|s| s.parse::<u64>().ok())
                } else {
                    None
                };
                return Ok(SendResult {
                    success: false,
                    txid: None,
                    error: Some(err_msg),
                    lacking_sats: lacking,
                });
            }
        };

        // Broadcast
        let broadcast_result = self.watchtower.broadcast(&built.hex).await?;

        if broadcast_result.success {
            Ok(SendResult {
                success: true,
                txid: broadcast_result.txid.or(Some(built.txid)),
                error: None,
                lacking_sats: None,
            })
        } else {
            Ok(SendResult {
                success: false,
                txid: None,
                error: broadcast_result.error,
                lacking_sats: None,
            })
        }
    }

    /// Fetch BCH UTXOs from Watchtower.
    pub async fn get_bch_utxos(&self) -> Result<Vec<crate::transaction::Utxo>> {
        self.watchtower.get_bch_utxos(&self.wallet_hash).await
    }

    /// Broadcast a raw transaction hex via Watchtower.
    pub async fn broadcast(
        &self,
        tx_hex: &str,
    ) -> Result<crate::watchtower::client::BroadcastResult> {
        self.watchtower.broadcast(tx_hex).await
    }

    /// Send fungible CashTokens.
    ///
    /// 1. Fetches token UTXOs for the category
    /// 2. Selects enough to cover the send amount
    /// 3. Fetches BCH UTXOs for fees
    /// 4. Builds, signs, and broadcasts the transaction locally
    pub async fn send_token(
        &self,
        category: &str,
        amount: u64,
        address: &str,
        change_address: Option<&str>,
    ) -> Result<SendResult> {
        let token_change_addr = match change_address {
            Some(addr) => addr.to_string(),
            None => self.hd_wallet.get_token_address_set_at(0)?.change,
        };
        let bch_change_addr = self.hd_wallet.get_address_set_at(0)?.change;

        // Fetch token UTXOs for this category
        let token_utxos = self
            .watchtower
            .get_cashtoken_utxos(&self.wallet_hash, category)
            .await
            .context("failed to fetch token UTXOs")?;

        // Select fungible token UTXOs (no NFT capability)
        let fungible_utxos: Vec<_> = token_utxos
            .iter()
            .filter(|u| u.capability.is_none())
            .collect();

        if fungible_utxos.is_empty() {
            return Ok(SendResult {
                success: false,
                txid: None,
                error: Some("no fungible token UTXOs found for this category".to_string()),
                lacking_sats: None,
            });
        }

        // Accumulate until we have enough tokens
        let mut selected: Vec<&CashTokenUtxo> = Vec::new();
        let mut token_total: u64 = 0;
        for utxo in &fungible_utxos {
            selected.push(utxo);
            token_total += utxo.token_amount;
            if token_total >= amount {
                break;
            }
        }

        if token_total < amount {
            return Ok(SendResult {
                success: false,
                txid: None,
                error: Some(format!(
                    "insufficient token balance: have {}, need {}",
                    token_total, amount
                )),
                lacking_sats: None,
            });
        }

        // Build token outputs
        let category_bytes = transaction::decode_txid_to_bytes(category)
            .context("invalid category hex")?;

        let mut outputs = vec![transaction::TokenTxOutput {
            address: address.to_string(),
            value: transaction::token_dust(),
            token: Some(transaction::TokenPrefix {
                category: category_bytes,
                nft: None,
                amount,
            }),
        }];

        // Token change output (if we selected more tokens than needed)
        let token_change = token_total - amount;
        if token_change > 0 {
            outputs.push(transaction::TokenTxOutput {
                address: token_change_addr,
                value: transaction::token_dust(),
                token: Some(transaction::TokenPrefix {
                    category: category_bytes,
                    nft: None,
                    amount: token_change,
                }),
            });
        }

        // Convert selected token UTXOs to transaction inputs (with token data for sighash)
        let mut all_inputs: Vec<transaction::Utxo> = selected
            .iter()
            .map(|u| transaction::Utxo {
                txid: u.txid.clone(),
                vout: u.vout,
                value: u.value,
                address_path: u.address_path.clone(),
                token: Some(transaction::TokenPrefix {
                    category: category_bytes,
                    nft: None,
                    amount: u.token_amount,
                }),
            })
            .collect();

        // Fetch BCH UTXOs for fees
        let bch_utxos = self
            .watchtower
            .get_bch_utxos(&self.wallet_hash)
            .await
            .context("failed to fetch BCH UTXOs")?;

        let mut sorted_bch = bch_utxos.clone();
        sorted_bch.sort_by(|a, b| b.value.cmp(&a.value));

        let output_bch: u64 = outputs.iter().map(|o| o.value).sum();
        for utxo in &sorted_bch {
            let input_bch: u64 = all_inputs.iter().map(|i| i.value).sum();
            if input_bch >= output_bch + 2000 {
                break;
            }
            all_inputs.push(utxo.clone());
        }

        // Build, sign, and broadcast
        let built = match transaction::build_token_transaction(
            &all_inputs,
            &outputs,
            &bch_change_addr,
            &self.hd_wallet,
            1.2,
        ) {
            Ok(tx) => tx,
            Err(e) => {
                let err_msg = e.to_string();
                let lacking = if err_msg.contains("short by") {
                    err_msg
                        .split("short by ")
                        .nth(1)
                        .and_then(|s| s.split(' ').next())
                        .and_then(|s| s.parse::<u64>().ok())
                } else {
                    None
                };
                return Ok(SendResult {
                    success: false,
                    txid: None,
                    error: Some(err_msg),
                    lacking_sats: lacking,
                });
            }
        };

        let broadcast_result = self.watchtower.broadcast(&built.hex).await?;

        if broadcast_result.success {
            Ok(SendResult {
                success: true,
                txid: broadcast_result.txid.or(Some(built.txid)),
                error: None,
                lacking_sats: None,
            })
        } else {
            Ok(SendResult {
                success: false,
                txid: None,
                error: broadcast_result.error,
                lacking_sats: None,
            })
        }
    }

    /// Send an NFT (non-fungible CashToken).
    ///
    /// 1. Fetches the specific NFT UTXO (by txid:vout)
    /// 2. Fetches BCH UTXOs for fees
    /// 3. Builds, signs, and broadcasts the transaction locally
    pub async fn send_nft(&self, params: NftSendParams) -> Result<SendResult> {
        let bch_change_addr = match &params.change_address {
            Some(addr) => addr.clone(),
            None => self.hd_wallet.get_address_set_at(0)?.change,
        };

        // Fetch the NFT UTXO to get its address_path for signing
        let token_utxos = self
            .watchtower
            .get_cashtoken_utxos(&self.wallet_hash, &params.category)
            .await
            .context("failed to fetch token UTXOs")?;

        let nft_utxo = token_utxos
            .iter()
            .find(|u| u.txid == params.txid && u.vout == params.vout)
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "NFT UTXO {}:{} not found in wallet",
                    params.txid,
                    params.vout
                )
            })?;

        // Build NFT output
        let category_bytes = transaction::decode_txid_to_bytes(&params.category)
            .context("invalid category hex")?;

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
                nft: Some(transaction::NftData {
                    capability,
                    commitment: commitment_bytes,
                }),
                amount: 0,
            }),
        }];

        // NFT UTXO as first input (with token data for sighash)
        let nft_commitment_bytes = if nft_utxo.commitment.is_empty() {
            Vec::new()
        } else {
            hex::decode(&nft_utxo.commitment).unwrap_or_default()
        };
        let nft_capability = transaction::NftCapability::parse(
            nft_utxo.capability.as_deref().unwrap_or("none")
        ).unwrap_or(transaction::NftCapability::None);
        let mut all_inputs = vec![transaction::Utxo {
            txid: nft_utxo.txid.clone(),
            vout: nft_utxo.vout,
            value: nft_utxo.value,
            address_path: nft_utxo.address_path.clone(),
            token: Some(transaction::TokenPrefix {
                category: category_bytes,
                nft: Some(transaction::NftData {
                    capability: nft_capability,
                    commitment: nft_commitment_bytes,
                }),
                amount: nft_utxo.token_amount,
            }),
        }];

        // Fetch BCH UTXOs for fees
        let bch_utxos = self
            .watchtower
            .get_bch_utxos(&self.wallet_hash)
            .await
            .context("failed to fetch BCH UTXOs")?;

        let mut sorted_bch = bch_utxos.clone();
        sorted_bch.sort_by(|a, b| b.value.cmp(&a.value));

        let output_bch: u64 = outputs.iter().map(|o| o.value).sum();
        for utxo in &sorted_bch {
            let input_bch: u64 = all_inputs.iter().map(|i| i.value).sum();
            if input_bch >= output_bch + 2000 {
                break;
            }
            all_inputs.push(utxo.clone());
        }

        // Build, sign, and broadcast
        let built = match transaction::build_token_transaction(
            &all_inputs,
            &outputs,
            &bch_change_addr,
            &self.hd_wallet,
            1.2,
        ) {
            Ok(tx) => tx,
            Err(e) => {
                let err_msg = e.to_string();
                let lacking = if err_msg.contains("short by") {
                    err_msg
                        .split("short by ")
                        .nth(1)
                        .and_then(|s| s.split(' ').next())
                        .and_then(|s| s.parse::<u64>().ok())
                } else {
                    None
                };
                return Ok(SendResult {
                    success: false,
                    txid: None,
                    error: Some(err_msg),
                    lacking_sats: lacking,
                });
            }
        };

        let broadcast_result = self.watchtower.broadcast(&built.hex).await?;

        if broadcast_result.success {
            Ok(SendResult {
                success: true,
                txid: broadcast_result.txid.or(Some(built.txid)),
                error: None,
                lacking_sats: None,
            })
        } else {
            Ok(SendResult {
                success: false,
                txid: None,
                error: broadcast_result.error,
                lacking_sats: None,
            })
        }
    }

    /// List fungible CashTokens.
    pub async fn get_fungible_tokens(&self) -> Result<Vec<FungibleToken>> {
        self.watchtower
            .get_fungible_tokens(&self.wallet_hash)
            .await
    }

    /// Get info for a specific CashToken.
    pub async fn get_token_info(&self, category: &str) -> Result<Option<FungibleToken>> {
        self.watchtower.get_token_info(category).await
    }

    /// Get NFT UTXOs.
    pub async fn get_nft_utxos(&self, category: Option<&str>) -> Result<Vec<NftUtxo>> {
        self.watchtower
            .get_nft_utxos(&self.wallet_hash, category)
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::network::BCH_DERIVATION_PATH;

    const TEST_MNEMONIC: &str =
        "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";

    #[test]
    fn test_bch_wallet_new() {
        let wallet = BchWallet::new("project123", TEST_MNEMONIC, BCH_DERIVATION_PATH, false);
        assert!(wallet.is_ok());
        let w = wallet.unwrap();
        assert!(!w.is_chipnet());
        assert!(!w.wallet_hash().is_empty());
    }

    #[test]
    fn test_bch_wallet_chipnet() {
        let wallet =
            BchWallet::new("project123", TEST_MNEMONIC, BCH_DERIVATION_PATH, true).unwrap();
        assert!(wallet.is_chipnet());
    }

    #[test]
    fn test_bch_wallet_address_derivation() {
        let wallet =
            BchWallet::new("project123", TEST_MNEMONIC, BCH_DERIVATION_PATH, false).unwrap();
        let set = wallet.get_address_set_at(0).unwrap();
        assert!(set.receiving.starts_with("bitcoincash:q"));
        assert!(set.change.starts_with("bitcoincash:q"));
    }

    #[test]
    fn test_bch_wallet_token_address_derivation() {
        let wallet =
            BchWallet::new("project123", TEST_MNEMONIC, BCH_DERIVATION_PATH, false).unwrap();
        let set = wallet.get_token_address_set_at(0).unwrap();
        assert!(set.receiving.starts_with("bitcoincash:z"));
        assert!(set.change.starts_with("bitcoincash:z"));
    }

    #[test]
    fn test_bch_wallet_invalid_mnemonic() {
        let result = BchWallet::new("project123", "bad words here", BCH_DERIVATION_PATH, false);
        assert!(result.is_err());
    }
}
