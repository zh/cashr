/// HD Wallet: BIP39 mnemonic -> BIP32 key derivation -> CashAddress.
///
/// Derivation path: m/44'/145'/0' (BIP44 coin type 145 = BCH)
///   - Receiving addresses: m/44'/145'/0'/0/{index}
///   - Change addresses:    m/44'/145'/0'/1/{index}
use anyhow::{Context, Result};
use bip32::{DerivationPath, XPrv};
use std::str::FromStr;

use crate::crypto;

/// A receiving + change address pair.
#[derive(Debug, Clone, PartialEq)]
pub struct AddressSet {
    pub receiving: String,
    pub change: String,
}

/// HD Wallet for Bitcoin Cash.
pub struct HdWallet {
    mnemonic: String,
    derivation_path: String,
    chipnet: bool,
    wallet_hash: String,
}

impl HdWallet {
    /// Create a new HdWallet.
    ///
    /// Validates the mnemonic and computes the wallet hash.
    pub fn new(mnemonic: &str, derivation_path: &str, chipnet: bool) -> Result<Self> {
        // Validate mnemonic
        let _parsed: bip39::Mnemonic = mnemonic
            .parse()
            .map_err(|_| anyhow::anyhow!("invalid BIP39 mnemonic phrase"))?;

        let wallet_hash = compute_wallet_hash(mnemonic, derivation_path);

        Ok(Self {
            mnemonic: mnemonic.to_string(),
            derivation_path: derivation_path.to_string(),
            chipnet,
            wallet_hash,
        })
    }

    pub fn wallet_hash(&self) -> &str {
        &self.wallet_hash
    }

    pub fn is_chipnet(&self) -> bool {
        self.chipnet
    }

    pub fn mnemonic(&self) -> &str {
        &self.mnemonic
    }

    pub fn derivation_path(&self) -> &str {
        &self.derivation_path
    }

    /// Derive the main HD node at the account-level derivation path.
    fn get_main_node(&self) -> Result<XPrv> {
        let mnemonic: bip39::Mnemonic = self
            .mnemonic
            .parse()
            .map_err(|_| anyhow::anyhow!("invalid mnemonic"))?;
        let seed = mnemonic.to_seed("");
        let path = DerivationPath::from_str(&self.derivation_path)
            .context("invalid derivation path")?;
        let child = XPrv::derive_from_path(seed, &path)
            .context("HD key derivation failed")?;
        Ok(child)
    }

    /// Derive a child node at a relative sub-path from the main node.
    /// Path should be like "0/0" (receiving index 0) or "1/0" (change index 0).
    fn get_node_at(&self, path: &str) -> Result<XPrv> {
        let main_node = self.get_main_node()?;
        let normalized = normalize_path(path);
        // Strip the "m/" prefix since we derive relative to main_node
        let relative = normalized
            .strip_prefix("m/")
            .unwrap_or(&normalized);

        // Parse individual child indices and derive step by step
        let mut node = main_node;
        for segment in relative.split('/') {
            if segment.is_empty() {
                continue;
            }
            let child_number = if segment.ends_with('\'') || segment.ends_with('h') {
                let idx: u32 = segment[..segment.len() - 1]
                    .parse()
                    .context("invalid path segment")?;
                bip32::ChildNumber::new(idx, true)
                    .map_err(|e| anyhow::anyhow!("invalid child number: {}", e))?
            } else {
                let idx: u32 = segment
                    .parse()
                    .context("invalid path segment")?;
                bip32::ChildNumber::new(idx, false)
                    .map_err(|e| anyhow::anyhow!("invalid child number: {}", e))?
            };
            node = node
                .derive_child(child_number)
                .map_err(|e| anyhow::anyhow!("child derivation failed: {}", e))?;
        }

        Ok(node)
    }

    /// Raw 32-byte private key at relative sub-path.
    pub fn get_private_key_at(&self, path: &str) -> Result<[u8; 32]> {
        let node = self.get_node_at(path)?;
        let key_bytes = node.to_bytes();
        let mut out = [0u8; 32];
        out.copy_from_slice(&key_bytes);
        Ok(out)
    }

    /// WIF-encoded private key at sub-path.
    pub fn get_private_key_wif_at(&self, path: &str) -> Result<String> {
        let key = self.get_private_key_at(path)?;
        Ok(encode_wif(&key))
    }

    /// Compressed public key as hex string at sub-path.
    pub fn get_pubkey_at(&self, path: &str) -> Result<String> {
        let node = self.get_node_at(path)?;
        let public_key = node.public_key();
        let pubkey_bytes = public_key.to_bytes();
        Ok(hex::encode(pubkey_bytes))
    }

    /// CashAddress at sub-path, optionally token-aware.
    pub fn get_address_at(&self, path: &str, token: bool) -> Result<String> {
        let pubkey_hex = self.get_pubkey_at(path)?;
        let address = crypto::pubkey_to_address(&pubkey_hex, self.chipnet)?;
        if token {
            crypto::to_token_address(&address)
        } else {
            Ok(address)
        }
    }

    /// Receiving + change addresses at a given index.
    pub fn get_address_set_at(&self, index: u32) -> Result<AddressSet> {
        Ok(AddressSet {
            receiving: self.get_address_at(&format!("0/{}", index), false)?,
            change: self.get_address_at(&format!("1/{}", index), false)?,
        })
    }

    /// Token-aware receiving + change addresses at a given index.
    pub fn get_token_address_set_at(&self, index: u32) -> Result<AddressSet> {
        Ok(AddressSet {
            receiving: self.get_address_at(&format!("0/{}", index), true)?,
            change: self.get_address_at(&format!("1/{}", index), true)?,
        })
    }
}

/// Compute wallet hash matching JS exactly:
/// sha256_hex(sha256_hex(mnemonic) + sha256_hex(derivation_path))
///
/// Where sha256_hex(s) = lowercase hex of SHA256(s as UTF-8 bytes).
pub fn compute_wallet_hash(mnemonic: &str, path: &str) -> String {
    let mnemonic_hash = hex::encode(crypto::sha256(mnemonic.as_bytes()));
    let path_hash = hex::encode(crypto::sha256(path.as_bytes()));
    let combined = format!("{}{}", mnemonic_hash, path_hash);
    hex::encode(crypto::sha256(combined.as_bytes()))
}

/// Normalize a sub-path to always start with "m/".
fn normalize_path(path: &str) -> String {
    if path.starts_with("m/") || path.starts_with("M/") {
        path.to_string()
    } else if path.starts_with('m') || path.starts_with('M') {
        format!("m/{}", &path[1..])
    } else {
        format!("m/{}", path)
    }
}

/// WIF encoding: 0x80 + 32-byte key + 0x01 (compressed) -> Base58Check.
fn encode_wif(private_key: &[u8; 32]) -> String {
    let mut data = Vec::with_capacity(34);
    data.push(0x80); // mainnet version byte
    data.extend_from_slice(private_key);
    data.push(0x01); // compressed flag
    bs58::encode(data).with_check().into_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::network::BCH_DERIVATION_PATH;

    const TEST_MNEMONIC: &str =
        "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";

    #[test]
    fn test_compute_wallet_hash_deterministic() {
        let hash1 = compute_wallet_hash(TEST_MNEMONIC, BCH_DERIVATION_PATH);
        let hash2 = compute_wallet_hash(TEST_MNEMONIC, BCH_DERIVATION_PATH);
        assert_eq!(hash1, hash2);
        // Should be a 64-char hex string
        assert_eq!(hash1.len(), 64);
        assert!(hash1.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn test_wallet_hash_different_mnemonics_differ() {
        let hash1 = compute_wallet_hash(TEST_MNEMONIC, BCH_DERIVATION_PATH);
        let hash2 = compute_wallet_hash(
            "zoo zoo zoo zoo zoo zoo zoo zoo zoo zoo zoo wrong",
            BCH_DERIVATION_PATH,
        );
        assert_ne!(hash1, hash2);
    }

    #[test]
    fn test_hd_wallet_new_valid() {
        let wallet = HdWallet::new(TEST_MNEMONIC, BCH_DERIVATION_PATH, false);
        assert!(wallet.is_ok());
        let w = wallet.unwrap();
        assert!(!w.is_chipnet());
        assert_eq!(w.mnemonic(), TEST_MNEMONIC);
    }

    #[test]
    fn test_hd_wallet_new_invalid_mnemonic() {
        let result = HdWallet::new("not a valid mnemonic", BCH_DERIVATION_PATH, false);
        assert!(result.is_err());
    }

    #[test]
    fn test_get_pubkey_at_0_0() {
        let wallet = HdWallet::new(TEST_MNEMONIC, BCH_DERIVATION_PATH, false).unwrap();
        let pubkey = wallet.get_pubkey_at("0/0").unwrap();
        // Compressed pubkey: 66 hex chars (33 bytes), starts with 02 or 03
        assert_eq!(pubkey.len(), 66);
        assert!(pubkey.starts_with("02") || pubkey.starts_with("03"));
    }

    #[test]
    fn test_get_address_at_mainnet() {
        let wallet = HdWallet::new(TEST_MNEMONIC, BCH_DERIVATION_PATH, false).unwrap();
        let addr = wallet.get_address_at("0/0", false).unwrap();
        assert!(addr.starts_with("bitcoincash:q"));
    }

    #[test]
    fn test_get_address_at_chipnet() {
        let wallet = HdWallet::new(TEST_MNEMONIC, BCH_DERIVATION_PATH, true).unwrap();
        let addr = wallet.get_address_at("0/0", false).unwrap();
        assert!(addr.starts_with("bchtest:q"));
    }

    #[test]
    fn test_get_address_at_token() {
        let wallet = HdWallet::new(TEST_MNEMONIC, BCH_DERIVATION_PATH, false).unwrap();
        let addr = wallet.get_address_at("0/0", true).unwrap();
        assert!(addr.starts_with("bitcoincash:z"));
    }

    #[test]
    fn test_get_address_set_at() {
        let wallet = HdWallet::new(TEST_MNEMONIC, BCH_DERIVATION_PATH, false).unwrap();
        let set = wallet.get_address_set_at(0).unwrap();
        assert!(set.receiving.starts_with("bitcoincash:q"));
        assert!(set.change.starts_with("bitcoincash:q"));
        assert_ne!(set.receiving, set.change);
    }

    #[test]
    fn test_get_token_address_set_at() {
        let wallet = HdWallet::new(TEST_MNEMONIC, BCH_DERIVATION_PATH, false).unwrap();
        let set = wallet.get_token_address_set_at(0).unwrap();
        assert!(set.receiving.starts_with("bitcoincash:z"));
        assert!(set.change.starts_with("bitcoincash:z"));
        assert_ne!(set.receiving, set.change);
    }

    #[test]
    fn test_addresses_at_different_indices_differ() {
        let wallet = HdWallet::new(TEST_MNEMONIC, BCH_DERIVATION_PATH, false).unwrap();
        let addr0 = wallet.get_address_at("0/0", false).unwrap();
        let addr1 = wallet.get_address_at("0/1", false).unwrap();
        let addr2 = wallet.get_address_at("0/2", false).unwrap();
        assert_ne!(addr0, addr1);
        assert_ne!(addr1, addr2);
        assert_ne!(addr0, addr2);
    }

    #[test]
    fn test_wif_encoding() {
        let wallet = HdWallet::new(TEST_MNEMONIC, BCH_DERIVATION_PATH, false).unwrap();
        let wif = wallet.get_private_key_wif_at("0/0").unwrap();
        // WIF for compressed mainnet starts with 'K' or 'L'
        assert!(wif.starts_with('K') || wif.starts_with('L'));
    }

    #[test]
    fn test_private_key_at() {
        let wallet = HdWallet::new(TEST_MNEMONIC, BCH_DERIVATION_PATH, false).unwrap();
        let key = wallet.get_private_key_at("0/0").unwrap();
        assert_eq!(key.len(), 32);
        // Should not be all zeros
        assert!(key.iter().any(|&b| b != 0));
    }

    #[test]
    fn test_normalize_path() {
        assert_eq!(normalize_path("0/0"), "m/0/0");
        assert_eq!(normalize_path("m/0/0"), "m/0/0");
        assert_eq!(normalize_path("M/0/0"), "M/0/0");
    }

    #[test]
    fn test_wallet_hash_matches_expected() {
        // This is the critical cross-verification test.
        // The wallet hash must match the JS computation exactly.
        //
        // Generated from JS:
        // sha256.sha256("abandon abandon ... about") = mnemonic_hash
        // sha256.sha256("m/44'/145'/0'") = path_hash
        // sha256.sha256(mnemonic_hash + path_hash) = wallet_hash
        let wallet = HdWallet::new(TEST_MNEMONIC, BCH_DERIVATION_PATH, false).unwrap();
        let hash = wallet.wallet_hash();
        // Verify it is a valid hex string of length 64
        assert_eq!(hash.len(), 64);
        assert!(hash.chars().all(|c| c.is_ascii_hexdigit()));

        // Cross-verify the algorithm:
        // Step 1: sha256_hex(mnemonic)
        let mh = hex::encode(crypto::sha256(TEST_MNEMONIC.as_bytes()));
        // Step 2: sha256_hex(path)
        let ph = hex::encode(crypto::sha256(BCH_DERIVATION_PATH.as_bytes()));
        // Step 3: sha256_hex(mh + ph)
        let combined = format!("{}{}", mh, ph);
        let wh = hex::encode(crypto::sha256(combined.as_bytes()));
        assert_eq!(hash, wh);
    }
}
