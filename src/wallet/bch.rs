/// BchWallet: business logic combining HdWallet + WatchtowerClient.
///
/// Provides all BCH wallet operations: balance, send, tokens, history.
use anyhow::Result;

use crate::transaction;
use crate::watchtower::client::{
    AddressScanEntry, AddressScanRequest, AddressSetPayload, BalanceResponse, FungibleToken,
    HistoryParams, HistoryResponse, NftUtxo, SendResult, SubscribeRequest, WatchtowerClient,
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
        let data = SubscribeRequest {
            addresses: AddressSetPayload {
                receiving: addresses.receiving.clone(),
                change: addresses.change.clone(),
            },
            project_id: self.project_id.clone(),
            wallet_hash: self.wallet_hash.clone(),
            address_index: index,
        };
        let result = self.watchtower.subscribe(&data).await?;
        if result.success {
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

    /// Bulk-subscribe addresses to Watchtower.
    pub async fn scan_addresses(&self, start_index: u32, count: u32) -> Result<()> {
        let end_index = start_index + count;
        let mut address_sets = Vec::new();
        for i in start_index..end_index {
            let addrs = self.hd_wallet.get_address_set_at(i)?;
            address_sets.push(AddressScanEntry {
                address_index: i,
                addresses: AddressSetPayload {
                    receiving: addrs.receiving,
                    change: addrs.change,
                },
            });
        }
        let data = AddressScanRequest {
            address_sets,
            wallet_hash: self.wallet_hash.clone(),
            project_id: self.project_id.clone(),
        };
        self.watchtower.scan_addresses(&data).await
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
    pub async fn send_token(
        &self,
        _category: &str,
        _amount: u64,
        _address: &str,
        _change_address: Option<&str>,
    ) -> Result<SendResult> {
        todo!("CashToken sends require token-aware outputs -- not yet implemented")
    }

    /// Send an NFT.
    pub async fn send_nft(&self, _params: NftSendParams) -> Result<SendResult> {
        todo!("CashToken NFT sends require token-aware outputs -- not yet implemented")
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
