/// Network constants, URL helpers, and server failover for Bitcoin Cash.
use anyhow::{bail, Result};

use crate::config;
use crate::electrumx::ElectrumxClient;

/// BIP44 derivation path for BCH (coin type 145)
pub const BCH_DERIVATION_PATH: &str = "m/44'/145'/0'";

/// Construct a watch-only wallet ID for read operations (no keys exposed).
pub fn watch_wallet_id(chipnet: bool, cashaddr: &str) -> String {
    let network = if chipnet { "testnet" } else { "mainnet" };
    format!("watch:{}:{}", network, cashaddr)
}

/// Build a mainnet-cash REST API configuration for the given base URL.
fn build_rest_config(base_url: &str) -> mainnet::apis::configuration::Configuration {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .unwrap_or_default();
    mainnet::apis::configuration::Configuration {
        base_path: base_url.to_string(),
        client,
        ..Default::default()
    }
}

/// Create a mainnet API configuration pointing at the default server (for tests).
#[cfg(test)]
pub fn mainnet_config(_chipnet: bool) -> mainnet::apis::configuration::Configuration {
    build_rest_config("https://rest-unstable.mainnet.cash")
}

/// Probe a REST server by hitting its base URL with a short timeout.
/// Any HTTP response (even 404) means the server is reachable.
async fn probe_url(url: &str) -> bool {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        .build()
        .unwrap_or_default();
    client.get(url).send().await.is_ok()
}

/// Connect to an electrumx (fulcrum-rust) server with failover.
///
/// Iterates the configured server list, probes each one, returns the first
/// working ElectrumxClient. Returns None if no electrumx servers are configured
/// (the caller should fall back to REST-only mode).
pub async fn connect_electrumx(chipnet: bool) -> Result<Option<ElectrumxClient>> {
    let servers = config::load_electrumx_servers(chipnet);
    if servers.is_empty() {
        return Ok(None);
    }

    for server_url in &servers {
        eprintln!("electrumx: trying {}...", server_url);
        let client = ElectrumxClient::new(server_url);
        if client.probe().await {
            eprintln!("electrumx: connected to {}", server_url);
            return Ok(Some(client));
        }
        eprintln!("electrumx: {} unreachable", server_url);
    }

    bail!(
        "All electrumx servers unreachable. Check your connection or ~/.cashr/servers.toml"
    )
}

/// Connect to a mainnet-cash REST API server with failover.
///
/// Iterates the configured server list, probes each one, returns a Configuration
/// for the first working server.
pub async fn connect_rest(_chipnet: bool) -> Result<mainnet::apis::configuration::Configuration> {
    let servers = config::load_rest_servers();

    for server_url in &servers {
        eprintln!("rest: trying {}...", server_url);
        if probe_url(server_url).await {
            eprintln!("rest: connected to {}", server_url);
            return Ok(build_rest_config(server_url));
        }
        eprintln!("rest: {} unreachable", server_url);
    }

    bail!("All REST servers unreachable. Check your connection or ~/.cashr/servers.toml")
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
    fn test_build_rest_config() {
        let cfg = build_rest_config("http://localhost:3000");
        assert_eq!(cfg.base_path, "http://localhost:3000");
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
