use anyhow::{Context, Result};
use owo_colors::OwoColorize;

use crate::network;
use crate::transaction;
use crate::wallet;

/// Send BCH to a recipient address.
pub async fn run(
    wallet_name: Option<&str>,
    address: &str,
    amount_str: &str,
    unit: &str,
    chipnet: bool,
) -> Result<()> {
    let network_name = if chipnet { "chipnet" } else { "mainnet" };

    // Parse amount
    let mut amount_bch: f64 = amount_str
        .parse()
        .context("amount must be a valid number")?;

    if amount_bch <= 0.0 {
        anyhow::bail!("amount must be a positive number");
    }

    match unit {
        "sats" => {
            amount_bch /= 1e8;
        }
        "bch" => {}
        _ => {
            anyhow::bail!("unit must be \"bch\" or \"sats\"");
        }
    }

    // Validate address format (basic check: must contain a colon or be a legacy format)
    if !address.contains(':') && !address.starts_with('1') && !address.starts_with('3') {
        anyhow::bail!("invalid BCH address");
    }

    let w = wallet::load_wallet(wallet_name).context("failed to load wallet")?;
    let bch = w.for_network(chipnet)?;

    let change_set = bch.get_address_set_at(0)?;
    let change_address = &change_set.change;

    println!(
        "\n   Sending {} from {} on {}",
        format!("{} BCH", amount_bch).bold(),
        w.name.bold(),
        network_name.cyan()
    );
    println!("   {}", format!("To:     {}", address).dimmed());
    println!(
        "   {}",
        format!("Change: {}", change_address).dimmed()
    );
    println!();

    let result = bch
        .send_bch(amount_bch, address, Some(change_address))
        .await
        .context("send BCH request failed")?;

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
                    "Insufficient balance. Short by {} satoshis.",
                    lacking
                )
                .yellow()
            );
        }
    }

    println!();
    Ok(())
}

/// Send all spendable BCH to a recipient (drain wallet).
pub async fn run_send_all(
    wallet_name: Option<&str>,
    address: &str,
    chipnet: bool,
) -> Result<()> {
    let network_name = if chipnet { "chipnet" } else { "mainnet" };

    if !address.contains(':') && !address.starts_with('1') && !address.starts_with('3') {
        anyhow::bail!("invalid BCH address");
    }

    let w = wallet::load_wallet(wallet_name).context("failed to load wallet")?;
    let bch = w.for_network(chipnet)?;
    let hd = w.hd_wallet(chipnet)?;

    // Fetch all UTXOs
    let utxos = bch.get_bch_utxos().await.context("failed to fetch UTXOs")?;

    if utxos.is_empty() {
        anyhow::bail!("no spendable UTXOs found");
    }

    let total: u64 = utxos.iter().map(|u| u.value).sum();

    // Estimate fee: all inputs, 1 output, no change
    let estimated_size = (utxos.len() * 148) + 34 + 10;
    let fee = (estimated_size as f64 * 1.2).ceil() as u64;

    if total <= fee {
        anyhow::bail!(
            "balance too low to cover fee. Have {} sats, need {} sats for fee",
            total,
            fee
        );
    }

    let send_amount = total - fee;

    println!(
        "\n   Sending {} from {} on {} {}",
        format!("ALL ({} sats)", send_amount).bold(),
        w.name.bold(),
        network_name.cyan(),
        "(draining wallet)".dimmed()
    );
    println!("   {}", format!("To:   {}", address).dimmed());
    println!("   {}", format!("Fee:  {} sats", fee).dimmed());
    println!();

    // Build tx with all UTXOs, single output, no change
    let outputs = vec![transaction::TxOutput {
        address: address.to_string(),
        value: send_amount,
    }];

    let built = transaction::build_send_all_transaction(&utxos, &outputs, &hd)
        .context("failed to build transaction")?;

    // Broadcast
    let broadcast_result = bch
        .broadcast(&built.hex)
        .await
        .context("failed to broadcast transaction")?;

    let txid = broadcast_result
        .txid
        .unwrap_or_else(|| built.txid.clone());

    println!("   {}\n", "Transaction sent successfully!".green());
    println!("   txid: {}", txid);
    let explorer = network::explorer_url(chipnet);
    println!("   {}", format!("{}{}", explorer, txid).dimmed());
    println!("   {}", format!("Sent: {} sats, Fee: {} sats", send_amount, built.fee).dimmed());

    println!();
    Ok(())
}
