use anyhow::{Context, Result};
use owo_colors::OwoColorize;

use crate::network;
use crate::wallet;
use crate::wallet::bch::NftSendParams;

/// Truncate a hex string for display.
fn short_hex(hex: &str, len: usize) -> String {
    if hex.len() <= len * 2 + 3 {
        return hex.to_string();
    }
    format!("{}...{}", &hex[..len], &hex[hex.len() - len..])
}

/// Format a token amount with decimal scaling.
fn format_token_amount(raw_amount: f64, decimals: u32) -> String {
    if decimals == 0 {
        return format!("{}", raw_amount as u64);
    }
    let scaled = raw_amount / 10f64.powi(decimals as i32);
    format!("{:.prec$}", scaled, prec = decimals as usize)
}

/// List fungible CashTokens in the wallet.
pub async fn list(wallet_name: Option<&str>, chipnet: bool) -> Result<()> {
    let network_name = if chipnet { "chipnet" } else { "mainnet" };

    let w = wallet::load_wallet(wallet_name).context("failed to load wallet")?;
    let bch = w.for_network(chipnet)?;

    println!(
        "\n   {}\n",
        format!("CashTokens — {} ({})", w.name, network_name).bold()
    );

    let tokens = bch
        .get_fungible_tokens()
        .await
        .context("failed to fetch tokens")?;

    if tokens.is_empty() {
        println!("   {}\n", "No tokens found.".dimmed());
        return Ok(());
    }

    for t in &tokens {
        let amount = format_token_amount(t.balance, t.decimals);
        let symbol = if !t.symbol.is_empty() {
            format!(" {}", t.symbol)
        } else {
            String::new()
        };
        let name = if t.name != "Unknown Token" {
            &t.name
        } else {
            ""
        };

        println!("   {}", format!("{}{}", amount, symbol).bold());
        if !name.is_empty() {
            println!("   {}", name.dimmed());
        }
        println!("   {}", t.category.dimmed());
        println!();
    }

    println!(
        "   {}",
        format!(
            "{} token{} total",
            tokens.len(),
            if tokens.len() != 1 { "s" } else { "" }
        )
        .dimmed()
    );

    println!();
    Ok(())
}

/// Show info for a specific CashToken.
pub async fn info(
    wallet_name: Option<&str>,
    category: &str,
    chipnet: bool,
) -> Result<()> {
    let network_name = if chipnet { "chipnet" } else { "mainnet" };

    if category.len() != 64 || !category.chars().all(|c| c.is_ascii_hexdigit()) {
        anyhow::bail!("category must be a 64-character hex string");
    }

    let w = wallet::load_wallet(wallet_name).context("failed to load wallet")?;
    let bch = w.for_network(chipnet)?;

    println!(
        "\n   {}\n",
        format!("Token Info — {} ({})", w.name, network_name).bold()
    );

    let token_info = bch
        .get_token_info(category)
        .await
        .context("failed to fetch token info")?;

    let token_info = match token_info {
        Some(t) => t,
        None => {
            println!("   {}\n", "Token not found.".yellow());
            return Ok(());
        }
    };

    println!("   Name:      {}", token_info.name);
    if !token_info.symbol.is_empty() {
        println!("   Symbol:    {}", token_info.symbol);
    }
    println!("   Decimals:  {}", token_info.decimals);
    println!("   Category:  {}", token_info.category);

    // Fetch wallet balance for this token
    match bch.get_token_balance(category).await {
        Ok(bal) => {
            let amount = format_token_amount(bal.balance, token_info.decimals);
            let symbol = if !token_info.symbol.is_empty() {
                format!(" {}", token_info.symbol)
            } else {
                String::new()
            };
            println!("   Balance:   {}{}", amount, symbol);
        }
        Err(_) => {
            println!("   {}", "Balance:   0".dimmed());
        }
    }

    // Show NFTs for this category
    if let Ok(nfts) = bch.get_nft_utxos(Some(category)).await {
        if !nfts.is_empty() {
            println!("\n   {}\n", format!("NFTs ({})", nfts.len()).bold());
            for nft in &nfts {
                let cap = if nft.capability == "none" {
                    String::new()
                } else {
                    format!(" [{}]", nft.capability)
                };
                let commitment = if nft.commitment.is_empty() {
                    "(empty)".to_string()
                } else {
                    short_hex(&nft.commitment, 8)
                };
                println!("   {}{}", commitment.cyan(), cap);
                println!(
                    "   {}",
                    format!("{}:{}", nft.txid, nft.vout).dimmed()
                );
                println!();
            }
        }
    }

    println!();
    Ok(())
}

/// Send fungible CashTokens.
pub async fn send(
    wallet_name: Option<&str>,
    address: &str,
    amount_str: &str,
    category: &str,
    chipnet: bool,
) -> Result<()> {
    let network_name = if chipnet { "chipnet" } else { "mainnet" };

    // Validate category
    if category.len() != 64 || !category.chars().all(|c| c.is_ascii_hexdigit()) {
        anyhow::bail!("category must be a 64-character hex string");
    }

    // Parse amount
    let token_amount: u64 = amount_str
        .parse()
        .context("amount must be a valid integer")?;
    if token_amount == 0 {
        anyhow::bail!("amount must be positive");
    }

    let w = wallet::load_wallet(wallet_name).context("failed to load wallet")?;
    let bch = w.for_network(chipnet)?;

    // Token change goes to our token address
    let token_change = bch.get_token_address_set_at(0)?.change;

    // Fetch token info for display
    let mut token_label = short_hex(category, 8);
    if let Ok(Some(info)) = bch.get_token_info(category).await {
        if !info.symbol.is_empty() {
            token_label = info.symbol;
        } else if info.name != "Unknown Token" {
            token_label = info.name;
        }
    }

    println!(
        "\n   Sending {} from {} on {}",
        format!("{} {}", amount_str, token_label).bold(),
        w.name.bold(),
        network_name.cyan()
    );
    println!("   {}", format!("Category: {}", category).dimmed());
    println!("   {}", format!("To:       {}", address).dimmed());
    println!("   {}", format!("Change:   {}", token_change).dimmed());
    println!();

    let result = bch
        .send_token(category, token_amount, address, Some(&token_change))
        .await
        .context("send token request failed")?;

    if result.success {
        println!("   {}\n", "Transaction sent successfully!".green());
        if let Some(ref txid) = result.txid {
            println!("   txid: {}", txid);
            let explorer = network::explorer_url(chipnet);
            println!("   {}", format!("{}{}", explorer, txid).dimmed());
        }
    } else {
        let err_msg = result.error.as_deref().unwrap_or("Unknown error");
        println!(
            "   {}",
            format!("Transaction failed: {}", err_msg).red()
        );
        if let Some(lacking) = result.lacking_sats {
            println!(
                "   {}",
                format!(
                    "Insufficient BCH for transaction fees. Short by {} satoshis.",
                    lacking
                )
                .yellow()
            );
        }
    }

    println!();
    Ok(())
}

/// Send an NFT (non-fungible CashToken).
pub async fn send_nft(
    wallet_name: Option<&str>,
    address: &str,
    category: &str,
    commitment: &str,
    capability: &str,
    txid: Option<&str>,
    vout: Option<u32>,
    chipnet: bool,
) -> Result<()> {
    let network_name = if chipnet { "chipnet" } else { "mainnet" };

    // Validate category
    if category.len() != 64 || !category.chars().all(|c| c.is_ascii_hexdigit()) {
        anyhow::bail!("category must be a 64-character hex string");
    }

    // Validate capability
    if !["none", "minting", "mutable"].contains(&capability) {
        anyhow::bail!("capability must be \"none\", \"minting\", or \"mutable\"");
    }

    let w = wallet::load_wallet(wallet_name).context("failed to load wallet")?;
    let bch = w.for_network(chipnet)?;

    // Resolve UTXO: either from flags or auto-detect
    let (resolved_txid, resolved_vout) = match (txid, vout) {
        (Some(t), Some(v)) => (t.to_string(), v),
        _ => {
            println!("   {}", "\n   Searching for NFT UTXO...".dimmed());
            let nfts = bch
                .get_nft_utxos(Some(category))
                .await
                .context("failed to search for NFTs")?;

            let found = nfts.iter().find(|n| {
                n.commitment == commitment && n.capability == capability
            });

            match found {
                Some(nft) => (nft.txid.clone(), nft.vout),
                None => {
                    anyhow::bail!(
                        "no NFT found matching category {} with commitment \"{}\" and capability \"{}\". Use `cashr token info <category>` to list available NFTs.",
                        short_hex(category, 8),
                        commitment,
                        capability
                    );
                }
            }
        }
    };

    // Change address for leftover BCH
    let change_address = bch.get_token_address_set_at(0)?.change;

    println!("\n   Sending NFT from {} on {}", w.name.bold(), network_name.cyan());
    println!(
        "   {}",
        format!("Category:   {}", category).dimmed()
    );
    println!(
        "   {}",
        format!(
            "Commitment: {}",
            if commitment.is_empty() {
                "(empty)"
            } else {
                commitment
            }
        )
        .dimmed()
    );
    println!(
        "   {}",
        format!("Capability: {}", capability).dimmed()
    );
    println!(
        "   {}",
        format!("UTXO:       {}:{}", short_hex(&resolved_txid, 8), resolved_vout).dimmed()
    );
    println!("   {}", format!("To:         {}", address).dimmed());
    println!();

    let result = bch
        .send_nft(NftSendParams {
            category: category.to_string(),
            commitment: commitment.to_string(),
            capability: capability.to_string(),
            txid: resolved_txid,
            vout: resolved_vout,
            address: address.to_string(),
            change_address: Some(change_address),
        })
        .await
        .context("send NFT request failed")?;

    if result.success {
        println!("   {}\n", "Transaction sent successfully!".green());
        if let Some(ref txid) = result.txid {
            println!("   txid: {}", txid);
            let explorer = network::explorer_url(chipnet);
            println!("   {}", format!("{}{}", explorer, txid).dimmed());
        }
    } else {
        let err_msg = result.error.as_deref().unwrap_or("Unknown error");
        println!(
            "   {}",
            format!("Transaction failed: {}", err_msg).red()
        );
        if let Some(lacking) = result.lacking_sats {
            println!(
                "   {}",
                format!(
                    "Insufficient BCH for transaction fees. Short by {} satoshis.",
                    lacking
                )
                .yellow()
            );
        }
    }

    println!();
    Ok(())
}
