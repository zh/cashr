use anyhow::{Context, Result};
use owo_colors::OwoColorize;

use crate::cli::utils::{bch_to_sats, format_sats};
use crate::wallet;

/// Display wallet balance (BCH or token).
pub async fn run(
    wallet_name: Option<&str>,
    chipnet: bool,
    token_id: Option<&str>,
    sats_only: bool,
    verbose: bool,
) -> Result<()> {
    let network = if chipnet { "chipnet" } else { "mainnet" };

    let w = wallet::load_wallet(wallet_name).context("failed to load wallet")?;
    let bch = w.for_network(chipnet)?;
    bch.ensure_synced(5).await?;

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

        if verbose {
            let utxos = bch
                .get_bch_utxos()
                .await
                .context("failed to fetch UTXOs for verbose output")?;
            if !utxos.is_empty() {
                let mut by_path: std::collections::BTreeMap<String, u64> =
                    std::collections::BTreeMap::new();
                for u in &utxos {
                    *by_path.entry(u.address_path.clone()).or_insert(0) += u.value;
                }
                println!("\n   {}\n", "Per-address breakdown:".dimmed());
                for (path, sats) in &by_path {
                    let bch_val = *sats as f64 / 1e8;
                    println!(
                        "   {}  {} BCH  ({} sats)",
                        format!("m/44'/145'/0'/{}", path).dimmed(),
                        bch_val,
                        format_sats(*sats as i64).dimmed()
                    );
                }
            }
        }
    }

    println!();
    Ok(())
}
