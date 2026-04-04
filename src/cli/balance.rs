use anyhow::{Context, Result};
use owo_colors::OwoColorize;

use crate::wallet;

/// Convert BCH to satoshis.
fn bch_to_sats(bch: f64) -> i64 {
    (bch * 1e8).round() as i64
}

/// Format satoshis with thousands separators.
fn format_sats(sats: i64) -> String {
    let s = sats.abs().to_string();
    let mut result = String::new();
    for (i, ch) in s.chars().rev().enumerate() {
        if i > 0 && i % 3 == 0 {
            result.push(',');
        }
        result.push(ch);
    }
    if sats < 0 {
        result.push('-');
    }
    result.chars().rev().collect()
}

/// Display wallet balance (BCH or token).
pub async fn run(
    wallet_name: Option<&str>,
    chipnet: bool,
    token_id: Option<&str>,
    sats_only: bool,
) -> Result<()> {
    let network = if chipnet { "chipnet" } else { "mainnet" };

    let w = wallet::load_wallet(wallet_name).context("failed to load wallet")?;
    let bch = w.for_network(chipnet)?;

    if let Some(tid) = token_id {
        // Validate hex format
        if tid.len() != 64 || !tid.chars().all(|c| c.is_ascii_hexdigit()) {
            anyhow::bail!("token must be a 64-character hex string");
        }

        // Fetch token metadata
        let mut token_name = String::new();
        let mut token_symbol = String::new();
        let mut decimals: u32 = 0;

        if let Ok(Some(info)) = bch.get_token_info(tid).await {
            if info.name != "Unknown Token" {
                token_name = info.name;
            }
            token_symbol = info.symbol;
            decimals = info.decimals;
        }

        let label = if !token_symbol.is_empty() {
            token_symbol.clone()
        } else if !token_name.is_empty() {
            token_name.clone()
        } else {
            "Token".to_string()
        };

        println!(
            "\n   {}\n",
            format!("{} Balance — {} ({})", label, w.name, network).bold()
        );
        println!("   {}", format!("Category: {}", tid).dimmed());
        if !token_name.is_empty() {
            println!("   {}", format!("Name:     {}", token_name).dimmed());
        }

        let result = bch
            .get_token_balance(tid)
            .await
            .context("failed to fetch token balance")?;

        let display_balance = if decimals > 0 {
            result.balance / 10f64.powi(decimals as i32)
        } else {
            result.balance
        };
        let display_spendable = if decimals > 0 {
            result.spendable / 10f64.powi(decimals as i32)
        } else {
            result.spendable
        };
        let unit = if !token_symbol.is_empty() {
            &token_symbol
        } else {
            "tokens"
        };

        println!("   Balance:    {} {}", display_balance, unit);
        if (result.spendable - result.balance).abs() > f64::EPSILON {
            println!(
                "   {}",
                format!("Spendable:  {} {}", display_spendable, unit).dimmed()
            );
        }
    } else {
        // BCH balance
        println!("\n   {}\n", format!("Balance — {} ({})", w.name, network).bold());

        let result = bch
            .get_balance()
            .await
            .context("failed to fetch balance")?;

        let balance_sats = bch_to_sats(result.balance);
        let spendable_sats = bch_to_sats(result.spendable);

        if sats_only {
            println!("   Balance:    {} sats", format_sats(balance_sats));
            if (result.spendable - result.balance).abs() > f64::EPSILON {
                println!(
                    "   {}",
                    format!("Spendable:  {} sats", format_sats(spendable_sats)).dimmed()
                );
            }
        } else {
            println!("   Balance:    {} BCH", result.balance);
            println!(
                "{}",
                format!("               {} sats", format_sats(balance_sats)).dimmed()
            );
            if (result.spendable - result.balance).abs() > f64::EPSILON {
                println!("   Spendable:  {} BCH", result.spendable);
                println!(
                    "{}",
                    format!("               {} sats", format_sats(spendable_sats)).dimmed()
                );
            }
        }
    }

    println!();
    Ok(())
}
