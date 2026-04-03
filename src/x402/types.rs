/// x402-bch v2.2 protocol types.
///
/// All structs use camelCase JSON serialization to match the x402 specification.
use serde::{Deserialize, Serialize};

pub const BCH_ASSET_ID: &str = "0x0000000000000000000000000000000000000001";
pub const BCH_MAINNET_NETWORK: &str = "bip122:000000000000000000651ef99cb9fcbe";
pub const BCH_CHIPNET_NETWORK: &str =
    "bip122:000000000019d6689c085ae165831e934ff763ae46a2a6c172b3f1b60a8ce26f";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ResourceInfo {
    pub url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(rename = "mimeType", skip_serializing_if = "Option::is_none")]
    pub mime_type: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PaymentRequirements {
    pub scheme: String,
    pub network: String,
    pub amount: String,
    pub asset: String,
    pub pay_to: String,
    pub max_timeout_seconds: u32,
    #[serde(default)]
    pub extra: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PaymentRequired {
    pub x402_version: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    pub resource: ResourceInfo,
    pub accepts: Vec<PaymentRequirements>,
    #[serde(default)]
    pub extensions: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Authorization {
    pub from: String,
    pub to: String,
    pub value: String,
    pub txid: String,
    pub vout: Option<u32>,
    pub amount: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Payload {
    pub signature: String,
    pub authorization: Authorization,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PaymentPayload {
    pub x402_version: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resource: Option<ResourceInfo>,
    pub accepted: PaymentRequirements,
    pub payload: Payload,
    #[serde(default)]
    pub extensions: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct VerifyResponse {
    pub is_valid: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub payer: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub invalid_reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub remaining_balance_sat: Option<String>,
}

/// Result of an x402 payment attempt.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct X402PaymentResult {
    pub success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub response: Option<X402Response>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub txid: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub settlement: Option<X402Settlement>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct X402Response {
    pub status: u16,
    #[serde(rename = "statusText")]
    pub status_text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct X402Settlement {
    pub success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub txid: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// All x402 error codes.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    MissingAuthorization,
    InvalidPayload,
    InvalidScheme,
    InvalidNetwork,
    InvalidReceiverAddress,
    InvalidExactBchPayloadSignature,
    InsufficientUtxoBalance,
    UtxoNotFound,
    NoUtxoFoundForAddress,
    UnexpectedUtxoValidationError,
    UnexpectedVerifyError,
    UnexpectedSettleError,
    InvalidX402Version,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_constants() {
        assert_eq!(
            BCH_ASSET_ID,
            "0x0000000000000000000000000000000000000001"
        );
        assert!(BCH_MAINNET_NETWORK.starts_with("bip122:"));
        assert!(BCH_CHIPNET_NETWORK.starts_with("bip122:"));
    }

    #[test]
    fn test_resource_info_roundtrip() {
        let info = ResourceInfo {
            url: "https://example.com".to_string(),
            description: Some("desc".to_string()),
            mime_type: Some("application/json".to_string()),
        };
        let json = serde_json::to_string(&info).unwrap();
        let parsed: ResourceInfo = serde_json::from_str(&json).unwrap();
        assert_eq!(info, parsed);
        assert!(json.contains("mimeType"));
    }

    #[test]
    fn test_payment_requirements_camel_case() {
        let pr = PaymentRequirements {
            scheme: "utxo".to_string(),
            network: BCH_MAINNET_NETWORK.to_string(),
            amount: "1000".to_string(),
            asset: BCH_ASSET_ID.to_string(),
            pay_to: "bitcoincash:qtest".to_string(),
            max_timeout_seconds: 300,
            extra: serde_json::json!({}),
        };
        let json = serde_json::to_string(&pr).unwrap();
        assert!(json.contains("payTo"));
        assert!(json.contains("maxTimeoutSeconds"));
        let parsed: PaymentRequirements = serde_json::from_str(&json).unwrap();
        assert_eq!(pr, parsed);
    }

    #[test]
    fn test_payment_required_roundtrip() {
        let pr = PaymentRequired {
            x402_version: 2,
            error: None,
            resource: ResourceInfo {
                url: "https://api.example.com".to_string(),
                description: None,
                mime_type: None,
            },
            accepts: vec![],
            extensions: serde_json::json!({}),
        };
        let json = serde_json::to_string(&pr).unwrap();
        assert!(json.contains("x402Version"));
        let parsed: PaymentRequired = serde_json::from_str(&json).unwrap();
        assert_eq!(pr, parsed);
    }

    #[test]
    fn test_authorization_roundtrip() {
        let auth = Authorization {
            from: "payer".to_string(),
            to: "payee".to_string(),
            value: "1000".to_string(),
            txid: "abc123".to_string(),
            vout: Some(0),
            amount: Some("1000".to_string()),
        };
        let json = serde_json::to_string(&auth).unwrap();
        let parsed: Authorization = serde_json::from_str(&json).unwrap();
        assert_eq!(auth, parsed);
    }

    #[test]
    fn test_payment_payload_roundtrip() {
        let payload = PaymentPayload {
            x402_version: 2,
            resource: None,
            accepted: PaymentRequirements {
                scheme: "utxo".to_string(),
                network: BCH_MAINNET_NETWORK.to_string(),
                amount: "1000".to_string(),
                asset: BCH_ASSET_ID.to_string(),
                pay_to: "bitcoincash:qtest".to_string(),
                max_timeout_seconds: 60,
                extra: serde_json::json!({}),
            },
            payload: Payload {
                signature: "sig".to_string(),
                authorization: Authorization {
                    from: "from".to_string(),
                    to: "to".to_string(),
                    value: "1000".to_string(),
                    txid: "tx".to_string(),
                    vout: None,
                    amount: None,
                },
            },
            extensions: serde_json::json!({}),
        };
        let json = serde_json::to_string(&payload).unwrap();
        assert!(json.contains("x402Version"));
        let parsed: PaymentPayload = serde_json::from_str(&json).unwrap();
        assert_eq!(payload, parsed);
    }

    #[test]
    fn test_verify_response_roundtrip() {
        let resp = VerifyResponse {
            is_valid: true,
            payer: Some("addr".to_string()),
            invalid_reason: None,
            remaining_balance_sat: Some("50000".to_string()),
        };
        let json = serde_json::to_string(&resp).unwrap();
        assert!(json.contains("isValid"));
        assert!(json.contains("remainingBalanceSat"));
        let parsed: VerifyResponse = serde_json::from_str(&json).unwrap();
        assert_eq!(resp, parsed);
    }

    #[test]
    fn test_error_code_serde() {
        let code = ErrorCode::MissingAuthorization;
        let json = serde_json::to_string(&code).unwrap();
        assert_eq!(json, "\"missing_authorization\"");

        let code2 = ErrorCode::InvalidExactBchPayloadSignature;
        let json2 = serde_json::to_string(&code2).unwrap();
        assert_eq!(json2, "\"invalid_exact_bch_payload_signature\"");
    }
}
