/// Network constants and URL helpers for Bitcoin Cash.
/// BIP44 derivation path for BCH (coin type 145)
pub const BCH_DERIVATION_PATH: &str = "m/44'/145'/0'";

/// Mainnet Cash REST API base URL.
/// The same server handles both mainnet and chipnet — the network is
/// determined by the wallet ID format (watch:mainnet: vs watch:testnet:).
pub fn mainnet_api_url(_chipnet: bool) -> &'static str {
    "https://rest-unstable.mainnet.cash"
}

/// Construct a watch-only wallet ID for read operations (no keys exposed).
pub fn watch_wallet_id(chipnet: bool, cashaddr: &str) -> String {
    let network = if chipnet { "testnet" } else { "mainnet" };
    format!("watch:{}:{}", network, cashaddr)
}

/// Create a mainnet API configuration with a request timeout.
pub fn mainnet_config(chipnet: bool) -> mainnet::apis::configuration::Configuration {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .unwrap_or_default();
    mainnet::apis::configuration::Configuration {
        base_path: mainnet_api_url(chipnet).to_string(),
        client,
        ..Default::default()
    }
}

/// Block explorer transaction URL.
pub fn explorer_url(chipnet: bool) -> &'static str {
    if chipnet {
        "https://chipnet.chaingraph.cash/tx/"
    } else {
        "https://bchexplorer.info/tx/"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_derivation_path() {
        assert_eq!(BCH_DERIVATION_PATH, "m/44'/145'/0'");
    }

    #[test]
    fn test_mainnet_api_url_mainnet() {
        assert_eq!(mainnet_api_url(false), "https://rest-unstable.mainnet.cash");
    }

    #[test]
    fn test_mainnet_api_url_chipnet() {
        // Same server handles both — network determined by wallet ID format
        assert_eq!(mainnet_api_url(true), "https://rest-unstable.mainnet.cash");
    }

    #[test]
    fn test_watch_wallet_id_mainnet() {
        let id = watch_wallet_id(false, "bitcoincash:qtest");
        assert_eq!(id, "watch:mainnet:bitcoincash:qtest");
    }

    #[test]
    fn test_watch_wallet_id_chipnet() {
        let id = watch_wallet_id(true, "bchtest:qtest");
        assert_eq!(id, "watch:testnet:bchtest:qtest");
    }

    #[test]
    fn test_mainnet_config_base_path() {
        let cfg = mainnet_config(false);
        assert_eq!(cfg.base_path, "https://rest-unstable.mainnet.cash");
    }

    #[test]
    fn test_explorer_url_mainnet() {
        assert!(explorer_url(false).contains("bchexplorer"));
    }

    #[test]
    fn test_explorer_url_chipnet() {
        assert!(explorer_url(true).contains("chipnet"));
    }
}
