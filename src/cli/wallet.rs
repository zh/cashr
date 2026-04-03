use anyhow::{Context, Result};
use owo_colors::OwoColorize;

use crate::storage;
use crate::wallet;

/// Create a new wallet with a 12-word seed phrase.
pub async fn create(name: &str, chipnet: bool) -> Result<()> {
    let network = if chipnet { "chipnet" } else { "mainnet" };
    println!("\n   Creating new wallet on {}...\n", network.cyan());

    let info = wallet::generate_mnemonic(name)
        .context("failed to create wallet")?;

    // Display seed phrase with warning
    println!(
        "{}",
        "   IMPORTANT: Write down your seed phrase and store it safely."
            .yellow()
            .bold()
    );
    println!(
        "{}",
        "   Anyone with this phrase can access your funds.".yellow()
    );
    println!(
        "{}",
        "   This is the only time it will be displayed.\n".yellow()
    );
    println!("{}", "   Seed phrase:\n".bold());

    let words: Vec<&str> = info.mnemonic.split_whitespace().collect();
    for (i, word) in words.iter().enumerate() {
        println!("   {:>2}. {}", i + 1, word);
    }

    println!();
    println!("   {}", format!("Wallet hash: {}", info.wallet_hash).dimmed());

    // Derive and show address, subscribe with Watchtower
    if let Ok(w) = wallet::load_wallet(Some(name)) {
        if let Ok(bch) = w.for_network(chipnet) {
            if let Ok(addr_set) = bch.get_address_set_at(0) {
                println!("   {}", format!("Network:     {}", network).dimmed());
                println!(
                    "   {}",
                    format!("Address:     {}", addr_set.receiving).dimmed()
                );
            }
            // Subscribe to Watchtower (non-critical)
            let _ = bch.get_new_address_set(0).await;
        }
    }

    println!("\n   {}\n", "Wallet created and stored.".green());
    Ok(())
}

/// Import an existing wallet from a seed phrase.
pub async fn import(name: &str, chipnet: bool) -> Result<()> {
    let network = if chipnet { "chipnet" } else { "mainnet" };

    // Prompt for mnemonic
    let input = inquire::Text::new("Enter your 12-word seed phrase:")
        .prompt()
        .context("failed to read seed phrase")?;

    if input.trim().is_empty() {
        anyhow::bail!("no seed phrase provided");
    }

    let info =
        wallet::import_mnemonic(name, &input).context("failed to import wallet")?;

    println!(
        "\n   {}",
        format!("Wallet imported successfully on {}.", network).green()
    );
    println!();
    println!(
        "   {}",
        format!("Wallet hash: {}", info.wallet_hash).dimmed()
    );

    // Derive and show address
    if let Ok(w) = wallet::load_wallet(Some(name)) {
        if let Ok(bch) = w.for_network(chipnet) {
            if let Ok(addr_set) = bch.get_address_set_at(0) {
                println!(
                    "   {}",
                    format!("Address:     {}", addr_set.receiving).dimmed()
                );
            }
            // Register addresses 0-9 with Watchtower and trigger UTXO scan
            println!("   {}", "Registering addresses with Watchtower...".dimmed());
            if let Err(e) = bch.scan_addresses(0, 10).await {
                println!("   {}", format!("Warning: address scan failed: {}", e).yellow());
            }
            if let Err(e) = bch.scan_utxos(false).await {
                println!("   {}", format!("Warning: UTXO scan failed: {}", e).yellow());
            }
        }
    }

    println!("\n   {}\n", "Stored in filesystem.".dimmed());
    Ok(())
}

/// Display wallet info: name, hash, address, balance.
pub async fn info(wallet_name: Option<&str>, chipnet: bool) -> Result<()> {
    let network = if chipnet { "chipnet" } else { "mainnet" };

    let w = wallet::load_wallet(wallet_name).context("failed to load wallet")?;
    let bch = w.for_network(chipnet)?;

    println!("\n   {}\n", format!("Wallet Info ({})", network).bold());
    println!(
        "   {}",
        format!("Name:        {}", w.name).dimmed()
    );
    println!(
        "   {}",
        format!("Wallet hash: {}", w.wallet_hash()).dimmed()
    );

    let addr_set = bch.get_address_set_at(0)?;
    println!("   Address:      {}", addr_set.receiving);

    // Fetch balance
    match bch.get_balance().await {
        Ok(balance) => {
            println!("   Balance:      {} BCH", balance.balance);
            if (balance.spendable - balance.balance).abs() > f64::EPSILON {
                println!(
                    "   {}",
                    format!("Spendable:    {} BCH", balance.spendable).dimmed()
                );
            }
        }
        Err(_) => {
            println!("   {}", "Balance:      (unable to fetch)".yellow());
        }
    }

    println!();
    Ok(())
}

/// Export (display) the wallet seed phrase.
pub fn export(wallet_name: Option<&str>) -> Result<()> {
    let info = wallet::load_mnemonic(wallet_name).context("failed to load wallet")?;

    println!(
        "\n   {}",
        "WARNING: Do not share your seed phrase with anyone."
            .yellow()
            .bold()
    );
    println!(
        "   {}\n",
        "Anyone with this phrase can access your funds.".yellow()
    );

    let words: Vec<&str> = info.mnemonic.split_whitespace().collect();
    for (i, word) in words.iter().enumerate() {
        println!("   {:>2}. {}", i + 1, word);
    }

    println!();
    Ok(())
}

/// Delete a wallet after confirmation.
pub fn delete(name: &str) -> Result<()> {
    if !storage::wallet_exists(name)? {
        anyhow::bail!("wallet '{}' not found", name);
    }

    // Show what will be deleted
    println!("\n   {}", format!("Wallet: {}", name).bold());
    if let Ok(info) = wallet::load_mnemonic(Some(name)) {
        println!("   {}", format!("Hash:   {}", info.wallet_hash).dimmed());
    }

    let confirmed = inquire::Confirm::new(&format!("Delete wallet '{name}'? This cannot be undone."))
        .with_default(false)
        .prompt()
        .context("failed to read confirmation")?;

    if !confirmed {
        println!("\n   {}\n", "Cancelled.".dimmed());
        return Ok(());
    }

    storage::delete_wallet(name).context("failed to delete wallet")?;
    println!("\n   {}\n", format!("Wallet '{}' deleted.", name).green());
    Ok(())
}

/// Set a wallet as the default.
pub fn set_default(name: &str) -> Result<()> {
    if !storage::wallet_exists(name)? {
        anyhow::bail!("wallet '{}' not found", name);
    }
    storage::set_default_wallet(name)?;
    println!("\n   {}\n", format!("'{}' is now the default wallet.", name).green());
    Ok(())
}

/// Re-scan addresses and UTXOs with Watchtower.
pub async fn scan(wallet_name: Option<&str>, count: u32, chipnet: bool) -> Result<()> {
    let network = if chipnet { "chipnet" } else { "mainnet" };

    let w = wallet::load_wallet(wallet_name).context("failed to load wallet")?;
    let bch = w.for_network(chipnet)?;

    println!(
        "\n   {}",
        format!("Scanning {} addresses on {}...", count, network).dimmed()
    );

    bch.scan_addresses(0, count)
        .await
        .context("failed to register addresses with Watchtower")?;

    println!("   {}", "Addresses registered. Triggering UTXO scan...".dimmed());

    bch.scan_utxos(false)
        .await
        .context("failed to trigger UTXO scan")?;

    println!("\n   {}\n", "Scan complete.".green());
    Ok(())
}

/// List all stored wallets.
pub fn list(wallet_name: Option<&str>) -> Result<()> {
    let wallets = storage::list_wallets().context("failed to list wallets")?;

    if wallets.is_empty() {
        println!("\n   {}\n", "No wallets found.".dimmed());
        return Ok(());
    }

    let default_name = storage::get_default_wallet().unwrap_or(None);

    // The "active" wallet is whichever --name resolves to, or the default
    let active = wallet_name
        .map(|n| n.to_string())
        .or(default_name);

    println!("\n   {}\n", "Wallets".bold());

    for name in &wallets {
        let is_active = active.as_deref() == Some(name.as_str());
        let marker = if is_active { " *" } else { "  " };

        // Derive address at 0 for display
        let addr = wallet::load_wallet(Some(name))
            .ok()
            .and_then(|w| w.hd_wallet(false).ok())
            .and_then(|hd| hd.get_address_at("0/0", false).ok())
            .unwrap_or_default();

        if is_active {
            println!("   {}{}", marker, name.bold());
        } else {
            println!("   {}{}", marker, name);
        }
        if !addr.is_empty() {
            println!("      {}", addr.dimmed());
        }
    }

    println!();
    Ok(())
}
