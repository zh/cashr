# CashTokens (CHIP-2022-02) — Complete Reference

**Spec**: CHIP-2022-02-CashTokens v2.2.2 (Final)
**Activated**: May 15, 2023 (mainnet MTP `1684152000`)

---

## 1. Overview

CashTokens adds two token primitives to Bitcoin Cash:

- **Fungible Tokens (FT)** — divisible, interchangeable amounts (like ERC-20)
- **Non-Fungible Tokens (NFT)** — unique tokens with optional commitment data and capability levels (like ERC-721 but more powerful)

Both types are encoded directly in transaction outputs via a **token prefix** (byte `0xef`) prepended to the locking bytecode. No new transaction types or opcodes for creation — tokens use standard P2PKH/P2SH outputs.

---

## 2. Token Prefix Binary Format

The token prefix is inserted BEFORE the locking bytecode in an output. The output's `scriptPubKey_length` (CompactSize) covers **both** the token prefix and the locking bytecode.

### Serialized Output Layout

```
<value>          8 bytes LE (satoshis)
<length>         CompactSize (covers token_prefix + locking_bytecode)
[token_prefix]   Variable (only if output has tokens)
<locking_bytecode>  Standard P2PKH/P2SH script
```

### Token Prefix Structure

```
Byte(s)    Field                 Description
───────    ─────                 ───────────
[0]        PREFIX_TOKEN          0xef (239) — fixed marker byte
[1-32]     category_id           32 bytes, LE byte order (same as txid)
[33]       token_bitfield        Flags + NFT capability (see below)
[34+]      commitment_length     CompactSize (conditional: HAS_COMMITMENT_LENGTH)
[...]      commitment            Raw bytes (conditional: HAS_COMMITMENT_LENGTH)
[...]      ft_amount             CompactSize (conditional: HAS_AMOUNT)
```

### Token Bitfield (1 byte)

```
Bit   Hex    Name                    Meaning
───   ───    ────                    ───────
7     0x80   RESERVED_BIT            MUST be 0 (reserved for future use)
6     0x40   HAS_COMMITMENT_LENGTH   Commitment length + data follows
5     0x20   HAS_NFT                 Output contains an NFT
4     0x10   HAS_AMOUNT              Fungible token amount follows
3-0   0x0f   nft_capability          NFT capability (only meaningful if HAS_NFT)
                                       0x00 = none (immutable)
                                       0x01 = mutable
                                       0x02 = minting
                                       0x03+ = reserved (invalid)
```

### Encoding Examples

| Scenario | Bitfield | After bitfield |
|----------|----------|----------------|
| FT only, 100 tokens | `0x10` | `64` (varint 100) |
| FT only, 253 tokens | `0x10` | `fd fd00` (varint 253) |
| Immutable NFT, no commitment | `0x20` | (nothing) |
| Immutable NFT + commitment `ff00` | `0x60` | `02 ff00` (len=2, data) |
| Mutable NFT, no commitment | `0x21` | (nothing) |
| Minting NFT, no commitment | `0x22` | (nothing) |
| Minting NFT + commitment `01` + 500 FT | `0x72` | `01 01 fdf401` |
| Immutable NFT + no commitment + 1 FT | `0x30` | `01` (varint 1) |

### CompactSize (varint) Encoding

Same encoding as Bitcoin's CompactSize used throughout the protocol:

| Value Range | Encoding |
|-------------|----------|
| 0-252 | 1 byte: value directly |
| 253-65535 | `0xfd` + 2 bytes LE |
| 65536-4294967295 | `0xfe` + 4 bytes LE |
| 4294967296+ | `0xff` + 8 bytes LE |

**Must be minimally encoded** (consensus rule). Value `1` MUST be `0x01`, NOT `0xfd0100`.

### Limits

- **Max commitment length**: 40 bytes
- **Max fungible amount**: 9,223,372,036,854,775,807 (`0x7fffffffffffffff`, max signed i64)
- **Min fungible amount in prefix**: 1 (zero means no FT, use HAS_AMOUNT=0)
- **Min commitment length if HAS_COMMITMENT_LENGTH**: 1 (zero-length = don't set the flag)

---

## 3. Token Category

### Derivation

A token **category ID** is a 32-byte transaction hash. It is determined at genesis:

- The category ID equals the **outpoint transaction hash** of an input in the genesis transaction whose **outpoint index is 0**.
- Only inputs spending output 0 of their parent transaction can create new categories.
- A single genesis transaction can create multiple categories (one per qualifying input).

**In practice**: Create a preliminary tx with output 0 going to your address. Then spend that output 0 in the genesis tx — its parent txid becomes the category ID.

### Byte Order

Category ID is stored in **internal byte order** (little-endian), same as outpoint transaction hashes in the Bitcoin P2P protocol. When displayed as hex, it's byte-reversed (same as txid display).

---

## 4. Token Types

### Fungible Tokens (FT)

- Created entirely at genesis — total supply is fixed forever
- Amount per UTXO encoded as CompactSize in token prefix
- Can be split across multiple outputs (conservation rule: input sum >= output sum)
- Can be burned by omitting from outputs
- No minting of new FTs after genesis (minting NFTs cannot create FTs)
- Decimals are a display convention (defined in BCMR metadata), not on-chain

### Non-Fungible Tokens (NFT)

Three capability levels forming a hierarchy:

| Capability | Value | Can Create | Can Modify |
|------------|-------|------------|------------|
| **minting** | `0x02` | Unlimited NFTs of any capability + any commitment | Yes |
| **mutable** | `0x01` | One NFT with any commitment (mutable or immutable) | Yes |
| **none** (immutable) | `0x00` | Nothing | No — must pass through unchanged or burn |

- **Commitment**: 0-40 bytes of arbitrary on-chain data
- **Zero-length commitment**: `HAS_COMMITMENT_LENGTH` is NOT set
- Each output can hold at most **one NFT** (but can combine with any FT amount of the same category)

### Combined FT + NFT

A single output can hold both a fungible amount AND an NFT, provided they share the same category. The bitfield has both `HAS_AMOUNT` and `HAS_NFT` set.

---

## 5. Token Validation Rules

### Genesis Rules

- At least one input must have outpoint_index = 0
- All FT supply for a category must be created in the genesis tx
- NFTs of any capability can be created at genesis
- Multiple categories can be created in one tx

### Spending Rules (Token Conservation)

For each category across a transaction:

1. **Fungible**: `sum(output_amounts) <= sum(input_amounts)` (or genesis category)
2. **Minting NFTs**: Output minting categories must be available as input minting or genesis categories
3. **Mutable NFTs**: If category has input minting capability, unlimited mutable outputs allowed. Otherwise, `count(output_mutable) <= count(input_mutable)`
4. **Immutable NFTs**: If category has input minting capability, unlimited immutable outputs allowed. Otherwise, each output immutable NFT must match an input immutable NFT (same category + commitment), OR consume a mutable token
5. **Burning**: Any token can be burned by omitting from outputs (implicit)

### Consensus Rules

- Coinbase transactions cannot include token prefixes
- `RESERVED_BIT` (0x80) must be unset
- Capability values > 0x02 are invalid
- Token prefix encoding no tokens (neither HAS_NFT nor HAS_AMOUNT) is invalid
- `HAS_COMMITMENT_LENGTH` without `HAS_NFT` is invalid
- All CompactSize values must be minimally encoded

---

## 6. Transaction Signing with Tokens

### Standard SIGHASH_FORKID (0x41)

For outputs: token prefix is part of the serialized output, so `hashOutputs` automatically covers token data.

For inputs: outpoints commit to the UTXO being spent (which implicitly commits to its token data since it's identified by txid:vout).

**No changes needed for basic P2PKH token transactions.** The existing BIP143-style SIGHASH_FORKID works correctly.

### New: SIGHASH_UTXOS (0x20)

Optional flag added by CashTokens CHIP:

- When set, adds `hashUtxos` to the signing preimage (after `hashPrevouts`)
- `hashUtxos` = double-SHA256 of all input UTXOs serialized in input order (including their token prefixes)
- Must be combined with `SIGHASH_FORKID` (0x40)
- Must NOT be combined with `SIGHASH_ANYONECANPAY`
- Recommended for multi-party transactions (covenants, atomic swaps)
- Not required for simple P2PKH sends

### Token Prefix in Signing Serialization

When evaluating a UTXO that includes tokens, the full token prefix (`0xef` + token data) is inserted before `coveredBytecode` in the sighash preimage. This ensures the signature commits to the token data of the input being spent.

---

## 7. Token-Aware Addresses

CashAddress types extended with two new address types:

| Type | Prefix Letter | Meaning |
|------|---------------|---------|
| P2PKH | `q` (mainnet) / `q` (chipnet) | Regular BCH address |
| P2SH | `p` (mainnet) / `p` (chipnet) | Regular script hash |
| **P2PKH + Tokens** | **`z`** (mainnet) / **`z`** (chipnet) | Token-aware P2PKH |
| **P2SH + Tokens** | **`r`** (mainnet) / **`r`** (chipnet) | Token-aware P2SH |

- Same 20-byte pubkey hash, different address type flag
- `z`-prefix and `q`-prefix addresses produce identical P2PKH scripts
- Token-aware wallets SHOULD refuse to send tokens to non-token-aware addresses
- Receiving tokens at a `q`-prefix address works at the consensus level but wallets may not display them

---

## 8. New VM Opcodes

Six introspection opcodes for inspecting tokens in Script:

| Opcode | Hex | Description |
|--------|-----|-------------|
| `OP_UTXOTOKENCATEGORY` | `0xce` | Push category ID (+ `0x02` if minting, `0x01` if mutable) of input UTXO |
| `OP_UTXOTOKENCOMMITMENT` | `0xcf` | Push NFT commitment of input UTXO |
| `OP_UTXOTOKENAMOUNT` | `0xd0` | Push FT amount of input UTXO |
| `OP_OUTPUTTOKENCATEGORY` | `0xd1` | Push category ID (+ capability byte) of output |
| `OP_OUTPUTTOKENCOMMITMENT` | `0xd2` | Push NFT commitment of output |
| `OP_OUTPUTTOKENAMOUNT` | `0xd3` | Push FT amount of output |

These enable on-chain covenants that verify token properties.

---

## 9. BCMR — Bitcoin Cash Metadata Registries

### Overview

BCMR is the standard for associating human-readable metadata (name, symbol, decimals, icons) with token categories. It's an off-chain JSON document referenced on-chain via OP_RETURN.

**Schema**: `https://cashtokens.org/bcmr-v2.schema.json`

### Registry JSON Structure

```json
{
  "$schema": "https://cashtokens.org/bcmr-v2.schema.json",
  "version": { "major": 2, "minor": 0, "patch": 0 },
  "latestRevision": "2024-01-15T00:00:00.000Z",
  "registryIdentity": {
    "name": "My Token Registry"
  },
  "identities": {
    "<authbase-txid-hex>": {
      "<ISO-8601-timestamp>": {
        "name": "Token Name",
        "description": "What this token does",
        "token": {
          "category": "<category-id-hex>",
          "symbol": "SYMB",
          "decimals": 8,
          "nfts": {
            "description": "NFT collection description",
            "parse": {
              "types": {
                "0": { "name": "Common", "uris": { "icon": "ipfs://..." } },
                "1": { "name": "Rare", "uris": { "icon": "ipfs://..." } }
              }
            }
          }
        },
        "uris": {
          "icon": "ipfs://QmXyz.../icon.png",
          "web": "https://mytoken.com",
          "support": "https://mytoken.com/support"
        },
        "tags": ["fungible-token"]
      }
    }
  }
}
```

### Key Fields

| Field | Description |
|-------|-------------|
| `identities` | Map of authbase txid -> timestamp -> identity snapshot |
| `name` | Human-readable token name |
| `token.symbol` | Ticker symbol (regex: `^[A-Z0-9]+[-A-Z0-9]*$`) |
| `token.decimals` | Display decimal places (default: 0) |
| `token.category` | 64-char hex category ID |
| `uris.icon` | Token icon URL (IPFS, HTTPS, or data URI) |
| `uris.web` | Project website |
| `token.nfts` | NFT collection config (types, parsing rules) |

### On-Chain Publication

Metadata is published via an OP_RETURN transaction:

```
OP_RETURN <"BCMR"> <sha256-hash-of-registry> [<uri> ...]
```

Bytecode: `0x6a 04 42434d52 20 <32-byte-hash> <uri-push>`

The URI points to the BCMR JSON (typically on IPFS).

### Authchain

- **Authbase**: The token category ID (genesis txid)
- **Authchain**: Series of transactions starting from authbase, each spending output 0 of the previous
- **Authhead**: The latest unspent transaction in the authchain
- Metadata updates are published as new authchain transactions
- Wallets follow the authchain forward to find the latest metadata

### Resolution Methods

1. **Chain-resolved**: Follow authchain from authbase, find OP_RETURN with BCMR data
2. **DNS-resolved**: `https://<domain>/.well-known/bitcoin-cash-metadata-registry.json`
3. **Embedded**: Built into wallet software (for well-known tokens)

---

## 10. Token Icons

### Storage Formats

Icons can be stored as:
- **IPFS URI**: `ipfs://QmXyz.../icon.png` (preferred for decentralization)
- **HTTPS URL**: `https://example.com/icon.png`
- **Data URI**: `data:image/png;base64,...` (for small icons)

### Recommendations

- Format: PNG or SVG
- Size: 256x256 to 512x512 pixels (square)
- No enforced limits in spec, but wallets may resize
- SVG preferred for scalability

### Resolution Flow

1. Wallet fetches BCMR registry (from authchain or DNS)
2. Reads `uris.icon` from the latest identity snapshot
3. Resolves the URI (IPFS gateway, HTTP fetch, or inline data)
4. Displays in wallet UI

### Watchtower Caching

Watchtower.cash indexes BCMR on-chain data and caches metadata:
- `GET /cashtokens/fungible/{category}/` returns `image_url` field
- Watchtower follows authchains and parses BCMR registries
- Wallets can use watchtower as a metadata API instead of resolving directly

---

## 11. Supply Definitions

| Term | Meaning |
|------|---------|
| **Genesis Supply** | Total FT created at genesis (immutable forever) |
| **Total Supply** | Sum of FT across all unspent UTXOs for the category |
| **Reserved Supply** | FT in UTXOs that also contain a minting/mutable NFT of same category |
| **Circulating Supply** | Total Supply - Reserved Supply |

All bounded by max VM number: 9,223,372,036,854,775,807.

---

## 12. Dust Limits

- Regular BCH output: **546 sats** (consensus minimum)
- Token output: **800 sats** (common wallet convention for safety margin)

Token outputs are slightly larger due to the prefix, but BCH's dust limit is a fixed 546 sats regardless of output size (unlike Bitcoin Core's dynamic calculation).

---

## 13. Watchtower Token API

### Endpoints

| Method | Path | Description |
|--------|------|-------------|
| GET | `/cashtokens/fungible/?wallet_hash=X&has_balance=true` | List FTs with balance |
| GET | `/cashtokens/fungible/{category}/` | Single token metadata |
| GET | `/utxo/wallet/{hash}/?is_cashtoken=true` | Token UTXOs for wallet |
| GET | `/balance/wallet/{hash}/{token_id}/` | Token balance |
| GET | `/utxo/wallet/{hash}/scan/?background=true` | Trigger UTXO rescan |

### Fungible Token Response

```json
{
  "id": "ct/af4d1c0b61ce53b23a2299c8fb66d902f066644af08d60728c0b1b205df549ae",
  "name": "AWE Token",
  "symbol": "AWE",
  "decimals": 0,
  "image_url": "ipfs://QmIcon...",
  "balance": 20.0
}
```

### Token UTXO Response

```json
{
  "txid": "048b841d...",
  "vout": 0,
  "value": 1000,
  "tokenid": "af4d1c0b...",
  "amount": 20,
  "commitment": null,
  "capability": null,
  "is_cashtoken": true,
  "address_path": "0/0",
  "wallet_index": "0"
}
```

Fields:
- `tokenid`: category hex (64 chars)
- `amount`: fungible token amount (float in watchtower)
- `commitment`: hex string or null
- `capability`: `"none"`, `"mutable"`, `"minting"`, or null (null = fungible only)
- `is_cashtoken`: boolean flag
- `address_path` / `wallet_index`: for key derivation during signing

---

## 14. Creating Tokens

### Genesis Transaction Structure

A genesis transaction must:
1. Have at least one input spending output 0 of a previous tx
2. Include token-prefixed outputs with the new category
3. Set ALL fungible supply in genesis outputs
4. Optionally include minting NFT for future NFT creation

### Using cashtokens.studio (Web UI)

1. Connect wallet via WalletConnect (e.g., Paytaca mobile)
2. "Create New Token" flow
3. Set: name, symbol, decimals, supply, icon
4. Studio builds genesis tx + BCMR OP_RETURN
5. Sign and broadcast

### Using cashtoken-sdk (Programmatic)

```typescript
import { CashTokenSDK } from 'cashtoken-sdk';

const sdk = new CashTokenSDK({ network: 'mainnet' });

// Create fungible token
const result = await sdk.createFungibleToken({
  supply: BigInt(21_000_000_00000000),  // 21M with 8 decimals
  metadata: {
    name: 'My Token',
    symbol: 'MTK',
    decimals: 8,
    icon: './icon.png',     // auto-uploaded to IPFS
    web: 'https://mytoken.com',
  },
});
// result.categoryId = genesis txid
```

---

## 15. Differences from Standard BCH Transactions

| Aspect | Standard BCH | CashToken BCH |
|--------|-------------|---------------|
| Output script | `<length> <scriptPubKey>` | `<length> [0xef token_data] <scriptPubKey>` |
| Address type | `q`/`p` prefix | `z`/`r` prefix (token-aware) |
| Signing | SIGHASH_FORKID | Same + optional SIGHASH_UTXOS (0x20) |
| VM opcodes | Standard set | + 6 token introspection opcodes |
| Coinbase | Normal | Cannot include token prefixes |
| Output sorting (BIP69) | By value, script | Extended: value, script, then token fields |

---

## 16. Commitment Encoding Patterns

### Simple Index (Sequential NFTs)

Commitment = VM number index into BCMR `parse.types`:
- `0x00` = type "0" (e.g., "Common")
- `0x01` = type "1" (e.g., "Rare")

### Structured Data (Parsable NFTs)

Up to 40 bytes encoding multiple fields:

```
[0-3]    timestamp      uint32 LE
[4-11]   amount         uint64 LE
[12-19]  price          uint64 LE
[20-39]  identifier     20 bytes
```

BCMR defines parsing rules via `parse.bytecode` (BCH VM script) and `parse.fields`.

### Empty Commitment

Zero-length commitment = no `HAS_COMMITMENT_LENGTH` flag, just `HAS_NFT`.
Used for simple "proof of ownership" NFTs without data.

---

## 17. Key Implementation Files

### paytaca-rust (Rust wallet)
- `src/transaction/mod.rs` — Token prefix encoding, tx builder
- `src/wallet/bch.rs` — `send_token()`, `send_nft()` implementations
- `src/watchtower/client.rs` — Token API client
- `src/cli/token.rs` — CLI commands (list, info, send, send-nft)

### paytaca-cli (JS wallet)
- `src/commands/token.ts` — CLI command handlers
- `src/wallet/bch.ts` — `sendToken()`, `sendNft()`, `getNftUtxos()`
- `node_modules/watchtower-cash-js/dist/bch/index.js` — tx building via libauth

### cashtoken-sdk (JS SDK)
- `src/genesis.ts` — Token creation
- `src/transfer.ts` — Token sending
- `src/mint.ts` — NFT minting
- `src/burn.ts` — Token burning
- `src/metadata.ts` — BCMR registry building + on-chain publishing
- `src/types.ts` — All type definitions
