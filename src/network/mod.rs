/// Network constants and URL helpers for Bitcoin Cash.

/// BIP44 derivation path for BCH (coin type 145)
pub const BCH_DERIVATION_PATH: &str = "m/44'/145'/0'";

/// Watchtower project IDs from environment variables.
pub struct ProjectId {
    pub mainnet: String,
    pub chipnet: String,
}

/// Default Paytaca project IDs on Watchtower (public, from watchtower.cash/api/projects/).
const DEFAULT_MAINNET_PROJECT_ID: &str = "5348e8fd-c001-47c7-b97c-807f545cf44e";
const DEFAULT_CHIPNET_PROJECT_ID: &str = "5348e8fd-c001-47c7-b97c-807f545cf44e";

/// Read Watchtower project IDs from environment, falling back to Paytaca defaults.
pub fn project_id() -> ProjectId {
    ProjectId {
        mainnet: std::env::var("WATCHTOWER_PROJECT_ID")
            .unwrap_or_else(|_| DEFAULT_MAINNET_PROJECT_ID.to_string()),
        chipnet: std::env::var("WATCHTOWER_CHIP_PROJECT_ID")
            .unwrap_or_else(|_| DEFAULT_CHIPNET_PROJECT_ID.to_string()),
    }
}

/// Watchtower REST API base URL.
pub fn watchtower_api_url(chipnet: bool) -> &'static str {
    if chipnet {
        "https://chipnet.watchtower.cash/api"
    } else {
        "https://watchtower.cash/api"
    }
}

/// Watchtower WebSocket URL.
pub fn watchtower_ws_url(chipnet: bool) -> &'static str {
    if chipnet {
        "wss://chipnet.watchtower.cash/ws"
    } else {
        "wss://watchtower.cash/ws"
    }
}

/// CashAddress prefix for the given network.
pub fn address_prefix(chipnet: bool) -> &'static str {
    if chipnet {
        "bchtest"
    } else {
        "bitcoincash"
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
    fn test_watchtower_api_url_mainnet() {
        assert_eq!(watchtower_api_url(false), "https://watchtower.cash/api");
    }

    #[test]
    fn test_watchtower_api_url_chipnet() {
        assert_eq!(
            watchtower_api_url(true),
            "https://chipnet.watchtower.cash/api"
        );
    }

    #[test]
    fn test_address_prefix_mainnet() {
        assert_eq!(address_prefix(false), "bitcoincash");
    }

    #[test]
    fn test_address_prefix_chipnet() {
        assert_eq!(address_prefix(true), "bchtest");
    }

    #[test]
    fn test_explorer_url_mainnet() {
        assert!(explorer_url(false).contains("bchexplorer"));
    }

    #[test]
    fn test_explorer_url_chipnet() {
        assert!(explorer_url(true).contains("chipnet"));
    }

    #[test]
    fn test_watchtower_ws_url() {
        assert!(watchtower_ws_url(false).starts_with("wss://"));
        assert!(watchtower_ws_url(true).starts_with("wss://"));
    }
}
