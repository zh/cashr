/// Server configuration loaded from ~/.cashr/servers.toml.
///
/// If the file is missing or unparseable, built-in defaults are used.
use serde::Deserialize;

use crate::storage;

/// Default mainnet-cash REST API server.
const DEFAULT_REST_SERVER: &str = "https://rest-unstable.mainnet.cash";

#[derive(Deserialize, Default)]
pub struct ServersConfig {
    pub electrumx: Option<ElectrumxServers>,
    pub rest: Option<RestServers>,
}

#[derive(Deserialize, Default)]
pub struct ElectrumxServers {
    pub mainnet: Option<Vec<String>>,
    pub chipnet: Option<Vec<String>>,
}

#[derive(Deserialize, Default)]
pub struct RestServers {
    pub servers: Option<Vec<String>>,
}

/// Load the full config from ~/.cashr/servers.toml (silently returns default on error).
fn load_config() -> ServersConfig {
    let path = match storage::base_dir() {
        Ok(dir) => dir.join("servers.toml"),
        Err(_) => return ServersConfig::default(),
    };
    let content = match std::fs::read_to_string(&path) {
        Ok(c) => c,
        Err(_) => return ServersConfig::default(),
    };
    match toml::from_str(&content) {
        Ok(cfg) => cfg,
        Err(e) => {
            eprintln!("Warning: failed to parse {}: {}", path.display(), e);
            ServersConfig::default()
        }
    }
}

/// Get electrumx server list for the given network.
/// Returns empty vec if no servers configured (caller should fall back to REST-only).
pub fn load_electrumx_servers(chipnet: bool) -> Vec<String> {
    let cfg = load_config();
    let servers = cfg.electrumx.and_then(|e| {
        if chipnet {
            e.chipnet
        } else {
            e.mainnet
        }
    });
    servers.unwrap_or_default()
}

/// Get mainnet-cash REST API server list.
/// Falls back to the built-in default if none configured.
pub fn load_rest_servers() -> Vec<String> {
    let cfg = load_config();
    let servers = cfg.rest.and_then(|r| r.servers);
    match servers {
        Some(s) if !s.is_empty() => s,
        _ => vec![DEFAULT_REST_SERVER.to_string()],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_rest_servers() {
        // Without a servers.toml file, should return the default
        let servers = load_rest_servers();
        assert!(!servers.is_empty());
        assert!(servers[0].contains("mainnet.cash"));
    }

    #[test]
    fn default_electrumx_servers_empty() {
        // Without a servers.toml file, electrumx list should be empty
        let servers = load_electrumx_servers(false);
        assert!(servers.is_empty());
    }

    #[test]
    fn parse_full_config() {
        let toml_str = r#"
[electrumx]
mainnet = ["http://localhost:3001", "http://backup:3001"]
chipnet = ["http://localhost:3002"]

[rest]
servers = ["https://custom.mainnet.cash"]
"#;
        let cfg: ServersConfig = toml::from_str(toml_str).unwrap();
        let mainnet = cfg.electrumx.as_ref().unwrap().mainnet.as_ref().unwrap();
        assert_eq!(mainnet.len(), 2);
        assert_eq!(mainnet[0], "http://localhost:3001");

        let chipnet = cfg.electrumx.as_ref().unwrap().chipnet.as_ref().unwrap();
        assert_eq!(chipnet.len(), 1);

        let rest = cfg.rest.as_ref().unwrap().servers.as_ref().unwrap();
        assert_eq!(rest[0], "https://custom.mainnet.cash");
    }

    #[test]
    fn parse_partial_config() {
        let toml_str = r#"
[electrumx]
mainnet = ["http://localhost:3001"]
"#;
        let cfg: ServersConfig = toml::from_str(toml_str).unwrap();
        assert!(cfg.electrumx.unwrap().chipnet.is_none());
        assert!(cfg.rest.is_none());
    }
}
