use anyhow::{Context, Result};
use owo_colors::OwoColorize;

use crate::wallet;

/// Derive receiving + change addresses at a specific index.
pub async fn derive(
    wallet_name: Option<&str>,
    index: u32,
    chipnet: bool,
    token: bool,
) -> Result<()> {
    let network = if chipnet { "chipnet" } else { "mainnet" };

    let w = wallet::load_wallet(wallet_name).context("failed to load wallet")?;
    let bch = w.for_network(chipnet).await?;

    let addr_set = if token {
        bch.get_token_address_set_at(index)?
    } else {
        bch.get_address_set_at(index)?
    };

    let label = if token { "Token address" } else { "Address" };
    println!(
        "\n   {}\n",
        format!("{} at index {} — {} ({})", label, index, w.name, network).bold()
    );
    println!("   Receiving:  {}", addr_set.receiving);
    println!("   {}", format!("Change:     {}", addr_set.change).dimmed());
    if token {
        println!("   {}", "Type:       token-aware (z-prefix)".dimmed());
    }
    println!();
    Ok(())
}

/// List multiple derived receiving addresses.
pub async fn list(
    wallet_name: Option<&str>,
    count: u32,
    chipnet: bool,
    token: bool,
) -> Result<()> {
    let network = if chipnet { "chipnet" } else { "mainnet" };

    let w = wallet::load_wallet(wallet_name).context("failed to load wallet")?;
    let bch = w.for_network(chipnet).await?;

    let type_label = if token {
        "Token Addresses"
    } else {
        "Addresses"
    };
    println!(
        "\n   {}\n",
        format!("{} — {} ({})", type_label, w.name, network).bold()
    );
    println!(
        "   {}",
        format!("{:<8}{}", "Index", "Receiving Address").dimmed()
    );
    println!("   {}", "\u{2500}".repeat(70).dimmed());

    for i in 0..count {
        let addr_set = if token {
            bch.get_token_address_set_at(i)?
        } else {
            bch.get_address_set_at(i)?
        };
        println!("   {:<8}{}", i, addr_set.receiving);
    }

    println!();
    Ok(())
}
