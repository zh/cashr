/// BchWallet: business logic combining HdWallet + Mainnet Cash REST API.
///
/// Security model:
/// - Read operations (balance, UTXOs, history): use watch:{network}:{cashaddr} wallet ID
/// - Broadcast: submit_transaction with watch wallet ID + locally-built raw tx hex
/// - Key material NEVER leaves the machine
use anyhow::{Context, Result};

use crate::constants::{DEFAULT_FEE_RATE, DEFAULT_PAGE_SIZE, FEE_RESERVE_SATS, SATS_PER_BCH};
use crate::network;
use crate::transaction;
use crate::types::{
    BalanceResponse, BroadcastResult, CashTokenUtxo, FungibleToken, HistoryResponse,
    NftUtxo, SendResult,
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

/// Number of address indices to track (receiving + change at each index).
const ADDRESS_SCAN_COUNT: u32 = 10;

/// Core BCH wallet combining local key derivation with Mainnet Cash REST API.
pub struct BchWallet {
    wallet_hash: String,
    hd_wallet: HdWallet,
    api_config: mainnet::apis::configuration::Configuration,
    /// Watch wallet IDs for all tracked addresses (receiving + change + token addresses).
    watch_ids: Vec<String>,
    /// Watch wallet IDs for regular (q-prefix) addresses only — for BCH operations.
    bch_watch_ids: Vec<String>,
    /// Watch wallet IDs for token-aware (z-prefix) addresses only — for token operations.
    token_watch_ids: Vec<String>,
    /// Address path mapping: watch_id -> address_path (e.g. "0/0", "1/3")
    watch_paths: std::collections::HashMap<String, String>,
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
    pub fn new(mnemonic: &str, path: &str, chipnet: bool) -> Result<Self> {
        let hd_wallet = HdWallet::new(mnemonic, path, chipnet)?;
        let wallet_hash = hd_wallet.wallet_hash().to_string();
        let api_config = network::mainnet_config(chipnet);

        // Build watch IDs for first N address indices (receiving + change + token).
        // Regular (q-prefix) and token (z-prefix) addresses share the same hash160,
        // so the API returns the same UTXOs/balance for both. We track them separately
        // to avoid double-counting in BCH operations.
        let mut watch_ids = Vec::new();
        let mut bch_watch_ids = Vec::new();
        let mut token_watch_ids = Vec::new();
        let mut watch_paths = std::collections::HashMap::new();
        for i in 0..ADDRESS_SCAN_COUNT {
            let addrs = hd_wallet.get_address_set_at(i)?;
            let token_addrs = hd_wallet.get_token_address_set_at(i)?;

            // Regular addresses (for BCH balance/history — no double-counting)
            for (addr, addr_path) in [
                (&addrs.receiving, format!("0/{}", i)),
                (&addrs.change, format!("1/{}", i)),
            ] {
                let wid = network::watch_wallet_id(chipnet, addr);
                if !watch_paths.contains_key(&wid) {
                    watch_ids.push(wid.clone());
                    bch_watch_ids.push(wid.clone());
                    watch_paths.insert(wid, addr_path);
                }
            }

            // Token addresses (for token operations only)
            for (addr, addr_path) in [
                (&token_addrs.receiving, format!("0/{}", i)),
                (&token_addrs.change, format!("1/{}", i)),
            ] {
                let wid = network::watch_wallet_id(chipnet, addr);
                if !watch_paths.contains_key(&wid) {
                    watch_ids.push(wid.clone());
                    token_watch_ids.push(wid.clone());
                    watch_paths.insert(wid, addr_path);
                }
            }
        }

        Ok(Self {
            wallet_hash,
            hd_wallet,
            api_config,
            watch_ids,
            bch_watch_ids,
            token_watch_ids,
            watch_paths,
        })
    }

    /// Derive receiving + change addresses at index (delegates to HdWallet).
    pub fn get_address_set_at(&self, index: u32) -> Result<AddressSet> {
        self.hd_wallet.get_address_set_at(index)
    }

    /// Derive token-aware addresses at index.
    pub fn get_token_address_set_at(&self, index: u32) -> Result<AddressSet> {
        self.hd_wallet.get_token_address_set_at(index)
    }

    // ── Read operations (watch wallet ID -- no keys exposed) ────────

    /// Get wallet BCH balance via Mainnet Cash API (parallel across all tracked addresses).
    pub async fn get_balance(&self) -> Result<BalanceResponse> {
        let futures: Vec<_> = self.bch_watch_ids.iter().map(|wid| {
            mainnet::apis::wallet_api::balance(
                &self.api_config,
                mainnet::models::BalanceRequest {
                    wallet_id: wid.clone(),
                    slp_semi_aware: None,
                },
            )
        }).collect();

        let results = futures::future::join_all(futures).await;
        let total_sats: f64 = results.into_iter()
            .filter_map(|r| r.ok())
            .filter_map(|r| r.sat)
            .filter_map(|s| s.parse::<f64>().ok())
            .sum();

        Ok(BalanceResponse {
            valid: true,
            wallet: self.wallet_hash.clone(),
            balance: total_sats / SATS_PER_BCH,
            spendable: total_sats / SATS_PER_BCH,
        })
    }

    /// Get per-address BCH balances (for verbose/debug output).
    pub async fn get_address_balances(&self) -> Result<Vec<(String, String, f64)>> {
        let futures: Vec<_> = self.bch_watch_ids.iter().map(|wid| {
            mainnet::apis::wallet_api::balance(
                &self.api_config,
                mainnet::models::BalanceRequest {
                    wallet_id: wid.clone(),
                    slp_semi_aware: None,
                },
            )
        }).collect();

        let results = futures::future::join_all(futures).await;
        let mut balances = Vec::new();
        for (wid, result) in self.bch_watch_ids.iter().zip(results) {
            let sats: f64 = result
                .ok()
                .and_then(|r| r.sat)
                .and_then(|s| s.parse::<f64>().ok())
                .unwrap_or(0.0);
            if sats > 0.0 {
                let path = self.watch_paths.get(wid).cloned().unwrap_or_default();
                // Extract the cashaddr from the watch_id (format: "watch:{network}:{addr}")
                let addr = wid.splitn(3, ':').nth(2).unwrap_or(wid).to_string();
                balances.push((path, addr, sats));
            }
        }
        Ok(balances)
    }

    /// Get token balance for a specific category (across all token addresses).
    pub async fn get_token_balance(&self, category: &str) -> Result<BalanceResponse> {
        let futures: Vec<_> = self.token_watch_ids.iter().map(|wid| {
            mainnet::apis::wallet_api::get_token_balance(
                &self.api_config,
                mainnet::models::GetTokenBalanceRequest {
                    wallet_id: wid.clone(),
                    category: category.to_string(),
                },
            )
        }).collect();

        let results = futures::future::join_all(futures).await;
        let balance: f64 = results.into_iter()
            .filter_map(|r| r.ok())
            .filter_map(|r| r.balance)
            .sum();

        Ok(BalanceResponse {
            valid: true,
            wallet: self.wallet_hash.clone(),
            balance,
            spendable: balance,
        })
    }

    /// Get BCH (non-token) UTXOs via Mainnet Cash API (parallel across all tracked addresses).
    pub async fn get_bch_utxos(&self) -> Result<Vec<transaction::Utxo>> {
        let futures: Vec<_> = self.watch_ids.iter().map(|wid| {
            let watch = serde_json::json!({ "walletId": wid });
            let wid = wid.clone();
            let config = &self.api_config;
            async move {
                let utxos = mainnet::apis::wallet_api::utxos(config, watch).await.ok();
                (wid, utxos)
            }
        }).collect();

        let results = futures::future::join_all(futures).await;
        let mut result = Vec::new();
        for (wid, utxos) in results {
            let Some(utxos) = utxos else { continue };
            let addr_path = self.watch_paths.get(&wid).cloned().unwrap_or_else(|| "0/0".to_string());
            for u in utxos {
                if let Some(Some(_token)) = &u.token { continue; }
                let value = u.satoshis as u64;
                if value < 546 { continue; }
                result.push(transaction::Utxo {
                    txid: u.txid,
                    vout: u.vout as u32,
                    value,
                    address_path: addr_path.clone(),
                    token: None,
                });
            }
        }
        Ok(result)
    }

    /// Get CashToken UTXOs for a specific category (parallel across all tracked addresses).
    pub async fn get_cashtoken_utxos(&self, category: &str) -> Result<Vec<CashTokenUtxo>> {
        let cat = category.to_string();
        let futures: Vec<_> = self.watch_ids.iter().map(|wid| {
            let wid = wid.clone();
            let cat = cat.clone();
            let config = &self.api_config;
            async move {
                let resp = mainnet::apis::wallet_api::get_token_utxos(
                    config,
                    mainnet::models::GetTokenUtxosRequest {
                        wallet_id: wid.clone(),
                        category: Some(Some(cat)),
                    },
                ).await.ok();
                (wid, resp)
            }
        }).collect();

        let results = futures::future::join_all(futures).await;
        let mut utxos = Vec::new();
        for (wid, resp) in results {
            let Some(items) = resp else { continue };
            let addr_path = self.watch_paths.get(&wid).cloned().unwrap_or_else(|| "0/0".to_string());
            for u in items {
                let token = match &u.token {
                    Some(Some(t)) => t,
                    _ => continue,
                };
                let tc = token.category.clone().unwrap_or_default();
                if tc != category { continue; }
                let token_amount = token.amount.unwrap_or(0.0) as u64;
                let (commitment, capability) = match &token.nft {
                    Some(Some(nft)) => {
                        let cap = match nft.capability {
                            mainnet::models::token_nft::Capability::None => Some("none".to_string()),
                            mainnet::models::token_nft::Capability::Mutable => Some("mutable".to_string()),
                            mainnet::models::token_nft::Capability::Minting => Some("minting".to_string()),
                        };
                        (nft.commitment.clone(), cap)
                    }
                    _ => (String::new(), None),
                };
                utxos.push(CashTokenUtxo {
                    txid: u.txid, vout: u.vout as u32, value: u.satoshis as u64,
                    address_path: addr_path.clone(), token_amount, commitment, capability,
                });
            }
        }
        Ok(utxos)
    }

    /// Broadcast a raw transaction hex via Mainnet Cash API.
    pub async fn broadcast(&self, tx_hex: &str) -> Result<BroadcastResult> {
        let resp = mainnet::apis::wallet_api::submit_transaction(
            &self.api_config,
            mainnet::models::SubmitTransactionRequest {
                wallet_id: self.watch_ids[0].clone(),
                transaction_hex: tx_hex.to_string(),
                await_propagation: Some(true),
            },
        )
        .await;

        match resp {
            Ok(r) => Ok(BroadcastResult {
                txid: r.tx_id,
                success: true,
                error: None,
            }),
            Err(e) => Ok(BroadcastResult {
                txid: None,
                success: false,
                error: Some(format!("{:?}", e)),
            }),
        }
    }

    /// Get transaction history via Mainnet Cash API.
    pub async fn get_history(&self, opts: HistoryOptions) -> Result<HistoryResponse> {
        // Query all tracked addresses in parallel, then merge & deduplicate.
        let page_size: usize = DEFAULT_PAGE_SIZE;
        // Fetch enough from each address to fill the requested page after merging.
        let fetch_count = (opts.page as f64) * (page_size as f64) + 1.0;

        // Use bch_watch_ids for BCH history, full watch_ids for token history.
        let ids = if opts.token_id.is_empty() {
            &self.bch_watch_ids
        } else {
            &self.watch_ids
        };
        let futures: Vec<_> = ids.iter().map(|wid| {
            mainnet::apis::wallet_api::get_history(
                &self.api_config,
                mainnet::models::HistoryRequest {
                    wallet_id: wid.clone(),
                    unit: Some(mainnet::models::history_request::Unit::Sat),
                    from_height: None,
                    to_height: None,
                    start: Some(0.0),
                    count: Some(fetch_count),
                },
            )
        }).collect();

        let results = futures::future::join_all(futures).await;

        // Merge results across addresses, summing value_change for same txid.
        let mut merged: std::collections::HashMap<String, mainnet::models::TransactionHistoryItem> =
            std::collections::HashMap::new();
        for item in results.into_iter().filter_map(|r| r.ok()).flatten() {
            let txid = item.hash.clone().unwrap_or_default();
            merged
                .entry(txid)
                .and_modify(|existing| {
                    // Sum BCH value_change across addresses
                    existing.value_change = Some(
                        existing.value_change.unwrap_or(0.0)
                            + item.value_change.unwrap_or(0.0),
                    );
                    // Merge token_amount_changes by category
                    if let Some(new_changes) = &item.token_amount_changes {
                        let ec = existing
                            .token_amount_changes
                            .get_or_insert_with(Vec::new);
                        for nc in new_changes {
                            let cat = nc.category.as_deref().unwrap_or("");
                            if let Some(found) = ec
                                .iter_mut()
                                .find(|c| c.category.as_deref().unwrap_or("") == cat)
                            {
                                found.amount = Some(
                                    found.amount.unwrap_or(0.0) + nc.amount.unwrap_or(0.0),
                                );
                                found.nft_amount = Some(
                                    found.nft_amount.unwrap_or(0.0)
                                        + nc.nft_amount.unwrap_or(0.0),
                                );
                            } else {
                                ec.push(nc.clone());
                            }
                        }
                    }
                })
                .or_insert(item);
        }
        let mut all_items: Vec<_> = merged.into_values().collect();

        // Sort by timestamp descending (most recent first).
        // Unconfirmed transactions (no timestamp) sort to the top.
        all_items.sort_by(|a, b| {
            let ta = a.timestamp.unwrap_or(f64::MAX);
            let tb = b.timestamp.unwrap_or(f64::MAX);
            tb.partial_cmp(&ta).unwrap_or(std::cmp::Ordering::Equal)
        });

        // Paginate the merged results.
        let start = (opts.page as usize - 1) * page_size;
        let has_next = all_items.len() > start + page_size;
        let display_items: Vec<_> = all_items
            .into_iter()
            .skip(start)
            .take(page_size)
            .collect();

        // Convert mainnet TransactionHistoryItem to our HistoryEntry format
        let history: Vec<crate::types::HistoryEntry> = display_items
            .iter()
            .map(|item| {
                let value_change = item.value_change.unwrap_or(0.0);
                let is_incoming = value_change > 0.0;
                let record_type = if is_incoming { "incoming" } else { "outgoing" };

                // value_change is in satoshis (we requested unit=sat)
                // Convert to BCH for display compatibility, unless token_id is set
                let amount = if opts.token_id.is_empty() {
                    value_change.abs() / SATS_PER_BCH
                } else {
                    // For token history, look at token_amount_changes
                    if let Some(changes) = &item.token_amount_changes {
                        changes
                            .iter()
                            .find(|c| {
                                c.category.as_deref() == Some(&opts.token_id)
                            })
                            .map(|c| c.amount.unwrap_or(0.0).abs())
                            .unwrap_or(0.0)
                    } else {
                        0.0
                    }
                };

                let timestamp = match item.timestamp {
                    Some(ts) => {
                        // Convert unix timestamp to ISO-ish date string
                        let secs = ts as i64;
                        let days = secs / 86400;
                        let rem = secs % 86400;
                        let hours = rem / 3600;
                        let mins = (rem % 3600) / 60;
                        // Simple epoch-days to date (good enough for display)
                        // 1970-01-01 is day 0
                        let (y, m, d) = epoch_days_to_date(days);
                        format!("{:04}-{:02}-{:02} {:02}:{:02} UTC", y, m, d, hours, mins)
                    }
                    None => String::new(),
                };

                // Extract token changes
                let token_changes: Vec<crate::types::TokenChange> = item
                    .token_amount_changes
                    .as_ref()
                    .map(|changes| {
                        changes
                            .iter()
                            .map(|c| crate::types::TokenChange {
                                category: c.category.clone().unwrap_or_default(),
                                amount: c.amount.unwrap_or(0.0),
                                nft_amount: c.nft_amount.unwrap_or(0.0),
                            })
                            .collect()
                    })
                    .unwrap_or_default();

                crate::types::HistoryEntry {
                    record_type: record_type.to_string(),
                    txid: item.hash.clone().unwrap_or_default(),
                    amount,
                    tx_fee: item.fee.unwrap_or(0.0) / SATS_PER_BCH,
                    senders: serde_json::Value::Null,
                    recipients: serde_json::Value::Null,
                    date_created: String::new(),
                    tx_timestamp: timestamp,
                    usd_price: 0.0,
                    market_prices: serde_json::Value::Null,
                    attributes: serde_json::Value::Null,
                    token_changes,
                }
            })
            .collect();

        // Filter by record_type if specified
        let filtered = if opts.record_type == "all" {
            history
        } else {
            history
                .into_iter()
                .filter(|h| h.record_type == opts.record_type)
                .collect()
        };

        Ok(HistoryResponse {
            history: filtered,
            page: opts.page.to_string(),
            num_pages: if has_next { opts.page + 1 } else { opts.page },
            has_next,
        })
    }

    /// Get per-address token balances (for verbose output).
    /// Returns Vec of (path, address, category -> amount).
    pub async fn get_address_token_balances(
        &self,
    ) -> Result<Vec<(String, String, std::collections::HashMap<String, f64>)>> {
        let futures: Vec<_> = self.token_watch_ids.iter().map(|wid| {
            mainnet::apis::wallet_api::get_all_token_balances(
                &self.api_config,
                mainnet::models::GetAllTokenBalancesRequest {
                    wallet_id: wid.clone(),
                },
            )
        }).collect();

        let results = futures::future::join_all(futures).await;
        let mut out = Vec::new();
        for (wid, result) in self.token_watch_ids.iter().zip(results) {
            let balances = match result {
                Ok(b) => b,
                Err(_) => continue,
            };
            let parsed: std::collections::HashMap<String, f64> = balances
                .into_iter()
                .filter_map(|(cat, amt_str)| {
                    let amt: f64 = amt_str.parse().ok()?;
                    if amt > 0.0 { Some((cat, amt)) } else { None }
                })
                .collect();
            if !parsed.is_empty() {
                let path = self.watch_paths.get(wid).cloned().unwrap_or_default();
                let addr = wid.splitn(3, ':').nth(2).unwrap_or(wid).to_string();
                out.push((path, addr, parsed));
            }
        }
        Ok(out)
    }

    /// List all fungible CashToken balances, with BCMR metadata.
    pub async fn get_fungible_tokens(&self) -> Result<Vec<FungibleToken>> {
        // Query all token-aware addresses in parallel and merge by category.
        let futures: Vec<_> = self.token_watch_ids.iter().map(|wid| {
            mainnet::apis::wallet_api::get_all_token_balances(
                &self.api_config,
                mainnet::models::GetAllTokenBalancesRequest {
                    wallet_id: wid.clone(),
                },
            )
        }).collect();

        let results = futures::future::join_all(futures).await;
        let mut merged: std::collections::HashMap<String, f64> = std::collections::HashMap::new();
        for result in results.into_iter().filter_map(|r| r.ok()) {
            for (category, amount_str) in result {
                let amount: f64 = amount_str.parse().unwrap_or(0.0);
                *merged.entry(category).or_insert(0.0) += amount;
            }
        }

        let mut tokens = Vec::new();
        for (category, amount) in merged {
            if amount <= 0.0 {
                continue;
            }

            // Fetch BCMR metadata for this category
            let (name, symbol, decimals) = self.fetch_bcmr_metadata(&category).await;

            tokens.push(FungibleToken {
                id: category.clone(),
                category,
                name,
                symbol,
                decimals,
                image_url: String::new(),
                balance: amount,
            });
        }

        Ok(tokens)
    }

    /// Get info for a specific CashToken (BCMR metadata).
    pub async fn get_token_info(&self, category: &str) -> Result<Option<FungibleToken>> {
        let (name, symbol, decimals) = self.fetch_bcmr_metadata(category).await;

        // Even if BCMR returns nothing, we still return a token with unknown name
        Ok(Some(FungibleToken {
            id: category.to_string(),
            category: category.to_string(),
            name,
            symbol,
            decimals,
            image_url: String::new(),
            balance: 0.0,
        }))
    }

    /// Get NFT UTXOs, optionally filtered by category (across all token addresses).
    pub async fn get_nft_utxos(&self, category: Option<&str>) -> Result<Vec<NftUtxo>> {
        let cat = category.map(|c| c.to_string());
        let futures: Vec<_> = self.token_watch_ids.iter().map(|wid| {
            mainnet::apis::wallet_api::get_token_utxos(
                &self.api_config,
                mainnet::models::GetTokenUtxosRequest {
                    wallet_id: wid.clone(),
                    category: cat.as_ref().map(|c| Some(c.clone())),
                },
            )
        }).collect();

        let results = futures::future::join_all(futures).await;

        // Deduplicate by (txid, vout) since same UTXO won't appear at multiple addresses.
        let mut seen: std::collections::HashSet<(String, u32)> = std::collections::HashSet::new();
        let mut nfts = Vec::new();
        for u in results.into_iter().filter_map(|r| r.ok()).flatten() {
            if !seen.insert((u.txid.clone(), u.vout as u32)) {
                continue;
            }
            let token = match &u.token {
                Some(Some(t)) => t,
                _ => continue,
            };

            // NFTs have a non-null nft field
            let nft = match &token.nft {
                Some(Some(n)) => n,
                _ => continue,
            };

            let cap = match nft.capability {
                mainnet::models::token_nft::Capability::None => "none",
                mainnet::models::token_nft::Capability::Mutable => "mutable",
                mainnet::models::token_nft::Capability::Minting => "minting",
            };

            nfts.push(NftUtxo {
                txid: u.txid,
                vout: u.vout as u32,
                category: token.category.clone().unwrap_or_default(),
                commitment: nft.commitment.clone(),
                capability: cap.to_string(),
                amount: token.amount.unwrap_or(0.0),
                value: u.satoshis,
            });
        }

        Ok(nfts)
    }

    // ── Send operations (local signing + broadcast) ─────────────────

    /// Send BCH to a recipient using native transaction building.
    ///
    /// 1. Fetches UTXOs from Mainnet Cash API
    /// 2. Builds and signs transaction locally
    /// 3. Broadcasts via Mainnet Cash API
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
        let amount_sats = (amount * SATS_PER_BCH).round() as u64;
        if amount_sats == 0 {
            return Ok(SendResult {
                success: false,
                txid: None,
                error: Some("amount must be greater than zero".to_string()),
                lacking_sats: None,
            });
        }

        // Fetch UTXOs
        let utxos = self.get_bch_utxos().await?;

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
            DEFAULT_FEE_RATE,
        ) {
            Ok(tx) => tx,
            Err(e) => {
                let err_msg = e.to_string();
                let lacking = extract_lacking_sats(&err_msg);
                return Ok(SendResult {
                    success: false,
                    txid: None,
                    error: Some(err_msg),
                    lacking_sats: lacking,
                });
            }
        };

        // Broadcast
        let broadcast_result = self.broadcast(&built.hex).await?;

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

    /// Send fungible CashTokens.
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
            .get_cashtoken_utxos(category)
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
        let category_bytes =
            transaction::decode_txid_to_bytes(category).context("invalid category hex")?;

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

        // Convert selected token UTXOs to transaction inputs
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
            .get_bch_utxos()
            .await
            .context("failed to fetch BCH UTXOs")?;

        let mut sorted_bch = bch_utxos.clone();
        sorted_bch.sort_by(|a, b| b.value.cmp(&a.value));

        let output_bch: u64 = outputs.iter().map(|o| o.value).sum();
        for utxo in &sorted_bch {
            let input_bch: u64 = all_inputs.iter().map(|i| i.value).sum();
            if input_bch >= output_bch + FEE_RESERVE_SATS {
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
            DEFAULT_FEE_RATE,
        ) {
            Ok(tx) => tx,
            Err(e) => {
                let err_msg = e.to_string();
                let lacking = extract_lacking_sats(&err_msg);
                return Ok(SendResult {
                    success: false,
                    txid: None,
                    error: Some(err_msg),
                    lacking_sats: lacking,
                });
            }
        };

        let broadcast_result = self.broadcast(&built.hex).await?;

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
    pub async fn send_nft(&self, params: NftSendParams) -> Result<SendResult> {
        let bch_change_addr = match &params.change_address {
            Some(addr) => addr.clone(),
            None => self.hd_wallet.get_address_set_at(0)?.change,
        };

        // Fetch the NFT UTXO to get its data for signing
        let token_utxos = self
            .get_cashtoken_utxos(&params.category)
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
        let category_bytes =
            transaction::decode_txid_to_bytes(&params.category).context("invalid category hex")?;

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

        // NFT UTXO as first input
        let nft_commitment_bytes = if nft_utxo.commitment.is_empty() {
            Vec::new()
        } else {
            hex::decode(&nft_utxo.commitment).unwrap_or_default()
        };
        let nft_capability =
            transaction::NftCapability::parse(nft_utxo.capability.as_deref().unwrap_or("none"))
                .unwrap_or(transaction::NftCapability::None);
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
            .get_bch_utxos()
            .await
            .context("failed to fetch BCH UTXOs")?;

        let mut sorted_bch = bch_utxos.clone();
        sorted_bch.sort_by(|a, b| b.value.cmp(&a.value));

        let output_bch: u64 = outputs.iter().map(|o| o.value).sum();
        for utxo in &sorted_bch {
            let input_bch: u64 = all_inputs.iter().map(|i| i.value).sum();
            if input_bch >= output_bch + FEE_RESERVE_SATS {
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
            DEFAULT_FEE_RATE,
        ) {
            Ok(tx) => tx,
            Err(e) => {
                let err_msg = e.to_string();
                let lacking = extract_lacking_sats(&err_msg);
                return Ok(SendResult {
                    success: false,
                    txid: None,
                    error: Some(err_msg),
                    lacking_sats: lacking,
                });
            }
        };

        let broadcast_result = self.broadcast(&built.hex).await?;

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

    // ── Internal helpers ────────────────────────────────────────────

    /// Fetch BCMR token metadata (name, symbol, decimals) for a category.
    async fn fetch_bcmr_metadata(&self, category: &str) -> (String, String, u32) {
        let resp = mainnet::apis::wallet_bcmr_api::bcmr_get_token_info(
            &self.api_config,
            mainnet::models::BcmrGetTokenInfoRequest {
                category: category.to_string(),
            },
        )
        .await;

        match resp {
            Ok(info) => {
                if let Some(Some(token_info)) = info.token_info {
                    let name = token_info
                        .get("name")
                        .and_then(|v| v.as_str())
                        .unwrap_or("Unknown Token")
                        .to_string();
                    let symbol = token_info
                        .get("token")
                        .and_then(|t| t.get("symbol"))
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    let decimals = token_info
                        .get("token")
                        .and_then(|t| t.get("decimals"))
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0) as u32;
                    (name, symbol, decimals)
                } else {
                    ("Unknown Token".to_string(), String::new(), 0)
                }
            }
            Err(_) => ("Unknown Token".to_string(), String::new(), 0),
        }
    }
}

/// Convert days since Unix epoch (1970-01-01) to (year, month, day).
/// Uses the civil calendar algorithm.
fn epoch_days_to_date(days: i64) -> (i64, i64, i64) {
    // Algorithm from Howard Hinnant's date library
    let z = days + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (y, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::network::BCH_DERIVATION_PATH;

    const TEST_MNEMONIC: &str =
        "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";

    #[test]
    fn test_bch_wallet_new() {
        let wallet = BchWallet::new(TEST_MNEMONIC, BCH_DERIVATION_PATH, false);
        assert!(wallet.is_ok());
    }

    #[test]
    fn test_bch_wallet_address_derivation() {
        let wallet = BchWallet::new(TEST_MNEMONIC, BCH_DERIVATION_PATH, false).unwrap();
        let set = wallet.get_address_set_at(0).unwrap();
        assert!(set.receiving.starts_with("bitcoincash:q"));
        assert!(set.change.starts_with("bitcoincash:q"));
    }

    #[test]
    fn test_bch_wallet_token_address_derivation() {
        let wallet = BchWallet::new(TEST_MNEMONIC, BCH_DERIVATION_PATH, false).unwrap();
        let set = wallet.get_token_address_set_at(0).unwrap();
        assert!(set.receiving.starts_with("bitcoincash:z"));
        assert!(set.change.starts_with("bitcoincash:z"));
    }

    #[test]
    fn test_bch_wallet_invalid_mnemonic() {
        let result = BchWallet::new("bad words here", BCH_DERIVATION_PATH, false);
        assert!(result.is_err());
    }

    #[test]
    fn test_watch_ids_include_receiving_address() {
        let wallet = BchWallet::new(TEST_MNEMONIC, BCH_DERIVATION_PATH, false).unwrap();
        let addr = wallet.get_address_set_at(0).unwrap().receiving;
        assert!(wallet.watch_ids.iter().any(|w| w.contains(&addr)));
        assert!(wallet.watch_ids[0].starts_with("watch:mainnet:"));
    }

    #[test]
    fn test_watch_ids_include_change_address() {
        let wallet = BchWallet::new(TEST_MNEMONIC, BCH_DERIVATION_PATH, false).unwrap();
        let change = wallet.get_address_set_at(0).unwrap().change;
        assert!(wallet.watch_ids.iter().any(|w| w.contains(&change)));
    }

    #[test]
    fn test_watch_ids_track_multiple_indices() {
        let wallet = BchWallet::new(TEST_MNEMONIC, BCH_DERIVATION_PATH, false).unwrap();
        // Should track 10 indices × 2 (receiving + change) = 20 unique addresses
        // Token addresses share the same hash so may deduplicate
        assert!(wallet.watch_ids.len() >= 20);
    }

    #[test]
    fn test_chipnet_watch_ids_use_testnet() {
        let wallet = BchWallet::new(TEST_MNEMONIC, BCH_DERIVATION_PATH, true).unwrap();
        assert!(wallet.watch_ids[0].starts_with("watch:testnet:"));
    }
}
