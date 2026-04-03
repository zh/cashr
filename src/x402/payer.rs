/// X402Payer: orchestrates signing + payload creation for x402 payments.
use anyhow::Result;

use crate::wallet::keys::HdWallet;
use super::protocol::{build_authorization, build_payment_payload};
use super::signing::sign_authorization;
use super::types::{PaymentPayload, PaymentRequirements};

/// Payer for x402 payments, holding a private key and address.
pub struct X402Payer {
    address: String,
    private_key: [u8; 32],
    #[allow(dead_code)]
    chipnet: bool,
}

impl X402Payer {
    /// Create a new payer from an HD wallet at a specific address index.
    /// Derives private key and address at receiving path 0/{index}.
    pub fn new(hd_wallet: &HdWallet, address_index: u32) -> Result<Self> {
        let path = format!("0/{}", address_index);
        let private_key = hd_wallet.get_private_key_at(&path)?;
        let address = hd_wallet.get_address_at(&path, false)?;

        Ok(Self {
            address,
            private_key,
            chipnet: hd_wallet.is_chipnet(),
        })
    }

    /// Get the payer's CashAddress.
    pub fn payer_address(&self) -> &str {
        &self.address
    }

    /// Build and sign a PaymentPayload.
    pub fn create_payment_payload(
        &self,
        requirements: &PaymentRequirements,
        resource_url: &str,
        txid: &str,
        vout: Option<u32>,
        amount: Option<&str>,
    ) -> Result<PaymentPayload> {
        // Build the unsigned payload
        let mut payload = build_payment_payload(
            requirements,
            resource_url,
            &self.address,
            txid,
            vout,
            amount,
        );

        // Build and sign the authorization
        let authorization = build_authorization(
            requirements,
            &self.address,
            txid,
            vout,
            amount,
        );
        let signature = sign_authorization(&authorization, &self.private_key)?;

        // Set the signature on the payload
        payload.payload.signature = signature;
        payload.payload.authorization = authorization;

        Ok(payload)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::network::BCH_DERIVATION_PATH;
    use crate::x402::types::*;
    use serde_json::json;

    const TEST_MNEMONIC: &str =
        "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";

    #[test]
    fn test_x402_payer_new() {
        let wallet = HdWallet::new(TEST_MNEMONIC, BCH_DERIVATION_PATH, false).unwrap();
        let payer = X402Payer::new(&wallet, 0).unwrap();
        assert!(payer.payer_address().starts_with("bitcoincash:q"));
    }

    #[test]
    fn test_x402_payer_address_matches_wallet() {
        let wallet = HdWallet::new(TEST_MNEMONIC, BCH_DERIVATION_PATH, false).unwrap();
        let expected_addr = wallet.get_address_at("0/0", false).unwrap();
        let payer = X402Payer::new(&wallet, 0).unwrap();
        assert_eq!(payer.payer_address(), &expected_addr);
    }

    #[test]
    fn test_create_payment_payload() {
        let wallet = HdWallet::new(TEST_MNEMONIC, BCH_DERIVATION_PATH, false).unwrap();
        let payer = X402Payer::new(&wallet, 0).unwrap();

        let requirements = PaymentRequirements {
            scheme: "utxo".to_string(),
            network: BCH_MAINNET_NETWORK.to_string(),
            amount: "1000".to_string(),
            asset: BCH_ASSET_ID.to_string(),
            pay_to: "bitcoincash:qtest".to_string(),
            max_timeout_seconds: 300,
            extra: json!({}),
        };

        let payload = payer
            .create_payment_payload(
                &requirements,
                "https://api.example.com/data",
                "txid123",
                Some(0),
                Some("1000"),
            )
            .unwrap();

        assert_eq!(payload.x402_version, 2);
        assert!(!payload.payload.signature.is_empty());
        assert_eq!(payload.payload.authorization.from, payer.payer_address());
        assert_eq!(payload.payload.authorization.to, "bitcoincash:qtest");
        assert_eq!(payload.payload.authorization.txid, "txid123");

        // Signature should be valid base64
        let decoded = base64::Engine::decode(
            &base64::engine::general_purpose::STANDARD,
            &payload.payload.signature,
        );
        assert!(decoded.is_ok());
    }

    #[test]
    fn test_create_payment_payload_chipnet() {
        let wallet = HdWallet::new(TEST_MNEMONIC, BCH_DERIVATION_PATH, true).unwrap();
        let payer = X402Payer::new(&wallet, 0).unwrap();
        assert!(payer.payer_address().starts_with("bchtest:q"));
    }

    #[test]
    fn test_create_payment_payload_different_indices() {
        let wallet = HdWallet::new(TEST_MNEMONIC, BCH_DERIVATION_PATH, false).unwrap();
        let payer0 = X402Payer::new(&wallet, 0).unwrap();
        let payer1 = X402Payer::new(&wallet, 1).unwrap();
        assert_ne!(payer0.payer_address(), payer1.payer_address());
    }
}
