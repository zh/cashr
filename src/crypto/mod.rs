/// Address derivation pipeline: pubkey -> CashAddress.
///
/// Pure functions with zero state. Each function is independently testable.
use anyhow::{bail, Context, Result};
use ripemd::Ripemd160;
use sha2::{Digest, Sha256};

// CashAddress type constants
const P2PKH: u8 = 0;
const P2SH: u8 = 1;
const P2PKH_WITH_TOKENS: u8 = 2;
const P2SH_WITH_TOKENS: u8 = 3;

/// SHA-256 hash.
pub fn sha256(data: &[u8]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(data);
    let result = hasher.finalize();
    let mut out = [0u8; 32];
    out.copy_from_slice(&result);
    out
}

/// Double SHA-256 (used in Bitcoin message signing).
pub fn sha256d(data: &[u8]) -> [u8; 32] {
    sha256(&sha256(data))
}

/// RIPEMD160(SHA256(data)) -- standard Bitcoin Hash160.
pub fn hash160(data: &[u8]) -> [u8; 20] {
    let sha = sha256(data);
    let mut hasher = Ripemd160::new();
    hasher.update(sha);
    let result = hasher.finalize();
    let mut out = [0u8; 20];
    out.copy_from_slice(&result);
    out
}

/// Compressed pubkey (hex) -> 20-byte public key hash.
pub fn pubkey_to_pkhash(pubkey_hex: &str) -> Result<[u8; 20]> {
    if pubkey_hex.is_empty() {
        bail!("pubkey cannot be empty");
    }
    let pubkey_bytes = hex::decode(pubkey_hex).context("invalid hex in pubkey")?;
    if pubkey_bytes.len() != 33 {
        bail!(
            "expected 33-byte compressed pubkey, got {} bytes",
            pubkey_bytes.len()
        );
    }
    Ok(hash160(&pubkey_bytes))
}

/// 20-byte pkhash -> Base58Check legacy address (version 0x00).
/// Used for test cross-verification.
pub fn pkhash_to_legacy_address(pkhash: &[u8; 20]) -> String {
    let mut data = Vec::with_capacity(21);
    data.push(0x00); // version byte
    data.extend_from_slice(pkhash);
    bs58::encode(data).with_check().into_string()
}

/// 20-byte pkhash -> CashAddress (q-prefix for p2pkh).
pub fn pkhash_to_cashaddr(pkhash: &[u8; 20], chipnet: bool) -> Result<String> {
    let prefix = if chipnet { "bchtest" } else { "bitcoincash" };
    encode_cashaddr(prefix, P2PKH, pkhash)
}

/// Full pipeline: compressed pubkey hex -> CashAddress.
pub fn pubkey_to_address(pubkey_hex: &str, chipnet: bool) -> Result<String> {
    let pkhash = pubkey_to_pkhash(pubkey_hex)?;
    pkhash_to_cashaddr(&pkhash, chipnet)
}

/// Convert regular CashAddress to token-aware (z-prefix).
/// p2pkh (type 0) -> p2pkhWithTokens (type 2)
/// p2sh (type 1) -> p2shWithTokens (type 3)
/// Already token-aware -> pass through unchanged.
pub fn to_token_address(address: &str) -> Result<String> {
    let (prefix, addr_type, payload) = decode_cashaddr(address)?;
    let new_type = match addr_type {
        P2PKH => P2PKH_WITH_TOKENS,
        P2SH => P2SH_WITH_TOKENS,
        P2PKH_WITH_TOKENS | P2SH_WITH_TOKENS => return Ok(address.to_string()),
        other => bail!("unknown CashAddress type: {}", other),
    };
    encode_cashaddr(&prefix, new_type, &payload)
}

/// Re-encode CashAddress with different prefix and/or type.
pub fn convert_cash_address(address: &str, to_testnet: bool, to_token: bool) -> Result<String> {
    let (_prefix, addr_type, payload) = decode_cashaddr(address)?;
    let new_prefix = if to_testnet { "bchtest" } else { "bitcoincash" };
    let new_type = if to_token {
        match addr_type {
            P2PKH | P2PKH_WITH_TOKENS => P2PKH_WITH_TOKENS,
            P2SH | P2SH_WITH_TOKENS => P2SH_WITH_TOKENS,
            other => other,
        }
    } else {
        match addr_type {
            P2PKH | P2PKH_WITH_TOKENS => P2PKH,
            P2SH | P2SH_WITH_TOKENS => P2SH,
            other => other,
        }
    };
    encode_cashaddr(new_prefix, new_type, &payload)
}

// ── CashAddress encoding/decoding ────────────────────────────────────
//
// The `cashaddr` crate (0.2) has a limited API. We implement encode/decode
// manually using the CashAddr specification (polymod-based checksum).
// This is ~120 lines but gives us full control over type bits 0-15.

/// Encode a CashAddress from prefix, type, and payload hash.
fn encode_cashaddr(prefix: &str, addr_type: u8, payload: &[u8]) -> Result<String> {
    // Determine size bit from payload length
    let size_bit: u8 = match payload.len() {
        20 => 0,
        24 => 1,
        28 => 2,
        32 => 3,
        40 => 4,
        48 => 5,
        56 => 6,
        64 => 7,
        _ => bail!("unsupported payload length: {}", payload.len()),
    };

    // Version byte: (type << 3) | size_bit
    let version_byte = (addr_type << 3) | size_bit;

    // Build data: version_byte + payload
    let mut data = Vec::with_capacity(1 + payload.len());
    data.push(version_byte);
    data.extend_from_slice(payload);

    // Convert to 5-bit groups
    let data_5bit = convert_bits(&data, 8, 5, true)?;

    // Compute checksum
    let checksum = compute_checksum(prefix, &data_5bit);

    // Build output string
    let charset = b"qpzry9x8gf2tvdw0s3jn54khce6mua7l";
    let mut result = String::with_capacity(prefix.len() + 1 + data_5bit.len() + 8);
    result.push_str(prefix);
    result.push(':');
    for &b in &data_5bit {
        result.push(charset[b as usize] as char);
    }
    for &b in &checksum {
        result.push(charset[b as usize] as char);
    }

    Ok(result)
}

/// Decode a CashAddress into (prefix, type, payload).
fn decode_cashaddr(address: &str) -> Result<(String, u8, Vec<u8>)> {
    // Handle addresses with or without prefix
    let (prefix, payload_str) = if let Some(idx) = address.find(':') {
        let p = &address[..idx];
        let d = &address[idx + 1..];
        (p.to_lowercase(), d.to_lowercase())
    } else {
        // Try to detect prefix by attempting decode with known prefixes
        // For now, require prefix
        bail!("CashAddress must include prefix (e.g. bitcoincash: or bchtest:)");
    };

    let charset = b"qpzry9x8gf2tvdw0s3jn54khce6mua7l";
    let mut values = Vec::with_capacity(payload_str.len());
    for ch in payload_str.bytes() {
        let pos = charset
            .iter()
            .position(|&c| c == ch)
            .ok_or_else(|| anyhow::anyhow!("invalid character in CashAddress: {}", ch as char))?;
        values.push(pos as u8);
    }

    if values.len() < 8 {
        bail!("CashAddress too short");
    }

    // Verify checksum
    let (data_part, checksum_part) = values.split_at(values.len() - 8);
    let expected = compute_checksum(&prefix, data_part);
    if checksum_part != expected.as_slice() {
        bail!("invalid CashAddress checksum");
    }

    // Convert from 5-bit back to 8-bit
    let data_8bit = convert_bits(data_part, 5, 8, false)?;
    if data_8bit.is_empty() {
        bail!("empty CashAddress payload");
    }

    let version_byte = data_8bit[0];
    let addr_type = version_byte >> 3;
    let payload = data_8bit[1..].to_vec();

    Ok((prefix, addr_type, payload))
}

/// Polymod for CashAddress checksum computation.
fn polymod(values: &[u8]) -> u64 {
    let mut c: u64 = 1;
    for &v in values {
        let c0 = (c >> 35) as u8;
        c = ((c & 0x07_ffff_ffff) << 5) ^ (v as u64);
        if c0 & 0x01 != 0 { c ^= 0x98_f2bc_8e61; }
        if c0 & 0x02 != 0 { c ^= 0x79_b76d_99e2; }
        if c0 & 0x04 != 0 { c ^= 0xf3_3e5f_b3c4; }
        if c0 & 0x08 != 0 { c ^= 0xae_2eab_e2a8; }
        if c0 & 0x10 != 0 { c ^= 0x1e_4f43_e470; }
    }
    c ^ 1
}

/// Compute CashAddress checksum.
fn compute_checksum(prefix: &str, data: &[u8]) -> Vec<u8> {
    let mut values = Vec::new();
    // Prefix is encoded as lower 5 bits of each char
    for ch in prefix.bytes() {
        values.push(ch & 0x1f);
    }
    values.push(0); // separator
    values.extend_from_slice(data);
    // Append 8 zero bytes for checksum template
    values.extend_from_slice(&[0u8; 8]);

    let poly = polymod(&values);
    let mut checksum = Vec::with_capacity(8);
    for i in 0..8 {
        checksum.push(((poly >> (5 * (7 - i))) & 0x1f) as u8);
    }
    checksum
}

/// Convert between bit groups (e.g., 8-bit to 5-bit and vice versa).
fn convert_bits(data: &[u8], from_bits: u32, to_bits: u32, pad: bool) -> Result<Vec<u8>> {
    let mut acc: u32 = 0;
    let mut bits: u32 = 0;
    let max_v: u32 = (1 << to_bits) - 1;
    let mut result = Vec::new();

    for &value in data {
        let v = value as u32;
        if v >> from_bits != 0 {
            bail!("invalid value for {}-bit encoding: {}", from_bits, v);
        }
        acc = (acc << from_bits) | v;
        bits += from_bits;
        while bits >= to_bits {
            bits -= to_bits;
            result.push(((acc >> bits) & max_v) as u8);
        }
    }

    if pad {
        if bits > 0 {
            result.push(((acc << (to_bits - bits)) & max_v) as u8);
        }
    } else if bits >= from_bits {
        bail!("non-zero padding in conversion");
    } else if ((acc << (to_bits - bits)) & max_v) != 0 {
        // Non-zero padding bits are acceptable in CashAddress decoding
        // Only fail if there are more bits than from_bits
    }

    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── Test vectors from JS paytaca wallet (derivation path 0/0) ────
    const PAYTACA_PUBKEY: &str =
        "02bbe7dbcdf8b2261530a867df7180b17a90b482f74f2736b8a30d3f756e42e217";
    const PAYTACA_MAINNET: &str =
        "bitcoincash:qqyx49mu0kkn9ftfj6hje6g2wfer34yfnq5tahq3q6";
    const PAYTACA_CHIPNET: &str =
        "bchtest:qqyx49mu0kkn9ftfj6hje6g2wfer34yfnqseeszx8x";
    const PAYTACA_TOKEN_MAINNET: &str =
        "bitcoincash:zqyx49mu0kkn9ftfj6hje6g2wfer34yfnqnpwfwhlf";
    const PAYTACA_TOKEN_CHIPNET: &str =
        "bchtest:zqyx49mu0kkn9ftfj6hje6g2wfer34yfnqhn2wvqc4";

    // ── 1. test_sha256 ──────────────────────────────────────────────

    #[test]
    fn test_sha256() {
        let hash = sha256(b"hello");
        assert_eq!(
            hex::encode(hash),
            "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"
        );
    }

    // ── 2. test_sha256d ─────────────────────────────────────────────

    #[test]
    fn test_sha256d() {
        // Verify double-hash identity: sha256d(x) == sha256(sha256(x))
        let hash = sha256d(b"hello");
        let expected = sha256(&sha256(b"hello"));
        assert_eq!(hash, expected);
        // Known value: sha256d("hello")
        assert_eq!(
            hex::encode(hash),
            "9595c9df90075148eb06860365df33584b75bff782a510c6cd4883a419833d50"
        );
    }

    // ── 3. test_hash160 ─────────────────────────────────────────────

    #[test]
    fn test_hash160() {
        // Use the paytaca pubkey to verify hash160 produces the expected pkhash
        let pubkey_bytes = hex::decode(PAYTACA_PUBKEY).unwrap();
        let h160 = hash160(&pubkey_bytes);
        // The pkhash is embedded in the CashAddress. We verify by round-tripping.
        let addr = pkhash_to_cashaddr(&h160, false).unwrap();
        assert_eq!(addr, PAYTACA_MAINNET);
    }

    // ── 4. test_pubkey_to_pkhash ────────────────────────────────────

    #[test]
    fn test_pubkey_to_pkhash() {
        let pkhash = pubkey_to_pkhash(PAYTACA_PUBKEY).unwrap();
        assert_eq!(pkhash.len(), 20);
        // Cross-verify: encoding this pkhash should yield the known mainnet address
        let addr = pkhash_to_cashaddr(&pkhash, false).unwrap();
        assert_eq!(addr, PAYTACA_MAINNET);
    }

    // ── 5. test_pkhash_to_legacy_address ────────────────────────────

    #[test]
    fn test_pkhash_to_legacy_address() {
        // Bitcoin wiki test vector: pubkey -> legacy address
        let wiki_pubkey = "0250863ad64a87ae8a2fe83c1af1a8403cb53f53e486d8511dad8a04887e5b2352";
        let pkhash = pubkey_to_pkhash(wiki_pubkey).unwrap();
        let legacy = pkhash_to_legacy_address(&pkhash);
        assert_eq!(legacy, "1PMycacnJaSqwwJqjawXBErnLsZ7RkXUAs");

        // Also verify paytaca pubkey produces a valid legacy address (starts with 1)
        let paytaca_pkhash = pubkey_to_pkhash(PAYTACA_PUBKEY).unwrap();
        let paytaca_legacy = pkhash_to_legacy_address(&paytaca_pkhash);
        assert!(paytaca_legacy.starts_with('1'));
    }

    // ── 6. test_pubkey_to_address_mainnet ───────────────────────────

    #[test]
    fn test_pubkey_to_address_mainnet() {
        let addr = pubkey_to_address(PAYTACA_PUBKEY, false).unwrap();
        assert_eq!(addr, PAYTACA_MAINNET);
    }

    // ── 7. test_pubkey_to_address_chipnet ───────────────────────────

    #[test]
    fn test_pubkey_to_address_chipnet() {
        let addr = pubkey_to_address(PAYTACA_PUBKEY, true).unwrap();
        assert_eq!(addr, PAYTACA_CHIPNET);
    }

    // ── 8. test_to_token_address_mainnet ────────────────────────────

    #[test]
    fn test_to_token_address_mainnet() {
        let token_addr = to_token_address(PAYTACA_MAINNET).unwrap();
        assert_eq!(token_addr, PAYTACA_TOKEN_MAINNET);
    }

    // ── 9. test_to_token_address_chipnet ────────────────────────────

    #[test]
    fn test_to_token_address_chipnet() {
        let token_addr = to_token_address(PAYTACA_CHIPNET).unwrap();
        assert_eq!(token_addr, PAYTACA_TOKEN_CHIPNET);
    }

    // ── 10. test_to_token_address_already_token ─────────────────────

    #[test]
    fn test_to_token_address_already_token() {
        // Mainnet token address passes through unchanged
        let result = to_token_address(PAYTACA_TOKEN_MAINNET).unwrap();
        assert_eq!(result, PAYTACA_TOKEN_MAINNET);

        // Chipnet token address passes through unchanged
        let result = to_token_address(PAYTACA_TOKEN_CHIPNET).unwrap();
        assert_eq!(result, PAYTACA_TOKEN_CHIPNET);
    }

    // ── 11. test_convert_cash_address_mainnet_to_chipnet ────────────

    #[test]
    fn test_convert_cash_address_mainnet_to_chipnet() {
        let converted = convert_cash_address(PAYTACA_MAINNET, true, false).unwrap();
        assert_eq!(converted, PAYTACA_CHIPNET);

        // And back
        let back = convert_cash_address(&converted, false, false).unwrap();
        assert_eq!(back, PAYTACA_MAINNET);
    }

    // ── 12. test_pubkey_to_address_invalid_hex ──────────────────────

    #[test]
    fn test_pubkey_to_address_invalid_hex() {
        let result = pubkey_to_address("zzzz_not_hex", false);
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("invalid") || msg.contains("hex"));
    }

    // ── 13. test_pubkey_to_address_empty ────────────────────────────

    #[test]
    fn test_pubkey_to_address_empty() {
        let result = pubkey_to_address("", false);
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("empty"));
    }

    // ── Additional coverage ─────────────────────────────────────────

    #[test]
    fn test_convert_cash_address_to_token_and_testnet() {
        // Mainnet non-token -> chipnet token
        let converted = convert_cash_address(PAYTACA_MAINNET, true, true).unwrap();
        assert_eq!(converted, PAYTACA_TOKEN_CHIPNET);
    }

    #[test]
    fn test_convert_cash_address_token_to_non_token() {
        // Token address -> non-token (strips token type)
        let converted = convert_cash_address(PAYTACA_TOKEN_MAINNET, false, false).unwrap();
        assert_eq!(converted, PAYTACA_MAINNET);
    }

    #[test]
    fn test_cashaddr_roundtrip() {
        let pkhash = pubkey_to_pkhash(PAYTACA_PUBKEY).unwrap();
        let addr = pkhash_to_cashaddr(&pkhash, false).unwrap();
        let (prefix, addr_type, payload) = decode_cashaddr(&addr).unwrap();
        assert_eq!(prefix, "bitcoincash");
        assert_eq!(addr_type, P2PKH);
        assert_eq!(payload, pkhash.to_vec());
    }

    #[test]
    fn test_pubkey_to_pkhash_wrong_length() {
        let result = pubkey_to_pkhash("aabb");
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("33-byte"));
    }
}
