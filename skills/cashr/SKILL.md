---
name: cashr-x402
description: Handles x402 HTTP payment requests using Cashr BCH wallet. Use when encountering HTTP 402 responses or when calling APIs that may require payment via the x402 protocol.
---

# Cashr x402 Payment Handler

This skill teaches the agent how to handle x402-based HTTP payments using the Cashr CLI wallet.

## Overview

Some APIs (like nanogpt, etc.) use the x402 protocol for HTTP payments. When you call these APIs:
- Server returns **HTTP 402 PAYMENT REQUIRED**
- You must pay with BCH to access the resource
- After payment, the server returns the actual response

Cashr implements the **x402-bch v2.2 protocol** natively.

## Commands

### Check if a URL requires payment (recommended first step)

```bash
cashr check <url> --json
# Returns: whether payment is required, cost in sats, accepted networks, etc.
```

With custom method/headers/body:
```bash
cashr check <url> --method POST -H "Content-Type: application/json" -d '{"prompt":"hello"}' --json
```

### Preview payment without executing (dry-run)

```bash
cashr pay <url> --dry-run --json
# Shows what would happen without actually paying
```

### Make a paid request

```bash
# Basic (will prompt for confirmation)
cashr pay <url>

# With JSON output (preferred for AI agents)
cashr pay <url> --json

# Skip confirmation prompt (only use after user has already approved via cashr check)
cashr pay <url> --confirmed --json

# POST request with body
cashr pay <url> --method POST -d '{"prompt":"hello"}' -H "Content-Type: application/json" --json

# Custom headers
cashr pay <url> -H "Authorization: Bearer token123" --json

# Safety limit on payment amount
cashr pay <url> --max-amount 5000 --json

# Use specific payer address index
cashr pay <url> --payer 0 --json
```

### Check wallet balance

```bash
cashr balance
cashr balance --sats
cashr balance --chipnet
```

## Decision Flow

When preparing to call an unfamiliar API that might require payment:

1. **First check**: `cashr check <url> --json`
   - If payment is not required -> proceed normally
   - If payment is required and BCH is accepted -> inform user of cost and **seek explicit approval** before paying
   - If payment is required but BCH is not accepted -> inform user

2. **When encountering HTTP 402**:
   - Parse the `PAYMENT-REQUIRED` headers
   - **Seek explicit user approval** before spending any BCH
   - Once approved, use `cashr pay <url> --confirmed --json` to handle payment
   - The command handles: parse headers -> select network -> build BCH tx -> broadcast -> retry with payment signature

3. **For known paid APIs**:
   - **Always seek user approval first**, then use `cashr pay <url> --confirmed --json`

## User Approval Required Before Any Payment

**CRITICAL**: The agent MUST NOT execute `cashr pay` without explicit user approval. Since `cashr pay` spends real BCH from the user's wallet, always:

1. Run `cashr check <url> --json` first to determine the cost
2. Inform the user of the cost (e.g., "This API costs ~1000 sats")
3. Wait for explicit user confirmation (e.g., "yes", "go ahead", "pay")
4. Only then execute the payment using `cashr pay <url> --confirmed --json`

The `--confirmed` flag skips the interactive confirmation prompt since the user has already approved the transaction. Do NOT use `--confirmed` without first obtaining explicit user approval.

Do NOT assume the user wants to pay - even if the cost seems small.

## AI Agent Workflow

```
Task: Call nanogpt API
Agent: cashr check https://api.nanogpt.com/v1/complete --json
  -> {"paymentRequired": true, "estimatedCostSats": 100, ...}

Agent: Informs user "This API costs ~100 sats. Approve to proceed?"
User: "yes"

Agent: cashr pay https://api.nanogpt.com/v1/complete --method POST -d '{"prompt":"hello"}' -H "Content-Type: application/json" --confirmed --json
  -> Handles 402 -> pays sats -> returns response with txid
```

## Key Options

| Option | Description |
|--------|-------------|
| `--json` | Machine-readable output (recommended for AI) |
| `--dry-run` | Preview payment without executing |
| `--confirmed` | Skip confirmation prompt (prior approval obtained) |
| `--chipnet` | Use chipnet (testnet) instead of mainnet |
| `--max-amount <sats>` | Safety limit on payment amount |
| `--method <METHOD>` | HTTP method (GET, POST, etc.) |
| `-H <header>` | Add HTTP header (repeatable) |
| `-d <body>` | Request body |
| `--payer <index>` | Address index to use as payer |
| `--change-address <addr>` | Override change address |
| `-n <wallet>` | Target a specific wallet by name |

## Notes

- Payment is per-request (no batching)
- Each request = separate BCH transaction
- Only BCH payments are supported (no stablecoins)
- Uses local wallet stored in ~/.cashr/ (credentials never leave the machine)
- Supports both mainnet and chipnet networks
- Implements x402-bch v2.2 protocol with Ed25519 payment signatures
