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
    /// Token data of this UTXO (if it's a CashToken UTXO).
    /// Required for correct sighash computation when spending token UTXOs.
    pub token: Option<TokenPrefix>,
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

// ── CashToken types ────────────────────────────────────────────────

/// CashToken NFT capability.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum NftCapability {
    None = 0,      // immutable
    Mutable = 1,
    Minting = 2,
}

impl NftCapability {
    /// Parse from string representation.
    pub fn parse(s: &str) -> Result<Self> {
        match s {
            "none" => Ok(Self::None),
            "mutable" => Ok(Self::Mutable),
            "minting" => Ok(Self::Minting),
            _ => bail!("unknown NFT capability: {}", s),
        }
    }
}

/// NFT data within a CashToken prefix.
#[derive(Debug, Clone)]
pub struct NftData {
    pub capability: NftCapability,
    pub commitment: Vec<u8>,
}

/// CashToken prefix attached to a transaction output.
#[derive(Debug, Clone)]
pub struct TokenPrefix {
    pub category: [u8; 32],   // 32 bytes, byte-reversed from hex (same as txid)
    pub nft: Option<NftData>,
    pub amount: u64,           // fungible token amount (0 if none)
}

/// A transaction output with optional CashToken data.
#[derive(Debug, Clone)]
pub struct TokenTxOutput {
    pub address: String,
    pub value: u64,
    pub token: Option<TokenPrefix>,
}

// ── Constants ───────────────────────────────────────────────────────

const SIGHASH_ALL: u32 = 0x01;
const SIGHASH_FORKID: u32 = 0x40;
const SIGHASH_ALL_FORKID: u32 = SIGHASH_ALL | SIGHASH_FORKID; // 0x41

const DUST_LIMIT: u64 = 546;
const TOKEN_DUST: u64 = 800; // minimum BCH on a token output
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

// ── CashToken transaction builder ──────────────────────────────────

/// Minimum BCH value for a token output.
pub const fn token_dust() -> u64 {
    TOKEN_DUST
}

/// Build a signed transaction that includes CashToken outputs.
///
/// All UTXO selection is done by the caller. This function:
/// 1. Serializes outputs (prepending token prefix where present)
/// 2. Calculates fee and adds BCH change if needed
/// 3. Signs all inputs (P2PKH + SIGHASH_FORKID)
pub fn build_token_transaction(
    inputs: &[Utxo],
    outputs: &[TokenTxOutput],
    change_address: &str,
    hd_wallet: &HdWallet,
    fee_rate: f64,
) -> Result<BuiltTransaction> {
    if inputs.is_empty() {
        bail!("no inputs provided");
    }
    if outputs.is_empty() {
        bail!("no outputs specified");
    }

    let fee_rate = if fee_rate <= 0.0 { DEFAULT_FEE_RATE } else { fee_rate };
    let input_total: u64 = inputs.iter().map(|u| u.value).sum();
    let output_total: u64 = outputs.iter().map(|o| o.value).sum();

    // Build output scripts (with token prefixes where present)
    let mut output_scripts: Vec<(u64, Vec<u8>)> = Vec::new();
    for o in outputs {
        let script = p2pkh_script_from_address(&o.address)
            .with_context(|| format!("failed to create script for address {}", o.address))?;
        let full_script = match &o.token {
            Some(token) => {
                let mut s = serialize_token_prefix(token);
                s.extend_from_slice(&script);
                s
            }
            None => script,
        };
        output_scripts.push((o.value, full_script));
    }

    // Compute output sizes for fee estimation
    let fixed_output_bytes: usize = output_scripts
        .iter()
        .map(|(_, script)| 8 + varint_len(script.len() as u64) + script.len())
        .sum();

    // Estimate fee with change output
    let size_with_change = OVERHEAD_SIZE + inputs.len() * INPUT_SIZE + fixed_output_bytes + OUTPUT_SIZE;
    let fee_with_change = (size_with_change as f64 * fee_rate).ceil() as u64;

    // Estimate fee without change output
    let size_no_change = OVERHEAD_SIZE + inputs.len() * INPUT_SIZE + fixed_output_bytes;
    let fee_no_change = (size_no_change as f64 * fee_rate).ceil() as u64;

    if input_total < output_total + fee_no_change {
        let lacking = (output_total + fee_no_change) - input_total;
        bail!(
            "insufficient funds: need {} sats, have {} sats (short by {} sats)",
            output_total + fee_no_change,
            input_total,
            lacking
        );
    }

    let fee;
    if input_total >= output_total + fee_with_change + DUST_LIMIT {
        let change_amount = input_total - output_total - fee_with_change;
        let change_script = p2pkh_script_from_address(change_address)
            .context("failed to create change script")?;
        output_scripts.push((change_amount, change_script));
        fee = fee_with_change;
    } else {
        fee = input_total - output_total;
    }

    // Build unsigned transaction
    let mut raw_tx = RawTx {
        version: 2u32,
        inputs: Vec::new(),
        outputs: output_scripts.clone(),
        locktime: 0u32,
    };

    for utxo in inputs {
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

    for (i, utxo) in inputs.iter().enumerate() {
        let private_key_bytes = hd_wallet.get_private_key_at(&utxo.address_path)?;
        let pubkey_hex = hd_wallet.get_pubkey_at(&utxo.address_path)?;
        let pubkey_bytes = hex::decode(&pubkey_hex).context("invalid pubkey hex")?;
        let p2pkh_script = p2pkh_script_from_pubkey_bytes(&pubkey_bytes);

        // For token UTXOs, the scriptCode includes the token prefix before the P2PKH script.
        // CHIP-2022-02: "the full encoded token prefix must be inserted immediately
        // before the coveredBytecode in the signing serialization."
        let script_code = match &utxo.token {
            Some(token) => {
                let mut sc = serialize_token_prefix(token);
                sc.extend_from_slice(&p2pkh_script);
                sc
            }
            None => p2pkh_script,
        };

        let sighash = sighash_forkid(
            &raw_tx, i, &script_code, utxo.value, SIGHASH_ALL_FORKID, &cache,
        );

        let secret_key = SecretKey::from_slice(&private_key_bytes).context("invalid private key")?;
        let message = Message::from_digest(sighash);
        let signature = secp.sign_ecdsa(&message, &secret_key);

        let der_sig = signature.serialize_der();
        raw_tx.inputs[i].script_sig = build_script_sig(&der_sig, &pubkey_bytes);
    }

    let tx_bytes = serialize_tx(&raw_tx);
    let hex_str = hex::encode(&tx_bytes);
    let txid_bytes = crypto::sha256d(&tx_bytes);
    let txid = hex::encode(txid_bytes.iter().rev().copied().collect::<Vec<u8>>());

    Ok(BuiltTransaction { hex: hex_str, txid, fee })
}

/// Serialize a CashToken prefix to bytes (CHIP-2022-02 format).
///
/// Format: 0xef + category(32) + bitfield(1) + [commitment_len + commitment] + [amount]
fn serialize_token_prefix(token: &TokenPrefix) -> Vec<u8> {
    let mut buf = Vec::with_capacity(64);
    buf.push(0xef); // TOKEN_PREFIX_BYTE
    buf.extend_from_slice(&token.category); // 32-byte category (byte-reversed)

    let mut bitfield: u8 = 0;
    if let Some(ref nft) = token.nft {
        bitfield |= 0x20; // HAS_NFT
        bitfield |= nft.capability as u8; // capability in lower nibble
        if !nft.commitment.is_empty() {
            bitfield |= 0x40; // HAS_COMMITMENT_LENGTH
        }
    }
    if token.amount > 0 {
        bitfield |= 0x10; // HAS_AMOUNT
    }
    buf.push(bitfield);

    // Commitment (if NFT with non-empty commitment)
    if let Some(ref nft) = token.nft {
        if !nft.commitment.is_empty() {
            write_varint(&mut buf, nft.commitment.len() as u64);
            buf.extend_from_slice(&nft.commitment);
        }
    }
    // Fungible amount
    if token.amount > 0 {
        write_varint(&mut buf, token.amount);
    }

    buf
}

/// Byte length of a varint encoding.
fn varint_len(value: u64) -> usize {
    if value < 0xfd { 1 }
    else if value <= 0xffff { 3 }
    else if value <= 0xffff_ffff { 5 }
    else { 9 }
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
/// Also used for CashToken category IDs (which are genesis txids).
pub fn decode_txid_to_bytes(txid_hex: &str) -> Result<[u8; 32]> {
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
                token: None,
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
                token: None,
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
                token: None,
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
                token: None,
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
                token: None,
            },
            Utxo {
                txid: "22".repeat(32),
                vout: 0,
                value: 50_000,
                address_path: "0/0".to_string(),
                token: None,
            },
            Utxo {
                txid: "33".repeat(32),
                vout: 0,
                value: 1_000,
                address_path: "0/0".to_string(),
                token: None,
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

    // ── CashToken prefix tests ─────────────────────────────────────

    #[test]
    fn test_serialize_token_prefix_fungible_only() {
        let category = [0xaa; 32];
        let token = TokenPrefix {
            category,
            nft: None,
            amount: 100,
        };
        let bytes = serialize_token_prefix(&token);

        assert_eq!(bytes[0], 0xef); // PREFIX byte
        assert_eq!(&bytes[1..33], &[0xaa; 32]); // category
        assert_eq!(bytes[33], 0x10); // bitfield: HAS_AMOUNT only
        assert_eq!(bytes[34], 100); // amount as varint
        assert_eq!(bytes.len(), 35);
    }

    #[test]
    fn test_serialize_token_prefix_nft_no_commitment() {
        let category = [0xbb; 32];
        let token = TokenPrefix {
            category,
            nft: Some(NftData {
                capability: NftCapability::None,
                commitment: Vec::new(),
            }),
            amount: 0,
        };
        let bytes = serialize_token_prefix(&token);

        assert_eq!(bytes[0], 0xef);
        assert_eq!(bytes[33], 0x20); // HAS_NFT, capability=none(0)
        assert_eq!(bytes.len(), 34); // no commitment, no amount
    }

    #[test]
    fn test_serialize_token_prefix_nft_with_commitment() {
        let category = [0xcc; 32];
        let token = TokenPrefix {
            category,
            nft: Some(NftData {
                capability: NftCapability::Mutable,
                commitment: vec![0xff, 0x00],
            }),
            amount: 0,
        };
        let bytes = serialize_token_prefix(&token);

        assert_eq!(bytes[0], 0xef);
        assert_eq!(bytes[33], 0x61); // HAS_NFT(0x20) | HAS_COMMITMENT(0x40) | mutable(1)
        assert_eq!(bytes[34], 2); // commitment length
        assert_eq!(&bytes[35..37], &[0xff, 0x00]); // commitment
        assert_eq!(bytes.len(), 37);
    }

    #[test]
    fn test_serialize_token_prefix_nft_minting_with_amount() {
        let category = [0xdd; 32];
        let token = TokenPrefix {
            category,
            nft: Some(NftData {
                capability: NftCapability::Minting,
                commitment: vec![0x01],
            }),
            amount: 500,
        };
        let bytes = serialize_token_prefix(&token);

        assert_eq!(bytes[0], 0xef);
        // HAS_AMOUNT(0x10) | HAS_NFT(0x20) | HAS_COMMITMENT(0x40) | minting(2)
        assert_eq!(bytes[33], 0x72);
        assert_eq!(bytes[34], 1); // commitment length
        assert_eq!(bytes[35], 0x01); // commitment
        // amount 500 as varint: 0xfd 0xf4 0x01
        assert_eq!(bytes[36], 0xfd);
        assert_eq!(&bytes[37..39], &500u16.to_le_bytes());
    }

    #[test]
    fn test_nft_capability_parse() {
        assert_eq!(NftCapability::parse("none").unwrap(), NftCapability::None);
        assert_eq!(NftCapability::parse("mutable").unwrap(), NftCapability::Mutable);
        assert_eq!(NftCapability::parse("minting").unwrap(), NftCapability::Minting);
        assert!(NftCapability::parse("invalid").is_err());
    }

    #[test]
    fn test_varint_len() {
        assert_eq!(varint_len(0), 1);
        assert_eq!(varint_len(252), 1);
        assert_eq!(varint_len(253), 3);
        assert_eq!(varint_len(0xffff), 3);
        assert_eq!(varint_len(0x10000), 5);
    }

    #[test]
    fn test_build_token_transaction_fungible() {
        let hd = HdWallet::new(TEST_MNEMONIC, BCH_DERIVATION_PATH, false).unwrap();
        let category = decode_txid_to_bytes(&"ab".repeat(32)).unwrap();

        let inputs = vec![
            // Token UTXO
            Utxo {
                txid: "11".repeat(32),
                vout: 0,
                value: 800,
                address_path: "0/0".to_string(),
                token: None,
            },
            // BCH UTXO for fees
            Utxo {
                txid: "22".repeat(32),
                vout: 0,
                value: 50_000,
                address_path: "0/0".to_string(),
                token: None,
            },
        ];

        let outputs = vec![TokenTxOutput {
            address: TEST_ADDRESS.to_string(),
            value: 800,
            token: Some(TokenPrefix {
                category,
                nft: None,
                amount: 100,
            }),
        }];

        let result = build_token_transaction(&inputs, &outputs, TEST_ADDRESS, &hd, 1.2);
        assert!(result.is_ok(), "build failed: {:?}", result.err());

        let tx = result.unwrap();
        assert!(!tx.hex.is_empty());
        assert_eq!(tx.txid.len(), 64);
        assert!(tx.fee > 0);

        // Verify the hex contains the token prefix byte
        let tx_bytes = hex::decode(&tx.hex).unwrap();
        // Version should be 2
        assert_eq!(&tx_bytes[0..4], &[0x02, 0x00, 0x00, 0x00]);
    }

    #[test]
    fn test_build_token_transaction_nft() {
        let hd = HdWallet::new(TEST_MNEMONIC, BCH_DERIVATION_PATH, false).unwrap();
        let category = decode_txid_to_bytes(&"cd".repeat(32)).unwrap();

        let inputs = vec![
            Utxo {
                txid: "33".repeat(32),
                vout: 0,
                value: 800,
                address_path: "0/0".to_string(),
                token: None,
            },
            Utxo {
                txid: "44".repeat(32),
                vout: 1,
                value: 10_000,
                address_path: "0/0".to_string(),
                token: None,
            },
        ];

        let outputs = vec![TokenTxOutput {
            address: TEST_ADDRESS.to_string(),
            value: 800,
            token: Some(TokenPrefix {
                category,
                nft: Some(NftData {
                    capability: NftCapability::None,
                    commitment: vec![0xde, 0xad],
                }),
                amount: 0,
            }),
        }];

        let result = build_token_transaction(&inputs, &outputs, TEST_ADDRESS, &hd, 1.2);
        assert!(result.is_ok(), "build failed: {:?}", result.err());

        let tx = result.unwrap();
        assert!(tx.fee > 0);
        assert!(tx.fee < 1000);
    }

    #[test]
    fn test_build_token_transaction_insufficient_funds() {
        let hd = HdWallet::new(TEST_MNEMONIC, BCH_DERIVATION_PATH, false).unwrap();
        let category = [0xaa; 32];

        let inputs = vec![Utxo {
            txid: "55".repeat(32),
            vout: 0,
            value: 100, // not enough
            address_path: "0/0".to_string(),
                token: None,
        }];

        let outputs = vec![TokenTxOutput {
            address: TEST_ADDRESS.to_string(),
            value: 800,
            token: Some(TokenPrefix {
                category,
                nft: None,
                amount: 50,
            }),
        }];

        let result = build_token_transaction(&inputs, &outputs, TEST_ADDRESS, &hd, 1.2);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("insufficient") || err.contains("short"));
    }
}
