/// Native BCH P2PKH transaction builder.
///
/// Builds, signs, and serializes Bitcoin Cash transactions locally
/// using SIGHASH_FORKID (BIP143-style) for P2PKH inputs.
use anyhow::{bail, Context, Result};
use secp256k1::{Message, Secp256k1, SecretKey};

use crate::crypto;
use crate::wallet::keys::HdWallet;

// ── Public types ────────────────────────────────────────────────────

/// A spendable UTXO with its derivation path for key lookup.
#[derive(Debug, Clone)]
pub struct Utxo {
    pub txid: String,        // 64-char hex
    pub vout: u32,
    pub value: u64,          // satoshis
    pub address_path: String, // e.g. "0/0" or "1/3"
}

/// A transaction output (recipient).
#[derive(Debug, Clone)]
pub struct TxOutput {
    pub address: String, // CashAddress with prefix
    pub value: u64,      // satoshis
}

/// A fully built and signed transaction ready for broadcast.
#[derive(Debug, Clone)]
pub struct BuiltTransaction {
    pub hex: String,  // raw tx hex
    pub txid: String, // computed txid (double-SHA256 of serialized, reversed)
    pub fee: u64,     // fee in satoshis
}

// ── Constants ───────────────────────────────────────────────────────

const SIGHASH_ALL: u32 = 0x01;
const SIGHASH_FORKID: u32 = 0x40;
const SIGHASH_ALL_FORKID: u32 = SIGHASH_ALL | SIGHASH_FORKID; // 0x41

const DUST_LIMIT: u64 = 546;
const DEFAULT_FEE_RATE: f64 = 1.2; // sats/byte

/// Estimated byte sizes for fee calculation.
const INPUT_SIZE: usize = 148;  // P2PKH input
const OUTPUT_SIZE: usize = 34;  // P2PKH output
const OVERHEAD_SIZE: usize = 10; // version + in/out counts + locktime

// ── Public API ──────────────────────────────────────────────────────

/// Build a signed P2PKH transaction.
///
/// Selects UTXOs, constructs outputs, calculates fee, signs inputs,
/// serializes, and computes the txid.
pub fn build_p2pkh_transaction(
    utxos: &[Utxo],
    outputs: &[TxOutput],
    change_address: &str,
    hd_wallet: &HdWallet,
    fee_rate: f64,
) -> Result<BuiltTransaction> {
    if utxos.is_empty() {
        bail!("no UTXOs available");
    }
    if outputs.is_empty() {
        bail!("no outputs specified");
    }

    let fee_rate = if fee_rate <= 0.0 { DEFAULT_FEE_RATE } else { fee_rate };

    // Total amount to send
    let send_total: u64 = outputs.iter().map(|o| o.value).sum();

    // Select UTXOs (greedy: sort by value descending, accumulate)
    let mut sorted_utxos: Vec<&Utxo> = utxos.iter().collect();
    sorted_utxos.sort_by(|a, b| b.value.cmp(&a.value));

    let mut selected: Vec<&Utxo> = Vec::new();
    let mut input_total: u64 = 0;

    for utxo in &sorted_utxos {
        selected.push(utxo);
        input_total += utxo.value;

        // Estimate fee with current selection (+ potential change output)
        let est_size = estimate_tx_size(selected.len(), outputs.len() + 1);
        let est_fee = (est_size as f64 * fee_rate).ceil() as u64;

        if input_total >= send_total + est_fee {
            break;
        }
    }

    // Final fee calculation
    let change_candidate = {
        let est_size = estimate_tx_size(selected.len(), outputs.len() + 1);
        let est_fee = (est_size as f64 * fee_rate).ceil() as u64;
        if input_total < send_total + est_fee {
            // Try without change output
            let est_size_no_change = estimate_tx_size(selected.len(), outputs.len());
            let est_fee_no_change = (est_size_no_change as f64 * fee_rate).ceil() as u64;
            if input_total < send_total + est_fee_no_change {
                let lacking = (send_total + est_fee_no_change) - input_total;
                bail!(
                    "insufficient funds: need {} sats, have {} sats (short by {} sats)",
                    send_total + est_fee_no_change,
                    input_total,
                    lacking
                );
            }
            // No change output needed/possible
            None
        } else {
            let change_amount = input_total - send_total - est_fee;
            if change_amount >= DUST_LIMIT {
                Some((change_address.to_string(), change_amount, est_fee))
            } else {
                // Change below dust -- donate to fee
                let est_size_no_change = estimate_tx_size(selected.len(), outputs.len());
                let fee_no_change = (est_size_no_change as f64 * fee_rate).ceil() as u64;
                let actual_fee = input_total - send_total;
                if actual_fee < fee_no_change {
                    bail!("insufficient funds after removing sub-dust change");
                }
                None
            }
        }
    };

    // Build final outputs list
    let mut final_outputs: Vec<TxOutput> = outputs.to_vec();
    let fee;

    if let Some((change_addr, change_amount, est_fee)) = change_candidate {
        fee = est_fee;
        final_outputs.push(TxOutput {
            address: change_addr,
            value: change_amount,
        });
    } else {
        fee = input_total - send_total;
    }

    // Convert outputs to script_pubkeys
    let output_scripts: Vec<(u64, Vec<u8>)> = final_outputs
        .iter()
        .map(|o| {
            let script = p2pkh_script_from_address(&o.address)
                .with_context(|| format!("failed to create script for address {}", o.address))?;
            Ok((o.value, script))
        })
        .collect::<Result<Vec<_>>>()?;

    // Build unsigned transaction structure
    let mut raw_tx = RawTx {
        version: 2u32,
        inputs: Vec::new(),
        outputs: output_scripts.clone(),
        locktime: 0u32,
    };

    // Prepare inputs (with empty scriptSig initially)
    for utxo in &selected {
        let prev_txid = decode_txid_to_bytes(&utxo.txid)?;
        raw_tx.inputs.push(RawInput {
            prev_txid,
            prev_vout: utxo.vout,
            script_sig: Vec::new(), // placeholder
            sequence: 0xffffffff,
        });
    }

    // Precompute shared hashes for all input signatures
    let cache = SighashCache {
        hash_prevouts: compute_hash_prevouts(&raw_tx.inputs),
        hash_sequence: compute_hash_sequence(&raw_tx.inputs),
        hash_outputs: compute_hash_outputs(&raw_tx.outputs),
    };

    // Sign each input
    let secp = Secp256k1::signing_only();

    for (i, utxo) in selected.iter().enumerate() {
        let private_key_bytes = hd_wallet.get_private_key_at(&utxo.address_path)?;
        let pubkey_hex = hd_wallet.get_pubkey_at(&utxo.address_path)?;
        let pubkey_bytes = hex::decode(&pubkey_hex).context("invalid pubkey hex")?;

        // The previous output's scriptPubKey (P2PKH of the UTXO's address)
        let prev_script = p2pkh_script_from_pubkey_bytes(&pubkey_bytes);

        // Compute sighash
        let sighash = sighash_forkid(
            &raw_tx,
            i,
            &prev_script,
            utxo.value,
            SIGHASH_ALL_FORKID,
            &cache,
        );

        // Sign
        let secret_key = SecretKey::from_slice(&private_key_bytes)
            .context("invalid private key")?;
        let message = Message::from_digest(sighash);
        let signature = secp.sign_ecdsa(&message, &secret_key);

        // Build scriptSig: <sig + hashtype> <pubkey>
        let der_sig = signature.serialize_der();
        let script_sig = build_script_sig(&der_sig, &pubkey_bytes);
        raw_tx.inputs[i].script_sig = script_sig;
    }

    // Serialize
    let tx_bytes = serialize_tx(&raw_tx);
    let hex_str = hex::encode(&tx_bytes);

    // Compute txid (double-SHA256 of serialized tx, byte-reversed)
    let txid_bytes = crypto::sha256d(&tx_bytes);
    let txid = hex::encode(txid_bytes.iter().rev().copied().collect::<Vec<u8>>());

    Ok(BuiltTransaction {
        hex: hex_str,
        txid,
        fee,
    })
}

/// Build a "send all" transaction: use ALL UTXOs, single output, no change.
/// The output value = total UTXO value - fee.
pub fn build_send_all_transaction(
    utxos: &[Utxo],
    outputs: &[TxOutput],
    hd_wallet: &HdWallet,
) -> Result<BuiltTransaction> {
    if utxos.is_empty() {
        bail!("no UTXOs available");
    }
    if outputs.is_empty() {
        bail!("no outputs specified");
    }

    let input_total: u64 = utxos.iter().map(|u| u.value).sum();
    let output_total: u64 = outputs.iter().map(|o| o.value).sum();
    let fee = input_total - output_total;

    // Convert outputs to script_pubkeys
    let output_scripts: Vec<(u64, Vec<u8>)> = outputs
        .iter()
        .map(|o| {
            let script = p2pkh_script_from_address(&o.address)
                .with_context(|| format!("failed to create script for address {}", o.address))?;
            Ok((o.value, script))
        })
        .collect::<Result<Vec<_>>>()?;

    // Build unsigned transaction
    let mut raw_tx = RawTx {
        version: 2u32,
        inputs: Vec::new(),
        outputs: output_scripts.clone(),
        locktime: 0u32,
    };

    for utxo in utxos {
        let prev_txid = decode_txid_to_bytes(&utxo.txid)?;
        raw_tx.inputs.push(RawInput {
            prev_txid,
            prev_vout: utxo.vout,
            script_sig: Vec::new(),
            sequence: 0xffffffff,
        });
    }

    let cache = SighashCache {
        hash_prevouts: compute_hash_prevouts(&raw_tx.inputs),
        hash_sequence: compute_hash_sequence(&raw_tx.inputs),
        hash_outputs: compute_hash_outputs(&raw_tx.outputs),
    };

    let secp = Secp256k1::signing_only();

    for (i, utxo) in utxos.iter().enumerate() {
        let private_key_bytes = hd_wallet.get_private_key_at(&utxo.address_path)?;
        let pubkey_hex = hd_wallet.get_pubkey_at(&utxo.address_path)?;
        let pubkey_bytes = hex::decode(&pubkey_hex).context("invalid pubkey hex")?;
        let prev_script = p2pkh_script_from_pubkey_bytes(&pubkey_bytes);

        let sighash = sighash_forkid(
            &raw_tx, i, &prev_script, utxo.value, SIGHASH_ALL_FORKID, &cache,
        );

        let secret_key = SecretKey::from_slice(&private_key_bytes).context("invalid private key")?;
        let message = Message::from_digest(sighash);
        let signature = secp.sign_ecdsa(&message, &secret_key);

        let der_sig = signature.serialize_der();
        let script_sig = build_script_sig(&der_sig, &pubkey_bytes);
        raw_tx.inputs[i].script_sig = script_sig;
    }

    let tx_bytes = serialize_tx(&raw_tx);
    let hex_str = hex::encode(&tx_bytes);
    let txid_bytes = crypto::sha256d(&tx_bytes);
    let txid = hex::encode(txid_bytes.iter().rev().copied().collect::<Vec<u8>>());

    Ok(BuiltTransaction { hex: hex_str, txid, fee })
}

// ── Internal types ──────────────────────────────────────────────────

struct RawTx {
    version: u32,
    inputs: Vec<RawInput>,
    outputs: Vec<(u64, Vec<u8>)>, // (value, scriptPubKey)
    locktime: u32,
}

struct RawInput {
    prev_txid: [u8; 32], // byte-reversed from hex
    prev_vout: u32,
    script_sig: Vec<u8>,
    sequence: u32,
}

// ── SIGHASH_FORKID (BIP143-style for BCH) ──────────────────────────

/// Precomputed hashes shared across all input signatures.
struct SighashCache {
    hash_prevouts: [u8; 32],
    hash_sequence: [u8; 32],
    hash_outputs: [u8; 32],
}

fn sighash_forkid(
    tx: &RawTx,
    input_index: usize,
    prev_script: &[u8],
    utxo_value: u64,
    hash_type: u32,
    cache: &SighashCache,
) -> [u8; 32] {
    let mut preimage = Vec::with_capacity(256);

    // 1. version [4 bytes LE]
    preimage.extend_from_slice(&tx.version.to_le_bytes());

    // 2. hash_prevouts [32 bytes]
    preimage.extend_from_slice(&cache.hash_prevouts);

    // 3. hash_sequence [32 bytes]
    preimage.extend_from_slice(&cache.hash_sequence);

    // 4. outpoint [36 bytes] = prev_txid + prev_vout
    let input = &tx.inputs[input_index];
    preimage.extend_from_slice(&input.prev_txid);
    preimage.extend_from_slice(&input.prev_vout.to_le_bytes());

    // 5. script_code [varint + bytes]
    write_varint(&mut preimage, prev_script.len() as u64);
    preimage.extend_from_slice(prev_script);

    // 6. value [8 bytes LE]
    preimage.extend_from_slice(&utxo_value.to_le_bytes());

    // 7. sequence [4 bytes LE]
    preimage.extend_from_slice(&input.sequence.to_le_bytes());

    // 8. hash_outputs [32 bytes]
    preimage.extend_from_slice(&cache.hash_outputs);

    // 9. locktime [4 bytes LE]
    preimage.extend_from_slice(&tx.locktime.to_le_bytes());

    // 10. sighash_type [4 bytes LE]
    preimage.extend_from_slice(&hash_type.to_le_bytes());

    crypto::sha256d(&preimage)
}

fn compute_hash_prevouts(inputs: &[RawInput]) -> [u8; 32] {
    let mut data = Vec::with_capacity(inputs.len() * 36);
    for input in inputs {
        data.extend_from_slice(&input.prev_txid);
        data.extend_from_slice(&input.prev_vout.to_le_bytes());
    }
    crypto::sha256d(&data)
}

fn compute_hash_sequence(inputs: &[RawInput]) -> [u8; 32] {
    let mut data = Vec::with_capacity(inputs.len() * 4);
    for input in inputs {
        data.extend_from_slice(&input.sequence.to_le_bytes());
    }
    crypto::sha256d(&data)
}

fn compute_hash_outputs(outputs: &[(u64, Vec<u8>)]) -> [u8; 32] {
    let mut data = Vec::new();
    for (value, script) in outputs {
        data.extend_from_slice(&value.to_le_bytes());
        write_varint(&mut data, script.len() as u64);
        data.extend_from_slice(script);
    }
    crypto::sha256d(&data)
}

// ── Script construction ─────────────────────────────────────────────

/// Build P2PKH scriptPubKey from a CashAddress.
///
/// OP_DUP OP_HASH160 <20-byte-pkhash> OP_EQUALVERIFY OP_CHECKSIG
pub fn p2pkh_script_from_address(address: &str) -> Result<Vec<u8>> {
    let pkhash = decode_cashaddr_to_pkhash(address)?;
    Ok(build_p2pkh_script(&pkhash))
}

/// Build P2PKH scriptPubKey from a compressed public key.
fn p2pkh_script_from_pubkey_bytes(pubkey_bytes: &[u8]) -> Vec<u8> {
    let pkhash = crypto::hash160(pubkey_bytes);
    build_p2pkh_script(&pkhash)
}

/// Assemble the 25-byte P2PKH script from a 20-byte hash.
fn build_p2pkh_script(pkhash: &[u8; 20]) -> Vec<u8> {
    let mut script = Vec::with_capacity(25);
    script.push(0x76); // OP_DUP
    script.push(0xa9); // OP_HASH160
    script.push(0x14); // push 20 bytes
    script.extend_from_slice(pkhash);
    script.push(0x88); // OP_EQUALVERIFY
    script.push(0xac); // OP_CHECKSIG
    script
}

/// Build scriptSig: <sig + hashtype_byte> <compressed_pubkey>
fn build_script_sig(der_sig: &[u8], pubkey_bytes: &[u8]) -> Vec<u8> {
    let sig_with_hashtype_len = der_sig.len() + 1; // DER sig + 0x41 byte
    let mut script = Vec::with_capacity(1 + sig_with_hashtype_len + 1 + pubkey_bytes.len());

    // Push signature + hashtype
    script.push(sig_with_hashtype_len as u8);
    script.extend_from_slice(der_sig);
    script.push(SIGHASH_ALL_FORKID as u8); // 0x41

    // Push pubkey
    script.push(pubkey_bytes.len() as u8);
    script.extend_from_slice(pubkey_bytes);

    script
}

// ── Transaction serialization ───────────────────────────────────────

fn serialize_tx(tx: &RawTx) -> Vec<u8> {
    let mut buf = Vec::with_capacity(256);

    // Version [4 bytes LE]
    buf.extend_from_slice(&tx.version.to_le_bytes());

    // Input count [varint]
    write_varint(&mut buf, tx.inputs.len() as u64);

    // Inputs
    for input in &tx.inputs {
        buf.extend_from_slice(&input.prev_txid);
        buf.extend_from_slice(&input.prev_vout.to_le_bytes());
        write_varint(&mut buf, input.script_sig.len() as u64);
        buf.extend_from_slice(&input.script_sig);
        buf.extend_from_slice(&input.sequence.to_le_bytes());
    }

    // Output count [varint]
    write_varint(&mut buf, tx.outputs.len() as u64);

    // Outputs
    for (value, script) in &tx.outputs {
        buf.extend_from_slice(&value.to_le_bytes());
        write_varint(&mut buf, script.len() as u64);
        buf.extend_from_slice(script);
    }

    // Locktime [4 bytes LE]
    buf.extend_from_slice(&tx.locktime.to_le_bytes());

    buf
}

// ── Utility functions ───────────────────────────────────────────────

/// Decode a 64-char hex txid to 32 bytes, reversed (internal byte order).
fn decode_txid_to_bytes(txid_hex: &str) -> Result<[u8; 32]> {
    if txid_hex.len() != 64 {
        bail!("txid must be 64 hex characters, got {}", txid_hex.len());
    }
    let bytes = hex::decode(txid_hex).context("invalid hex in txid")?;
    let mut out = [0u8; 32];
    // Reverse byte order (Bitcoin internal representation)
    for (i, &b) in bytes.iter().rev().enumerate() {
        out[i] = b;
    }
    Ok(out)
}

/// Write a Bitcoin-style variable-length integer.
fn write_varint(buf: &mut Vec<u8>, value: u64) {
    if value < 0xfd {
        buf.push(value as u8);
    } else if value <= 0xffff {
        buf.push(0xfd);
        buf.extend_from_slice(&(value as u16).to_le_bytes());
    } else if value <= 0xffff_ffff {
        buf.push(0xfe);
        buf.extend_from_slice(&(value as u32).to_le_bytes());
    } else {
        buf.push(0xff);
        buf.extend_from_slice(&value.to_le_bytes());
    }
}

/// Estimate transaction size in bytes for fee calculation.
pub fn estimate_tx_size(input_count: usize, output_count: usize) -> usize {
    (input_count * INPUT_SIZE) + (output_count * OUTPUT_SIZE) + OVERHEAD_SIZE
}

// ── CashAddress decoding (minimal, self-contained) ──────────────────
//
// We need to extract the 20-byte pkhash from a CashAddress.
// The crypto module's decode_cashaddr is private, so we implement
// a minimal decoder here.

const CASHADDR_CHARSET: &[u8; 32] = b"qpzry9x8gf2tvdw0s3jn54khce6mua7l";

/// Decode a CashAddress to its 20-byte public key hash.
fn decode_cashaddr_to_pkhash(address: &str) -> Result<[u8; 20]> {
    let (prefix, payload_str) = if let Some(idx) = address.find(':') {
        let p = &address[..idx];
        let d = &address[idx + 1..];
        (p.to_lowercase(), d.to_lowercase())
    } else {
        bail!("CashAddress must include prefix (e.g. bitcoincash: or bchtest:)");
    };

    let mut values = Vec::with_capacity(payload_str.len());
    for ch in payload_str.bytes() {
        let pos = CASHADDR_CHARSET
            .iter()
            .position(|&c| c == ch)
            .ok_or_else(|| anyhow::anyhow!("invalid CashAddress character: {}", ch as char))?;
        values.push(pos as u8);
    }

    if values.len() < 8 {
        bail!("CashAddress too short");
    }

    // Strip checksum (last 8 characters)
    let data_part = &values[..values.len() - 8];

    // Verify checksum
    let checksum_part = &values[values.len() - 8..];
    let expected = cashaddr_checksum(&prefix, data_part);
    if checksum_part != expected.as_slice() {
        bail!("invalid CashAddress checksum");
    }

    // Convert from 5-bit to 8-bit
    let data_8bit = convert_bits(data_part, 5, 8, false)?;
    if data_8bit.len() < 2 {
        bail!("CashAddress payload too short");
    }

    // Skip version byte, extract payload
    let payload = &data_8bit[1..];
    if payload.len() != 20 {
        bail!(
            "expected 20-byte payload, got {} bytes",
            payload.len()
        );
    }

    let mut out = [0u8; 20];
    out.copy_from_slice(payload);
    Ok(out)
}

fn cashaddr_polymod(values: &[u8]) -> u64 {
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

fn cashaddr_checksum(prefix: &str, data: &[u8]) -> Vec<u8> {
    let mut values = Vec::new();
    for ch in prefix.bytes() {
        values.push(ch & 0x1f);
    }
    values.push(0);
    values.extend_from_slice(data);
    values.extend_from_slice(&[0u8; 8]);

    let poly = cashaddr_polymod(&values);
    let mut checksum = Vec::with_capacity(8);
    for i in 0..8 {
        checksum.push(((poly >> (5 * (7 - i))) & 0x1f) as u8);
    }
    checksum
}

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

    if pad && bits > 0 {
        result.push(((acc << (to_bits - bits)) & max_v) as u8);
    }

    Ok(result)
}

// ── Tests ───────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::network::BCH_DERIVATION_PATH;

    const TEST_MNEMONIC: &str =
        "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";

    // Known CashAddress for test mnemonic at 0/0 (mainnet)
    const TEST_ADDRESS: &str =
        "bitcoincash:qqyx49mu0kkn9ftfj6hje6g2wfer34yfnq5tahq3q6";

    #[test]
    fn test_p2pkh_script_from_address() {
        let script = p2pkh_script_from_address(TEST_ADDRESS).unwrap();

        // P2PKH script is exactly 25 bytes
        assert_eq!(script.len(), 25);

        // Check opcodes
        assert_eq!(script[0], 0x76); // OP_DUP
        assert_eq!(script[1], 0xa9); // OP_HASH160
        assert_eq!(script[2], 0x14); // push 20 bytes
        assert_eq!(script[23], 0x88); // OP_EQUALVERIFY
        assert_eq!(script[24], 0xac); // OP_CHECKSIG

        // Verify the embedded pkhash matches known pubkey
        let pkhash = crypto::pubkey_to_pkhash(
            "02bbe7dbcdf8b2261530a867df7180b17a90b482f74f2736b8a30d3f756e42e217",
        )
        .unwrap();
        assert_eq!(&script[3..23], &pkhash);
    }

    #[test]
    fn test_p2pkh_script_from_chipnet_address() {
        let chipnet_addr = "bchtest:qqyx49mu0kkn9ftfj6hje6g2wfer34yfnqseeszx8x";
        let script = p2pkh_script_from_address(chipnet_addr).unwrap();
        assert_eq!(script.len(), 25);
        assert_eq!(script[0], 0x76);
    }

    #[test]
    fn test_p2pkh_script_from_token_address() {
        // Token addresses (z-prefix) should also decode correctly
        let token_addr = "bitcoincash:zqyx49mu0kkn9ftfj6hje6g2wfer34yfnqnpwfwhlf";
        let script = p2pkh_script_from_address(token_addr).unwrap();
        assert_eq!(script.len(), 25);
        // Same pkhash as the q-prefix address
        let regular_script = p2pkh_script_from_address(TEST_ADDRESS).unwrap();
        assert_eq!(&script[3..23], &regular_script[3..23]);
    }

    #[test]
    fn test_decode_txid_to_bytes() {
        // A known txid -- verify byte reversal
        let txid = "0000000000000000000000000000000000000000000000000000000000000001";
        let bytes = decode_txid_to_bytes(txid).unwrap();
        // Last byte of hex becomes first byte of internal representation
        assert_eq!(bytes[0], 0x01);
        assert_eq!(bytes[31], 0x00);
    }

    #[test]
    fn test_decode_txid_invalid_length() {
        let result = decode_txid_to_bytes("abcdef");
        assert!(result.is_err());
    }

    #[test]
    fn test_write_varint_small() {
        let mut buf = Vec::new();
        write_varint(&mut buf, 0);
        assert_eq!(buf, vec![0x00]);

        buf.clear();
        write_varint(&mut buf, 252);
        assert_eq!(buf, vec![0xfc]);
    }

    #[test]
    fn test_write_varint_medium() {
        let mut buf = Vec::new();
        write_varint(&mut buf, 253);
        assert_eq!(buf, vec![0xfd, 0xfd, 0x00]);

        buf.clear();
        write_varint(&mut buf, 0xffff);
        assert_eq!(buf, vec![0xfd, 0xff, 0xff]);
    }

    #[test]
    fn test_write_varint_large() {
        let mut buf = Vec::new();
        write_varint(&mut buf, 0x10000);
        assert_eq!(buf, vec![0xfe, 0x00, 0x00, 0x01, 0x00]);
    }

    #[test]
    fn test_fee_estimation() {
        // 1 input, 2 outputs
        let size = estimate_tx_size(1, 2);
        assert_eq!(size, 148 + 68 + 10); // 226

        // 2 inputs, 2 outputs
        let size = estimate_tx_size(2, 2);
        assert_eq!(size, 296 + 68 + 10); // 374

        // Fee at 1.2 sats/byte for 1-in/2-out
        let fee = (226.0_f64 * 1.2).ceil() as u64;
        assert_eq!(fee, 272);
    }

    #[test]
    fn test_tx_serialization_structure() {
        // Build a minimal unsigned transaction and verify serialization format
        let raw_tx = RawTx {
            version: 2,
            inputs: vec![RawInput {
                prev_txid: [0xaa; 32],
                prev_vout: 0,
                script_sig: vec![0x00], // dummy 1-byte scriptSig
                sequence: 0xffffffff,
            }],
            outputs: vec![(
                1000u64,
                build_p2pkh_script(&[0xbb; 20]),
            )],
            locktime: 0,
        };

        let serialized = serialize_tx(&raw_tx);

        // Version: 02000000
        assert_eq!(&serialized[0..4], &[0x02, 0x00, 0x00, 0x00]);

        // Input count: 01
        assert_eq!(serialized[4], 0x01);

        // prev_txid: 32 bytes of 0xaa
        assert_eq!(&serialized[5..37], &[0xaa; 32]);

        // prev_vout: 00000000
        assert_eq!(&serialized[37..41], &[0x00, 0x00, 0x00, 0x00]);

        // scriptSig length: 01, scriptSig: 00
        assert_eq!(serialized[41], 0x01);
        assert_eq!(serialized[42], 0x00);

        // sequence: ffffffff
        assert_eq!(&serialized[43..47], &[0xff, 0xff, 0xff, 0xff]);

        // Output count: 01
        assert_eq!(serialized[47], 0x01);

        // value: 1000 = 0xe803000000000000 LE
        assert_eq!(&serialized[48..56], &1000u64.to_le_bytes());

        // scriptPubKey length: 25 (0x19)
        assert_eq!(serialized[56], 0x19);

        // Locktime at end: 00000000
        let len = serialized.len();
        assert_eq!(&serialized[len - 4..], &[0x00, 0x00, 0x00, 0x00]);
    }

    #[test]
    fn test_sighash_forkid_preimage_structure() {
        // Verify the sighash computation produces a 32-byte hash
        let prev_script = build_p2pkh_script(&[0xcc; 20]);
        let raw_tx = RawTx {
            version: 2,
            inputs: vec![RawInput {
                prev_txid: [0xaa; 32],
                prev_vout: 0,
                script_sig: Vec::new(),
                sequence: 0xffffffff,
            }],
            outputs: vec![(
                1000u64,
                build_p2pkh_script(&[0xbb; 20]),
            )],
            locktime: 0,
        };

        let cache = SighashCache {
            hash_prevouts: compute_hash_prevouts(&raw_tx.inputs),
            hash_sequence: compute_hash_sequence(&raw_tx.inputs),
            hash_outputs: compute_hash_outputs(&raw_tx.outputs),
        };

        let sighash = sighash_forkid(
            &raw_tx,
            0,
            &prev_script,
            5000,
            SIGHASH_ALL_FORKID,
            &cache,
        );

        assert_eq!(sighash.len(), 32);
        // Should not be all zeros
        assert!(sighash.iter().any(|&b| b != 0));
    }

    #[test]
    fn test_build_script_sig_format() {
        // Simulate a DER signature (dummy) and pubkey
        let fake_der = vec![0x30, 0x44]; // minimal DER-ish
        let fake_pubkey = vec![0x02; 33]; // compressed pubkey placeholder

        let script_sig = build_script_sig(&fake_der, &fake_pubkey);

        // First byte: push length of (DER + hashtype byte) = 2 + 1 = 3
        assert_eq!(script_sig[0], 3);
        // DER signature bytes
        assert_eq!(&script_sig[1..3], &[0x30, 0x44]);
        // Hashtype byte
        assert_eq!(script_sig[3], 0x41);
        // Push length of pubkey = 33
        assert_eq!(script_sig[4], 33);
        // Pubkey bytes
        assert_eq!(script_sig[5..38], vec![0x02; 33][..]);
    }

    #[test]
    fn test_build_p2pkh_transaction_insufficient_funds() {
        let hd = HdWallet::new(TEST_MNEMONIC, BCH_DERIVATION_PATH, false).unwrap();
        let utxos = vec![Utxo {
            txid: "a".repeat(64),
            vout: 0,
            value: 100, // only 100 sats
            address_path: "0/0".to_string(),
        }];
        let outputs = vec![TxOutput {
            address: TEST_ADDRESS.to_string(),
            value: 10_000, // want 10k sats
        }];

        let result = build_p2pkh_transaction(&utxos, &outputs, TEST_ADDRESS, &hd, 1.2);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("insufficient") || err.contains("short"));
    }

    #[test]
    fn test_build_p2pkh_transaction_success() {
        let hd = HdWallet::new(TEST_MNEMONIC, BCH_DERIVATION_PATH, false).unwrap();

        // Create a UTXO with enough value
        let utxos = vec![Utxo {
            txid: "ab".repeat(32), // 64 hex chars
            vout: 0,
            value: 100_000, // 100k sats
            address_path: "0/0".to_string(),
        }];

        let outputs = vec![TxOutput {
            address: TEST_ADDRESS.to_string(),
            value: 10_000,
        }];

        let change_address = TEST_ADDRESS;
        let result = build_p2pkh_transaction(&utxos, &outputs, change_address, &hd, 1.2);
        assert!(result.is_ok(), "build failed: {:?}", result.err());

        let tx = result.unwrap();
        assert!(!tx.hex.is_empty());
        assert_eq!(tx.txid.len(), 64);
        assert!(tx.fee > 0);
        assert!(tx.fee < 1000); // reasonable fee for a simple tx

        // Verify the hex decodes back to valid bytes
        let bytes = hex::decode(&tx.hex).unwrap();
        // Version should be 2
        assert_eq!(&bytes[0..4], &[0x02, 0x00, 0x00, 0x00]);
    }

    #[test]
    fn test_build_p2pkh_transaction_no_utxos() {
        let hd = HdWallet::new(TEST_MNEMONIC, BCH_DERIVATION_PATH, false).unwrap();
        let result = build_p2pkh_transaction(&[], &[TxOutput {
            address: TEST_ADDRESS.to_string(),
            value: 1000,
        }], TEST_ADDRESS, &hd, 1.2);
        assert!(result.is_err());
    }

    #[test]
    fn test_build_p2pkh_transaction_no_outputs() {
        let hd = HdWallet::new(TEST_MNEMONIC, BCH_DERIVATION_PATH, false).unwrap();
        let utxos = vec![Utxo {
            txid: "ab".repeat(32),
            vout: 0,
            value: 100_000,
            address_path: "0/0".to_string(),
        }];
        let result = build_p2pkh_transaction(&utxos, &[], TEST_ADDRESS, &hd, 1.2);
        assert!(result.is_err());
    }

    #[test]
    fn test_build_p2pkh_transaction_exact_amount_no_change() {
        let hd = HdWallet::new(TEST_MNEMONIC, BCH_DERIVATION_PATH, false).unwrap();

        // Calculate what amount + fee would be for 1-in/1-out (no change)
        let size = estimate_tx_size(1, 1);
        let fee = (size as f64 * 1.2).ceil() as u64;
        let send_amount = 10_000u64;
        let utxo_value = send_amount + fee;

        let utxos = vec![Utxo {
            txid: "cd".repeat(32),
            vout: 0,
            value: utxo_value,
            address_path: "0/0".to_string(),
        }];
        let outputs = vec![TxOutput {
            address: TEST_ADDRESS.to_string(),
            value: send_amount,
        }];

        let result = build_p2pkh_transaction(&utxos, &outputs, TEST_ADDRESS, &hd, 1.2);
        assert!(result.is_ok(), "build failed: {:?}", result.err());

        let tx = result.unwrap();
        // Fee should be exactly what we calculated (or slightly more if change was sub-dust)
        assert!(tx.fee >= fee);
    }

    #[test]
    fn test_utxo_selection_greedy_descending() {
        let hd = HdWallet::new(TEST_MNEMONIC, BCH_DERIVATION_PATH, false).unwrap();

        // Provide multiple UTXOs of varying sizes
        let utxos = vec![
            Utxo {
                txid: "11".repeat(32),
                vout: 0,
                value: 500,
                address_path: "0/0".to_string(),
            },
            Utxo {
                txid: "22".repeat(32),
                vout: 0,
                value: 50_000,
                address_path: "0/0".to_string(),
            },
            Utxo {
                txid: "33".repeat(32),
                vout: 0,
                value: 1_000,
                address_path: "0/0".to_string(),
            },
        ];

        let outputs = vec![TxOutput {
            address: TEST_ADDRESS.to_string(),
            value: 10_000,
        }];

        // The 50k UTXO alone should be enough
        let result = build_p2pkh_transaction(&utxos, &outputs, TEST_ADDRESS, &hd, 1.2);
        assert!(result.is_ok());

        let tx = result.unwrap();
        // Fee for 1-in/2-out (with change)
        let expected_fee = (estimate_tx_size(1, 2) as f64 * 1.2).ceil() as u64;
        assert_eq!(tx.fee, expected_fee);
    }

    #[test]
    fn test_cashaddr_decode_roundtrip() {
        // Verify our CashAddress decoder extracts the correct pkhash
        let pkhash = decode_cashaddr_to_pkhash(TEST_ADDRESS).unwrap();

        // Cross-check with the crypto module
        let expected = crypto::pubkey_to_pkhash(
            "02bbe7dbcdf8b2261530a867df7180b17a90b482f74f2736b8a30d3f756e42e217",
        )
        .unwrap();
        assert_eq!(pkhash, expected);
    }
}
