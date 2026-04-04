/// x402-bch v2.2 protocol logic: parsing, selection, payload building.
use super::types::*;

/// Parse a 402 response body into PaymentRequired.
/// Returns None if body is not a valid x402 v2 response.
pub fn parse_payment_required(body: &serde_json::Value) -> Option<PaymentRequired> {
    let obj = body.as_object()?;

    // Must be x402Version 2
    let version = obj.get("x402Version")?.as_u64()?;
    if version != 2 {
        return None;
    }

    let error = obj
        .get("error")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    let resource = match obj.get("resource") {
        Some(r) if r.is_object() => ResourceInfo {
            url: r
                .get("url")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
            description: r.get("description").and_then(|v| v.as_str()).map(|s| s.to_string()),
            mime_type: r.get("mimeType").and_then(|v| v.as_str()).map(|s| s.to_string()),
        },
        _ => ResourceInfo {
            url: String::new(),
            description: None,
            mime_type: None,
        },
    };

    let extensions = obj
        .get("extensions")
        .cloned()
        .unwrap_or(serde_json::json!({}));

    let mut accepts = Vec::new();
    if let Some(accepts_arr) = obj.get("accepts").and_then(|v| v.as_array()) {
        for accept in accepts_arr {
            let scheme = accept.get("scheme").and_then(|v| v.as_str());
            let network = accept.get("network").and_then(|v| v.as_str());
            let pay_to = accept.get("payTo").and_then(|v| v.as_str());

            // Only include entries that have all three required fields
            if let (Some(scheme), Some(network), Some(pay_to)) = (scheme, network, pay_to) {
                accepts.push(PaymentRequirements {
                    scheme: scheme.to_string(),
                    network: network.to_string(),
                    amount: accept
                        .get("amount")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    asset: accept
                        .get("asset")
                        .and_then(|v| v.as_str())
                        .unwrap_or(BCH_ASSET_ID)
                        .to_string(),
                    pay_to: pay_to.to_string(),
                    max_timeout_seconds: accept
                        .get("maxTimeoutSeconds")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(60) as u32,
                    extra: accept
                        .get("extra")
                        .cloned()
                        .unwrap_or(serde_json::json!({})),
                });
            }
        }
    }

    Some(PaymentRequired {
        x402_version: 2,
        error,
        resource,
        accepts,
        extensions,
    })
}

/// Find a BCH payment option matching the client's network.
/// Looks for scheme="utxo" with matching network string.
pub fn select_bch_requirements(
    requirements: &PaymentRequired,
    chipnet: bool,
) -> Option<&PaymentRequirements> {
    let target_network = if chipnet {
        BCH_CHIPNET_NETWORK
    } else {
        BCH_MAINNET_NETWORK
    };
    requirements
        .accepts
        .iter()
        .find(|a| a.scheme == "utxo" && a.network == target_network)
}

/// Build an unsigned PaymentPayload struct.
pub fn build_payment_payload(
    accepted: &PaymentRequirements,
    resource_url: &str,
    payer: &str,
    txid: &str,
    vout: Option<u32>,
    amount: Option<&str>,
) -> PaymentPayload {
    let resource = ResourceInfo {
        url: resource_url.to_string(),
        description: Some(String::new()),
        mime_type: Some("application/json".to_string()),
    };

    PaymentPayload {
        x402_version: 2,
        resource: Some(resource),
        accepted: accepted.clone(),
        payload: Payload {
            signature: String::new(),
            authorization: build_authorization(accepted, payer, txid, vout, amount),
        },
        extensions: serde_json::json!({}),
    }
}

/// Build an Authorization struct.
pub fn build_authorization(
    accepted: &PaymentRequirements,
    payer: &str,
    txid: &str,
    vout: Option<u32>,
    amount: Option<&str>,
) -> Authorization {
    Authorization {
        from: payer.to_string(),
        to: accepted.pay_to.clone(),
        value: accepted.amount.clone(),
        txid: txid.to_string(),
        vout,
        amount: amount.map(|s| s.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // ── parsePaymentRequiredJson tests ───────────────────────────────

    #[test]
    fn test_parse_valid_payment_required() {
        let input = json!({
            "x402Version": 2,
            "error": "Payment required",
            "resource": { "url": "https://api.example.com/data" },
            "accepts": [{
                "scheme": "utxo",
                "network": BCH_MAINNET_NETWORK,
                "amount": "1000",
                "asset": BCH_ASSET_ID,
                "payTo": "bitcoincash:qp2f5j6q3fj5gjwgk8rkq8xrk8q8q8q8q8q8q8q8q",
                "maxTimeoutSeconds": 300,
                "extra": {}
            }],
            "extensions": {}
        });

        let result = parse_payment_required(&input);
        assert!(result.is_some());
        let pr = result.unwrap();
        assert_eq!(pr.x402_version, 2);
        assert_eq!(pr.error, Some("Payment required".to_string()));
        assert_eq!(pr.resource.url, "https://api.example.com/data");
        assert_eq!(pr.accepts.len(), 1);
        assert_eq!(pr.accepts[0].scheme, "utxo");
        assert_eq!(pr.accepts[0].amount, "1000");
    }

    #[test]
    fn test_parse_null_input() {
        assert!(parse_payment_required(&json!(null)).is_none());
    }

    #[test]
    fn test_parse_non_object_input() {
        assert!(parse_payment_required(&json!("string")).is_none());
        assert!(parse_payment_required(&json!(123)).is_none());
    }

    #[test]
    fn test_parse_wrong_version() {
        let input = json!({ "x402Version": 1, "accepts": [] });
        assert!(parse_payment_required(&input).is_none());
    }

    #[test]
    fn test_parse_default_values() {
        let input = json!({
            "x402Version": 2,
            "resource": {},
            "accepts": [{
                "scheme": "utxo",
                "network": BCH_MAINNET_NETWORK,
                "payTo": "bitcoincash:qp2f5j6q3fj5gjwgk8rkq8xrk8q8q8q8q8q8q8q8"
            }]
        });

        let result = parse_payment_required(&input).unwrap();
        assert_eq!(result.accepts[0].asset, BCH_ASSET_ID);
        assert_eq!(result.accepts[0].max_timeout_seconds, 60);
    }

    #[test]
    fn test_parse_filters_invalid_accepts() {
        let input = json!({
            "x402Version": 2,
            "resource": { "url": "https://api.example.com" },
            "accepts": [
                { "scheme": "utxo", "network": BCH_MAINNET_NETWORK, "payTo": "valid1" },
                { "scheme": "invalid" },
                { "network": BCH_MAINNET_NETWORK, "payTo": "missing-scheme" },
                { "scheme": "utxo", "payTo": "missing-network" },
                { "scheme": "utxo", "network": BCH_MAINNET_NETWORK }
            ]
        });

        let result = parse_payment_required(&input).unwrap();
        assert_eq!(result.accepts.len(), 1);
        assert_eq!(result.accepts[0].pay_to, "valid1");
    }

    // ── selectBchPaymentRequirements tests ───────────────────────────

    fn make_payment_required() -> PaymentRequired {
        PaymentRequired {
            x402_version: 2,
            error: None,
            resource: ResourceInfo {
                url: "https://api.example.com".to_string(),
                description: None,
                mime_type: None,
            },
            accepts: vec![
                PaymentRequirements {
                    scheme: "utxo".to_string(),
                    network: BCH_MAINNET_NETWORK.to_string(),
                    amount: "1000".to_string(),
                    asset: BCH_ASSET_ID.to_string(),
                    pay_to: "bitcoincash:mainnet-address".to_string(),
                    max_timeout_seconds: 300,
                    extra: json!({}),
                },
                PaymentRequirements {
                    scheme: "utxo".to_string(),
                    network: BCH_CHIPNET_NETWORK.to_string(),
                    amount: "1000".to_string(),
                    asset: BCH_ASSET_ID.to_string(),
                    pay_to: "bitcoincash:chipnet-address".to_string(),
                    max_timeout_seconds: 300,
                    extra: json!({}),
                },
            ],
            extensions: json!({}),
        }
    }

    #[test]
    fn test_select_mainnet() {
        let pr = make_payment_required();
        let result = select_bch_requirements(&pr, false);
        assert!(result.is_some());
        assert_eq!(result.unwrap().network, BCH_MAINNET_NETWORK);
        assert_eq!(result.unwrap().pay_to, "bitcoincash:mainnet-address");
    }

    #[test]
    fn test_select_chipnet() {
        let pr = make_payment_required();
        let result = select_bch_requirements(&pr, true);
        assert!(result.is_some());
        assert_eq!(result.unwrap().network, BCH_CHIPNET_NETWORK);
        assert_eq!(result.unwrap().pay_to, "bitcoincash:chipnet-address");
    }

    #[test]
    fn test_select_no_match() {
        let pr = PaymentRequired {
            x402_version: 2,
            error: None,
            resource: ResourceInfo {
                url: String::new(),
                description: None,
                mime_type: None,
            },
            accepts: vec![PaymentRequirements {
                scheme: "utxo".to_string(),
                network: BCH_MAINNET_NETWORK.to_string(),
                amount: "1000".to_string(),
                asset: BCH_ASSET_ID.to_string(),
                pay_to: "addr".to_string(),
                max_timeout_seconds: 60,
                extra: json!({}),
            }],
            extensions: json!({}),
        };
        let result = select_bch_requirements(&pr, true);
        assert!(result.is_none());
    }

    // ── buildPaymentPayload tests ────────────────────────────────────

    #[test]
    fn test_build_payment_payload() {
        let accepted = PaymentRequirements {
            scheme: "utxo".to_string(),
            network: BCH_MAINNET_NETWORK.to_string(),
            amount: "1000".to_string(),
            asset: BCH_ASSET_ID.to_string(),
            pay_to: "bitcoincash:qp2f5j6q3fj5gjwgk8rkq8xrk8q8q8q8q8q8q8q8".to_string(),
            max_timeout_seconds: 300,
            extra: json!({}),
        };

        let result = build_payment_payload(
            &accepted,
            "https://api.example.com/data",
            "bitcoincash:payer-address",
            "abc123txid",
            Some(0),
            Some("1000"),
        );

        assert_eq!(result.x402_version, 2);
        assert_eq!(
            result.resource.as_ref().unwrap().url,
            "https://api.example.com/data"
        );
        assert_eq!(result.accepted, accepted);
        assert_eq!(
            result.payload.authorization.from,
            "bitcoincash:payer-address"
        );
        assert_eq!(result.payload.authorization.to, accepted.pay_to);
        assert_eq!(result.payload.authorization.txid, "abc123txid");
        assert_eq!(result.payload.signature, "");
    }

    // ── buildAuthorization tests ─────────────────────────────────────

    #[test]
    fn test_build_authorization() {
        let accepted = PaymentRequirements {
            scheme: "utxo".to_string(),
            network: BCH_MAINNET_NETWORK.to_string(),
            amount: "1000".to_string(),
            asset: BCH_ASSET_ID.to_string(),
            pay_to: "bitcoincash:qp2f5j6q3fj5gjwgk8rkq8xrk8q8q8q8q8q8q8q8".to_string(),
            max_timeout_seconds: 300,
            extra: json!({}),
        };

        let result = build_authorization(
            &accepted,
            "bitcoincash:payer-address",
            "abc123txid",
            Some(0),
            Some("1000"),
        );

        assert_eq!(result.from, "bitcoincash:payer-address");
        assert_eq!(result.to, accepted.pay_to);
        assert_eq!(result.txid, "abc123txid");
        assert_eq!(result.vout, Some(0));
        assert_eq!(result.amount, Some("1000".to_string()));
    }

}
