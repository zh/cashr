/// Wallet management: mnemonic generation, import, load.
pub mod keys;
pub mod bch;

use anyhow::{bail, Result};
use getrandom::getrandom;

use crate::network::BCH_DERIVATION_PATH;
use crate::storage;
use keys::{compute_wallet_hash, HdWallet};
use bch::BchWallet;

/// Information about a wallet (returned from generate/import/load).
pub struct WalletInfo {
    pub name: String,
    pub mnemonic: String,
    pub wallet_hash: String,
}

impl std::fmt::Debug for WalletInfo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WalletInfo")
            .field("name", &self.name)
            .field("mnemonic", &"[REDACTED]")
            .field("wallet_hash", &self.wallet_hash)
            .finish()
    }
}

/// High-level Wallet struct that provides access to BCH wallets.
pub struct Wallet {
    pub name: String,
    mnemonic: String,
    wallet_hash: String,
}

impl Wallet {
    /// Get a BchWallet for the given network.
    pub fn for_network(&self, chipnet: bool) -> Result<BchWallet> {
        BchWallet::new(&self.mnemonic, BCH_DERIVATION_PATH, chipnet)
    }

    /// Get an HdWallet for the given network.
    pub fn hd_wallet(&self, chipnet: bool) -> Result<HdWallet> {
        HdWallet::new(&self.mnemonic, BCH_DERIVATION_PATH, chipnet)
    }

    pub fn wallet_hash(&self) -> &str {
        &self.wallet_hash
    }
}

/// Generate 16 bytes of cryptographically secure random entropy.
fn rand_entropy() -> Result<[u8; 16]> {
    let mut buf = [0u8; 16];
    getrandom(&mut buf).map_err(|e| anyhow::anyhow!("failed to generate random entropy: {}", e))?;
    Ok(buf)
}

/// Generate a new 12-word BIP39 mnemonic, store under name, set as default.
/// Error if name already exists.
pub fn generate_mnemonic(name: &str) -> Result<WalletInfo> {
    if storage::wallet_exists(name)? {
        bail!("wallet '{}' already exists", name);
    }

    // Generate 128 bits of entropy for a 12-word mnemonic
    let entropy = rand_entropy()?;
    let mnemonic = bip39::Mnemonic::from_entropy(&entropy)
        .map_err(|e| anyhow::anyhow!("failed to generate mnemonic: {}", e))?;
    let mnemonic_str = mnemonic.to_string();
    let wallet_hash = compute_wallet_hash(&mnemonic_str, BCH_DERIVATION_PATH);

    storage::store_mnemonic(&mnemonic_str, name)?;
    storage::set_default_wallet(name)?;

    Ok(WalletInfo {
        name: name.to_string(),
        mnemonic: mnemonic_str,
        wallet_hash,
    })
}

/// Import a mnemonic (trim, lowercase, validate BIP39), store, set as default.
/// Error if name already exists.
pub fn import_mnemonic(name: &str, mnemonic: &str) -> Result<WalletInfo> {
    if storage::wallet_exists(name)? {
        bail!("wallet '{}' already exists", name);
    }

    let trimmed = mnemonic.trim().to_lowercase();

    // Validate by parsing
    let _parsed: bip39::Mnemonic = trimmed
        .parse()
        .map_err(|_| anyhow::anyhow!("invalid BIP39 mnemonic phrase"))?;

    let wallet_hash = compute_wallet_hash(&trimmed, BCH_DERIVATION_PATH);

    storage::store_mnemonic(&trimmed, name)?;
    storage::set_default_wallet(name)?;

    Ok(WalletInfo {
        name: name.to_string(),
        mnemonic: trimmed,
        wallet_hash,
    })
}

/// Load mnemonic by name (explicit or default).
pub fn load_mnemonic(name: Option<&str>) -> Result<WalletInfo> {
    let resolved = storage::resolve_wallet_name(name)?;
    let mnemonic = storage::get_mnemonic(&resolved)?
        .ok_or_else(|| anyhow::anyhow!("wallet '{}' exists but mnemonic file is empty", resolved))?;
    let wallet_hash = compute_wallet_hash(&mnemonic, BCH_DERIVATION_PATH);

    Ok(WalletInfo {
        name: resolved,
        mnemonic,
        wallet_hash,
    })
}

/// Load a Wallet by name (explicit or default).
pub fn load_wallet(name: Option<&str>) -> Result<Wallet> {
    let info = load_mnemonic(name)?;
    Ok(Wallet {
        name: info.name,
        mnemonic: info.mnemonic,
        wallet_hash: info.wallet_hash,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup_temp_home() -> tempfile::TempDir {
        let tmp = tempfile::tempdir().unwrap();
        storage::set_base_dir_override(Some(tmp.path().to_path_buf()));
        tmp
    }

    #[test]
    fn test_generate_mnemonic_creates_valid_wallet() {
        let _tmp = setup_temp_home();
        let info = generate_mnemonic("test").unwrap();

        // Should be 12 words
        let words: Vec<&str> = info.mnemonic.split_whitespace().collect();
        assert_eq!(words.len(), 12);

        // Should be stored and set as default
        assert!(storage::wallet_exists("test").unwrap());
        assert_eq!(
            storage::get_default_wallet().unwrap(),
            Some("test".to_string())
        );
    }

    #[test]
    fn test_generate_mnemonic_duplicate_name_fails() {
        let _tmp = setup_temp_home();
        generate_mnemonic("dup").unwrap();
        let result = generate_mnemonic("dup");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("already exists"));
    }

    #[test]
    fn test_import_mnemonic_valid() {
        let _tmp = setup_temp_home();
        let mnemonic =
            "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";
        let info = import_mnemonic("imported", mnemonic).unwrap();
        assert_eq!(info.mnemonic, mnemonic);
        assert!(storage::wallet_exists("imported").unwrap());
    }

    #[test]
    fn test_import_mnemonic_trims_and_lowercases() {
        let _tmp = setup_temp_home();
        let info = import_mnemonic(
            "trimmed",
            "  Abandon Abandon Abandon Abandon Abandon Abandon Abandon Abandon Abandon Abandon Abandon About  ",
        )
        .unwrap();
        assert_eq!(
            info.mnemonic,
            "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about"
        );
    }

    #[test]
    fn test_import_mnemonic_invalid_phrase() {
        let _tmp = setup_temp_home();
        let result = import_mnemonic("bad", "not a valid mnemonic phrase");
        assert!(result.is_err());
    }

    #[test]
    fn test_load_mnemonic_explicit_name() {
        let _tmp = setup_temp_home();
        let generated = generate_mnemonic("loadme").unwrap();
        let loaded = load_mnemonic(Some("loadme")).unwrap();
        assert_eq!(generated.mnemonic, loaded.mnemonic);
        assert_eq!(loaded.name, "loadme");
    }

    #[test]
    fn test_load_mnemonic_default() {
        let _tmp = setup_temp_home();
        let generated = generate_mnemonic("default-test").unwrap();
        let loaded = load_mnemonic(None).unwrap();
        assert_eq!(generated.mnemonic, loaded.mnemonic);
    }

    #[test]
    fn test_load_mnemonic_no_default_error() {
        let _tmp = setup_temp_home();
        let result = load_mnemonic(None);
        assert!(result.is_err());
    }

    #[test]
    fn test_load_mnemonic_nonexistent_name() {
        let _tmp = setup_temp_home();
        let result = load_mnemonic(Some("ghost"));
        assert!(result.is_err());
    }

    #[test]
    fn test_load_wallet_derives_addresses() {
        let _tmp = setup_temp_home();
        import_mnemonic(
            "derive-test",
            "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about",
        )
        .unwrap();
        let wallet = load_wallet(Some("derive-test")).unwrap();
        let hd = wallet.hd_wallet(false).unwrap();
        let addr = hd.get_address_at("0/0", false).unwrap();
        assert!(addr.starts_with("bitcoincash:q"));
    }

    #[test]
    fn test_two_generates_different_mnemonics() {
        let _tmp = setup_temp_home();
        let info1 = generate_mnemonic("first").unwrap();
        let info2 = generate_mnemonic("second").unwrap();
        assert_ne!(info1.mnemonic, info2.mnemonic);
    }

    #[test]
    fn test_wallet_hash_matches_js_computation() {
        let _tmp = setup_temp_home();
        let mnemonic =
            "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";
        let info = import_mnemonic("hash-test", mnemonic).unwrap();

        // Verify via manual computation
        let expected = compute_wallet_hash(mnemonic, BCH_DERIVATION_PATH);
        assert_eq!(info.wallet_hash, expected);
    }

    #[test]
    fn test_multi_wallet_create_a_create_b_load_a() {
        let _tmp = setup_temp_home();
        let info_a = generate_mnemonic("a").unwrap();
        let _info_b = generate_mnemonic("b").unwrap();
        let loaded = load_mnemonic(Some("a")).unwrap();
        assert_eq!(loaded.mnemonic, info_a.mnemonic);
    }
}
