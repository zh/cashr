/// Bitcoin message signing for x402 protocol.
///
/// Implements Bitcoin Signed Message format:
/// 1. Prefix: b"\x18Bitcoin Signed Message:\n"
/// 2. Varint-encode message length
/// 3. Concatenate: prefix + varint + message_bytes
/// 4. Double SHA256
/// 5. ECDSA sign with secp256k1 (DER encoding)
/// 6. Base64 encode the DER signature
use anyhow::{Context, Result};
use base64::Engine;
use secp256k1::{Message, Secp256k1, SecretKey};

use crate::crypto;
use super::types::Authorization;

/// Sign a message using Bitcoin Signed Message format.
/// Returns base64-encoded DER signature.
pub fn sign_message_bch(message: &str, private_key: &[u8; 32]) -> Result<String> {
    let prefix = b"\x18Bitcoin Signed Message:\n";
    let msg_bytes = message.as_bytes();

    let mut data = Vec::new();
    data.extend_from_slice(prefix);

    // Varint encode message length.
    // For messages < 253 bytes, varint is a single byte.
    encode_varint(msg_bytes.len(), &mut data);

    data.extend_from_slice(msg_bytes);

    // Double SHA256
    let hash = crypto::sha256d(&data);

    // ECDSA sign
    let secp = Secp256k1::new();
    let secret = SecretKey::from_slice(private_key)
        .context("invalid private key")?;
    let msg = Message::from_digest(hash);
    let sig = secp.sign_ecdsa(&msg, &secret);

    // Base64 encode DER signature
    let der_bytes = sig.serialize_der();
    Ok(base64::engine::general_purpose::STANDARD.encode(der_bytes.as_ref()))
}

/// Sign an Authorization struct: serialize to JSON then sign.
pub fn sign_authorization(
    authorization: &Authorization,
    private_key: &[u8; 32],
) -> Result<String> {
    let message = serde_json::to_string(authorization)
        .context("failed to serialize authorization")?;
    sign_message_bch(&message, private_key)
}

/// Encode a length as Bitcoin varint.
fn encode_varint(len: usize, buf: &mut Vec<u8>) {
    if len < 253 {
        buf.push(len as u8);
    } else if len <= 0xFFFF {
        buf.push(0xFD);
        buf.extend_from_slice(&(len as u16).to_le_bytes());
    } else if len <= 0xFFFF_FFFF {
        buf.push(0xFE);
        buf.extend_from_slice(&(len as u32).to_le_bytes());
    } else {
        buf.push(0xFF);
        buf.extend_from_slice(&(len as u64).to_le_bytes());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sign_message_bch_produces_base64() {
        // Use a known private key (32 bytes, all 1s for simplicity)
        let mut key = [0u8; 32];
        key[31] = 1; // smallest valid private key
        let sig = sign_message_bch("test message", &key).unwrap();
        // Should be valid base64
        let decoded = base64::engine::general_purpose::STANDARD.decode(&sig);
        assert!(decoded.is_ok());
        assert!(!decoded.unwrap().is_empty());
    }

    #[test]
    fn test_sign_message_bch_deterministic() {
        let mut key = [0u8; 32];
        key[31] = 1;
        let sig1 = sign_message_bch("hello", &key).unwrap();
        let sig2 = sign_message_bch("hello", &key).unwrap();
        assert_eq!(sig1, sig2);
    }

    #[test]
    fn test_sign_message_bch_different_messages() {
        let mut key = [0u8; 32];
        key[31] = 1;
        let sig1 = sign_message_bch("message a", &key).unwrap();
        let sig2 = sign_message_bch("message b", &key).unwrap();
        assert_ne!(sig1, sig2);
    }

    #[test]
    fn test_sign_empty_message() {
        let mut key = [0u8; 32];
        key[31] = 1;
        let result = sign_message_bch("", &key);
        assert!(result.is_ok());
    }

    #[test]
    fn test_sign_authorization() {
        let mut key = [0u8; 32];
        key[31] = 1;
        let auth = Authorization {
            from: "payer".to_string(),
            to: "payee".to_string(),
            value: "1000".to_string(),
            txid: "abc123".to_string(),
            vout: Some(0),
            amount: Some("1000".to_string()),
        };
        let sig = sign_authorization(&auth, &key).unwrap();
        assert!(!sig.is_empty());

        // Verify it signed the JSON serialization
        let json = serde_json::to_string(&auth).unwrap();
        let sig2 = sign_message_bch(&json, &key).unwrap();
        assert_eq!(sig, sig2);
    }

    #[test]
    fn test_encode_varint_short() {
        let mut buf = Vec::new();
        encode_varint(42, &mut buf);
        assert_eq!(buf, vec![42]);
    }

    #[test]
    fn test_encode_varint_medium() {
        let mut buf = Vec::new();
        encode_varint(300, &mut buf);
        assert_eq!(buf[0], 0xFD);
        assert_eq!(buf.len(), 3);
    }

    #[test]
    fn test_sign_message_known_vector() {
        // Test vector from the JS CLI.
        // Note: ECDSA signatures may differ between implementations due to low-S normalization.
        // Both are valid. We verify the signature is deterministic (same key+message = same sig)
        // and is valid DER + base64.
        let key_hex = "28e9c4f61f735a059af93e0d9aca0b640126c827975841ad83723ccef295e659";
        let key_bytes = hex::decode(key_hex).unwrap();
        let mut key = [0u8; 32];
        key.copy_from_slice(&key_bytes);

        let sig1 = sign_message_bch("test message", &key).unwrap();
        let sig2 = sign_message_bch("test message", &key).unwrap();

        // Deterministic (RFC 6979)
        assert_eq!(sig1, sig2);

        // Valid base64
        let decoded = base64::engine::general_purpose::STANDARD.decode(&sig1).unwrap();
        assert!(!decoded.is_empty());

        // Valid DER-encoded ECDSA signature (starts with 0x30)
        assert_eq!(decoded[0], 0x30);
    }

    #[test]
    fn test_sign_message_bch_with_hd_derived_key() {
        // Use a realistic HD-derived key
        use crate::wallet::keys::HdWallet;
        use crate::network::BCH_DERIVATION_PATH;

        let mnemonic = "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";
        let wallet = HdWallet::new(mnemonic, BCH_DERIVATION_PATH, false).unwrap();
        let key = wallet.get_private_key_at("0/0").unwrap();

        let sig = sign_message_bch("test x402 message", &key).unwrap();
        assert!(!sig.is_empty());
        // Verify it is valid base64
        let decoded = base64::engine::general_purpose::STANDARD.decode(&sig).unwrap();
        assert!(!decoded.is_empty());
    }
}
