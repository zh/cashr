# cashr

A fast, lightweight Bitcoin Cash wallet CLI. Manage BCH, CashTokens (fungible + NFTs), and pay for HTTP resources via the x402 protocol — all from your terminal.

## Features

- **Multi-wallet** — create, import, and switch between named wallets
- **BCH send/receive** — with QR codes, send-all (drain), and satoshi/BCH units
- **CashTokens** — list, send, and receive fungible tokens and NFTs
- **x402 protocol** — check and pay for HTTP resources with BCH micropayments
- **Transaction history** — with token transfer indicators
- **Mainnet + chipnet** — network auto-detected per wallet
- **Local key management** — private keys never leave your machine

## Install

### From source

```bash
git clone https://github.com/zh/cashr.git
cd cashr
cargo install --path .
```

Requires Rust 1.70+.

## Quick Start

```bash
# Create a wallet (mainnet)
cashr wallet create mywallet

# Create a chipnet (testnet) wallet
cashr wallet create testnet --chipnet

# Check balance
cashr balance

# Receive BCH (shows QR code)
cashr receive

# Send BCH
cashr send bitcoincash:qr... 0.001

# List tokens
cashr token list

# View transaction history
cashr history
```

## Wallet Management

```bash
# Create a new wallet with a 12-word seed phrase
cashr wallet create <name> [--chipnet]

# Import an existing wallet
cashr wallet import <name> [--chipnet]

# List all wallets (shows network + address for each)
cashr wallet list

# Show wallet info (address, token address, balance)
cashr wallet info

# Switch default wallet
cashr wallet default <name>

# Export seed phrase
cashr wallet export

# Delete a wallet
cashr wallet delete <name>
```

Use `-n <name>` with any command to target a specific wallet:

```bash
cashr -n mywallet balance
cashr -n testnet token list
```

### Network Detection

The network (mainnet or chipnet) is stored when you create or import a wallet. All subsequent commands auto-detect the network — no need to pass `--chipnet` every time.

```bash
cashr wallet create prod                   # mainnet wallet
cashr wallet create dev --chipnet          # chipnet wallet
cashr -n dev balance                       # auto-detects chipnet
cashr -n prod send bitcoincash:q... 0.01   # auto-detects mainnet
```

## Balance & Addresses

```bash
# BCH balance
cashr balance
cashr balance --sats

# Token balance
cashr balance --token <category-id>

# Derive address at index
cashr address derive [--index N] [--token]

# List first N addresses
cashr address list [--count N] [--token]

# Receive address with QR code
cashr receive
cashr receive --token            # token-aware z-prefix address
cashr receive --no-qr            # suppress QR code
```

## Sending BCH

```bash
# Send BCH (amount in BCH)
cashr send <address> <amount>

# Send in satoshis
cashr send <address> 10000 --unit sats

# Send all (drain wallet)
cashr send-all <address>
```

## CashTokens

```bash
# List all tokens (fungible + NFTs)
cashr token list

# Token info with BCMR metadata
cashr token info <category-id>

# Send fungible tokens
cashr token send <address> <amount> --token <category-id>

# Send an NFT
cashr token send-nft <address> --token <category-id> \
  --commitment <hex> --capability none
```

### NFT Capabilities

| Capability | Meaning |
|-----------|---------|
| `none` | Immutable — cannot be modified |
| `mutable` | Commitment can be updated when spent |
| `minting` | Can create new NFTs in the collection |

## Transaction History

```bash
# Full history
cashr history

# Filter by direction
cashr history --record-type incoming
cashr history --record-type outgoing

# Filter by token
cashr history --token <category-id>

# Pagination
cashr history --page 2
```

Token transactions show transfer details:

```
    IN  +0.00001 BCH
         +30 tokens [ea38c6a2...3b202b]
         2026-04-04 07:53 UTC
         b86f39fcec...5787360424

   OUT  -0.00227 BCH
         -1 NFT [909427e2...e9c2a9]
         2026-04-04 07:56 UTC
         501724359d...b1ea4f78
```

## x402 Protocol

The [x402 protocol](https://github.com/anthropics/x402) enables HTTP resources to require cryptocurrency micropayments. cashr supports x402-bch natively.

### Check a URL

```bash
# See if a URL requires payment
cashr check https://api.example.com/resource

# JSON output
cashr check https://api.example.com/resource --json
```

### Pay for a URL

```bash
# Pay and get the resource
cashr pay https://api.example.com/resource

# Skip confirmation prompt
cashr pay https://api.example.com/resource --confirmed

# Dry run (show cost without paying)
cashr pay https://api.example.com/resource --dry-run

# Custom HTTP method and headers
cashr pay https://api.example.com/data \
  -X POST \
  -H "Content-Type: application/json" \
  -d '{"query": "test"}' \
  --confirmed

# JSON output
cashr pay https://api.example.com/resource --json --confirmed
```

## Architecture

### Security Model

cashr keeps your private keys local:

- **Read operations** (balance, UTXOs, tokens) — queried via local electrumx server or remote REST API
- **Token metadata** (name, symbol, decimals) — fetched from Watchtower / Paytaca BCMR registries
- **Transaction signing** — done locally using your HD wallet keys
- **Broadcast** — signed raw transaction hex sent via electrumx or REST API
- **Key material** — stored in `~/.cashr/wallets/` with 0600 permissions, never sent to any server

### How It Works

```
                  ┌─────────────┐
                  │  cashr CLI  │
                  └──────┬──────┘
                         │
         ┌───────────────┼───────────────┐
         │               │               │
  Local HD Wallet    Electrumx       External
  (key derivation)   (balance)       (history → REST)
  (tx signing)       (UTXOs)         (token names → BCMR)
  (tx building)      (CashTokens)
                     (broadcast)
```

When a local [fulcrum-rust](https://github.com/user/fulcrum-rust) server is configured, cashr uses it for all balance, UTXO, CashToken, and broadcast operations via Electrum protocol 1.5. The mainnet-cash REST API is only used for transaction history (connected lazily on first `cashr history` call). Token metadata (name, symbol, decimals) is fetched from Watchtower and Paytaca BCMR registries.

Without electrumx configured, all operations fall back to the mainnet-cash REST API.

### Wallet Storage

```
~/.cashr/
  wallets/
    mywallet          # 12-word mnemonic (plaintext, 0600 perms)
    mywallet.net      # network: "mainnet" or "chipnet"
    default           # name of the default wallet
  servers.toml        # optional: server configuration
```

### Dependencies

- **[fulcrum-rust](https://github.com/user/fulcrum-rust)** — local Electrum REST gateway with CashToken support (protocol 1.5)
- **[mainnet-cash](https://rest-unstable.mainnet.cash)** — REST API fallback for transaction history
- **Watchtower / Paytaca BCMR** — token metadata registries
- Local transaction building with `secp256k1` + `SIGHASH_FORKID`
- BIP39/BIP32/BIP44 HD wallet derivation
- CashToken prefix encoding (CHIP-2022-02)

## Configuration

### Environment Variables

| Variable | Description | Default |
|----------|-------------|---------|
| `CASHR_HOME` | Wallet storage directory | `~/.cashr` |

### Server Configuration

Create `~/.cashr/servers.toml` to configure backends:

```toml
[electrumx]
# Local fulcrum-rust instances (tried in order, first success wins)
mainnet = ["http://localhost:3001"]
chipnet = ["http://localhost:3002"]

[rest]
# Mainnet-cash REST API (only used for transaction history)
servers = ["https://rest-unstable.mainnet.cash"]
```

Without this file, cashr falls back to the mainnet-cash REST API for all operations.

## License

MIT
