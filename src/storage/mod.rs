/// Filesystem wallet storage at ~/.cashr/.
///
/// Storage layout:
///   ~/.cashr/
///   ├── default                 # Contains name of default wallet
///   └── wallets/
///       ├── savings             # Mnemonic for "savings" wallet
///       └── trading             # Mnemonic for "trading" wallet
use anyhow::{Context, Result};
use std::cell::RefCell;
use std::path::PathBuf;

#[derive(thiserror::Error, Debug)]
pub enum StorageError {
    #[error("invalid wallet name '{name}': must be alphanumeric, hyphens, or underscores (max 64 chars)")]
    InvalidWalletName { name: String },
    #[error("wallet '{name}' already exists")]
    WalletExists { name: String },
    #[error("wallet '{name}' not found")]
    WalletNotFound { name: String },
    #[error("no default wallet set -- use --name or create a wallet first")]
    NoDefaultWallet,
}

thread_local! {
    /// Thread-local override for base directory (used in tests).
    static BASE_DIR_OVERRIDE: RefCell<Option<PathBuf>> = const { RefCell::new(None) };
}

/// Set a base directory override for the current thread (for testing).
#[cfg(test)]
pub fn set_base_dir_override(path: Option<PathBuf>) {
    BASE_DIR_OVERRIDE.with(|cell| {
        *cell.borrow_mut() = path;
    });
}

/// Get base directory: thread-local override > CASHR_HOME env var > ~/.cashr.
pub(crate) fn base_dir() -> Result<PathBuf> {
    let override_path = BASE_DIR_OVERRIDE.with(|cell| cell.borrow().clone());
    if let Some(path) = override_path {
        return Ok(path);
    }
    if let Ok(override_dir) = std::env::var("CASHR_HOME") {
        return Ok(PathBuf::from(override_dir));
    }
    dirs::home_dir()
        .map(|h| h.join(".cashr"))
        .ok_or_else(|| anyhow::anyhow!("cannot determine home directory"))
}

/// Get or create the cashr directory.
fn cashr_dir() -> Result<PathBuf> {
    let dir = base_dir()?;
    if !dir.exists() {
        std::fs::create_dir_all(&dir).context("failed to create cashr directory")?;
    }
    Ok(dir)
}

/// Get or create the wallets subdirectory.
fn wallets_dir() -> Result<PathBuf> {
    let dir = cashr_dir()?.join("wallets");
    if !dir.exists() {
        std::fs::create_dir_all(&dir).context("failed to create wallets directory")?;
    }
    Ok(dir)
}

/// Validate wallet name: alphanumeric + hyphens + underscores, max 64 chars.
fn validate_wallet_name(name: &str) -> Result<()> {
    if name.is_empty() || name.len() > 64 {
        return Err(StorageError::InvalidWalletName {
            name: name.to_string(),
        }
        .into());
    }
    let valid = name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    if !valid {
        return Err(StorageError::InvalidWalletName {
            name: name.to_string(),
        }
        .into());
    }
    Ok(())
}

/// Store a mnemonic under a wallet name.
/// File permissions are set to 0o600 on Unix.
/// Returns an error if the wallet already exists.
pub fn store_mnemonic(mnemonic: &str, name: &str) -> Result<()> {
    validate_wallet_name(name)?;
    let path = wallets_dir()?.join(name);
    if path.exists() {
        return Err(StorageError::WalletExists {
            name: name.to_string(),
        }
        .into());
    }

    let content = format!("{}\n", mnemonic.trim());
    std::fs::write(&path, content).context("failed to write mnemonic file")?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = std::fs::Permissions::from_mode(0o600);
        std::fs::set_permissions(&path, perms).context("failed to set file permissions")?;
    }

    Ok(())
}

/// Get mnemonic by wallet name. Returns Ok(None) if not found.
pub fn get_mnemonic(name: &str) -> Result<Option<String>> {
    validate_wallet_name(name)?;
    let path = wallets_dir()?.join(name);
    match std::fs::read_to_string(&path) {
        Ok(content) => Ok(Some(content.trim().to_string())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(anyhow::Error::new(e).context("failed to read mnemonic file")),
    }
}

/// Store the network for a wallet (mainnet or chipnet).
pub fn store_network(name: &str, chipnet: bool) -> Result<()> {
    let path = wallets_dir()?.join(format!("{}.net", name));
    let network = if chipnet { "chipnet" } else { "mainnet" };
    std::fs::write(&path, network).context("failed to write network file")?;
    Ok(())
}

/// Get the network for a wallet. Returns None if not set (legacy wallets default to mainnet).
pub fn get_network(name: &str) -> Result<Option<bool>> {
    let path = wallets_dir()?.join(format!("{}.net", name));
    match std::fs::read_to_string(&path) {
        Ok(content) => Ok(Some(content.trim() == "chipnet")),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(anyhow::Error::new(e).context("failed to read network file")),
    }
}

/// Resolve the network (chipnet or mainnet) for a wallet.
/// Uses the stored .net file if available, otherwise defaults to mainnet.
pub fn resolve_chipnet(wallet_name: Option<&str>) -> bool {
    let name = wallet_name
        .map(|n| n.to_string())
        .or_else(|| get_default_wallet().ok().flatten())
        .unwrap_or_default();
    if name.is_empty() {
        return false; // default to mainnet
    }
    get_network(&name).unwrap_or(None).unwrap_or(false)
}

/// Delete a wallet file and its network metadata. Clears default if this was the default wallet.
pub fn delete_wallet(name: &str) -> Result<()> {
    validate_wallet_name(name)?;
    let dir = wallets_dir()?;

    // Delete mnemonic file
    let path = dir.join(name);
    match std::fs::remove_file(&path) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(anyhow::Error::new(e).context("failed to delete wallet file")),
    }

    // Delete network sidecar file
    let net_path = dir.join(format!("{}.net", name));
    let _ = std::fs::remove_file(net_path);

    // Clear default if this was the default wallet
    if let Ok(Some(default_name)) = get_default_wallet() {
        if default_name == name {
            clear_default_wallet()?;
        }
    }

    Ok(())
}

/// Set the default wallet name.
pub fn set_default_wallet(name: &str) -> Result<()> {
    validate_wallet_name(name)?;
    let path = cashr_dir()?.join("default");
    let content = format!("{}\n", name);
    std::fs::write(&path, content).context("failed to write default wallet file")?;
    Ok(())
}

/// Get the default wallet name. Returns Ok(None) if no default is set.
pub fn get_default_wallet() -> Result<Option<String>> {
    let path = cashr_dir()?.join("default");
    match std::fs::read_to_string(&path) {
        Ok(content) => {
            let trimmed = content.trim().to_string();
            if trimmed.is_empty() {
                Ok(None)
            } else {
                Ok(Some(trimmed))
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(anyhow::Error::new(e).context("failed to read default wallet file")),
    }
}

/// Remove the default file.
pub fn clear_default_wallet() -> Result<()> {
    let path = cashr_dir()?.join("default");
    match std::fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(anyhow::Error::new(e).context("failed to clear default wallet")),
    }
}

/// List all wallet names (filenames in wallets/).
pub fn list_wallets() -> Result<Vec<String>> {
    let dir = wallets_dir()?;
    let mut names = Vec::new();
    for entry in std::fs::read_dir(&dir).context("failed to read wallets directory")? {
        let entry = entry?;
        if entry.file_type()?.is_file() {
            if let Some(name) = entry.file_name().to_str() {
                // Skip metadata sidecar files
                if name.ends_with(".net") {
                    continue;
                }
                names.push(name.to_string());
            }
        }
    }
    names.sort();
    Ok(names)
}

/// Check if a wallet exists.
pub fn wallet_exists(name: &str) -> Result<bool> {
    validate_wallet_name(name)?;
    let path = wallets_dir()?.join(name);
    Ok(path.exists())
}

/// Resolve wallet name: explicit name > default > error.
pub fn resolve_wallet_name(name: Option<&str>) -> Result<String> {
    if let Some(n) = name {
        validate_wallet_name(n)?;
        if !wallet_exists(n)? {
            return Err(StorageError::WalletNotFound {
                name: n.to_string(),
            }
            .into());
        }
        return Ok(n.to_string());
    }

    match get_default_wallet()? {
        Some(default_name) => {
            if !wallet_exists(&default_name)? {
                return Err(StorageError::WalletNotFound {
                    name: default_name,
                }
                .into());
            }
            Ok(default_name)
        }
        None => Err(StorageError::NoDefaultWallet.into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Set up a temp directory and configure the base dir override.
    /// Returns the TempDir guard (must be kept alive for the test).
    fn setup_temp_home() -> tempfile::TempDir {
        let tmp = tempfile::tempdir().unwrap();
        set_base_dir_override(Some(tmp.path().to_path_buf()));
        tmp
    }

    #[test]
    fn test_validate_name_valid() {
        assert!(validate_wallet_name("my-wallet").is_ok());
        assert!(validate_wallet_name("test_1").is_ok());
        assert!(validate_wallet_name("ABC123").is_ok());
        assert!(validate_wallet_name("a").is_ok());
    }

    #[test]
    fn test_validate_name_invalid() {
        assert!(validate_wallet_name("../etc").is_err());
        assert!(validate_wallet_name("").is_err());
        assert!(validate_wallet_name("a b").is_err());
        assert!(validate_wallet_name("a/b").is_err());
        assert!(validate_wallet_name("a.b").is_err());
        assert!(validate_wallet_name(&"x".repeat(65)).is_err());
    }

    #[test]
    fn test_store_and_get_mnemonic() {
        let _tmp = setup_temp_home();
        store_mnemonic("test mnemonic phrase", "testwallet").unwrap();
        let result = get_mnemonic("testwallet").unwrap();
        assert_eq!(result, Some("test mnemonic phrase".to_string()));
    }

    #[test]
    fn test_get_mnemonic_not_found() {
        let _tmp = setup_temp_home();
        let result = get_mnemonic("nonexistent").unwrap();
        assert_eq!(result, None);
    }

    #[test]
    fn test_store_mnemonic_already_exists() {
        let _tmp = setup_temp_home();
        store_mnemonic("first", "dupwallet").unwrap();
        let result = store_mnemonic("second", "dupwallet");
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("already exists"));
        // Verify original mnemonic is unchanged
        let mnemonic = get_mnemonic("dupwallet").unwrap();
        assert_eq!(mnemonic, Some("first".to_string()));
    }

    #[test]
    fn test_delete_wallet() {
        let _tmp = setup_temp_home();
        store_mnemonic("test", "todelete").unwrap();
        assert!(wallet_exists("todelete").unwrap());
        delete_wallet("todelete").unwrap();
        assert!(!wallet_exists("todelete").unwrap());
    }

    #[test]
    fn test_delete_wallet_not_found() {
        let _tmp = setup_temp_home();
        let result = delete_wallet("doesnotexist");
        assert!(result.is_ok());
    }

    #[test]
    fn test_delete_wallet_clears_default() {
        let _tmp = setup_temp_home();
        store_mnemonic("test", "mywallet").unwrap();
        set_default_wallet("mywallet").unwrap();
        assert_eq!(get_default_wallet().unwrap(), Some("mywallet".to_string()));
        delete_wallet("mywallet").unwrap();
        assert_eq!(get_default_wallet().unwrap(), None);
    }

    #[test]
    fn test_set_and_get_default() {
        let _tmp = setup_temp_home();
        store_mnemonic("test", "default-test").unwrap();
        set_default_wallet("default-test").unwrap();
        let result = get_default_wallet().unwrap();
        assert_eq!(result, Some("default-test".to_string()));
    }

    #[test]
    fn test_get_default_not_set() {
        let _tmp = setup_temp_home();
        let result = get_default_wallet().unwrap();
        assert_eq!(result, None);
    }

    #[test]
    fn test_clear_default() {
        let _tmp = setup_temp_home();
        store_mnemonic("test", "clearme").unwrap();
        set_default_wallet("clearme").unwrap();
        clear_default_wallet().unwrap();
        assert_eq!(get_default_wallet().unwrap(), None);
    }

    #[test]
    fn test_list_wallets() {
        let _tmp = setup_temp_home();
        store_mnemonic("a", "alpha").unwrap();
        store_mnemonic("b", "beta").unwrap();
        let wallets = list_wallets().unwrap();
        assert_eq!(wallets, vec!["alpha".to_string(), "beta".to_string()]);
    }

    #[test]
    fn test_list_wallets_empty() {
        let _tmp = setup_temp_home();
        let wallets = list_wallets().unwrap();
        assert!(wallets.is_empty());
    }

    #[test]
    fn test_wallet_exists() {
        let _tmp = setup_temp_home();
        store_mnemonic("test", "exists-test").unwrap();
        assert!(wallet_exists("exists-test").unwrap());
        assert!(!wallet_exists("does-not-exist").unwrap());
    }

    #[test]
    fn test_resolve_wallet_name_explicit() {
        let _tmp = setup_temp_home();
        store_mnemonic("test", "explicit").unwrap();
        let name = resolve_wallet_name(Some("explicit")).unwrap();
        assert_eq!(name, "explicit");
    }

    #[test]
    fn test_resolve_wallet_name_default() {
        let _tmp = setup_temp_home();
        store_mnemonic("test", "default-w").unwrap();
        set_default_wallet("default-w").unwrap();
        let name = resolve_wallet_name(None).unwrap();
        assert_eq!(name, "default-w");
    }

    #[test]
    fn test_resolve_wallet_name_no_default() {
        let _tmp = setup_temp_home();
        let result = resolve_wallet_name(None);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("no default wallet"));
    }

    #[cfg(unix)]
    #[test]
    fn test_file_permissions_unix() {
        use std::os::unix::fs::PermissionsExt;

        let _tmp = setup_temp_home();
        store_mnemonic("secret mnemonic", "perms-test").unwrap();
        let path = wallets_dir().unwrap().join("perms-test");
        let metadata = std::fs::metadata(&path).unwrap();
        let mode = metadata.permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }
}
