use anyhow::{Context, Result};
use owo_colors::OwoColorize;

use crate::cli::utils::{bch_to_sats, format_sats, short_txid};
use crate::network;
use crate::wallet;
use crate::wallet::bch::HistoryOptions;

/// Display transaction history.
pub async fn run(
    wallet_name: Option<&str>,
    chipnet: bool,
    page: u32,
    record_type: &str,
    token_id: Option<&str>,
    sats: bool,
) -> Result<()> {
    let network_name = if chipnet { "chipnet" } else { "mainnet" };

    if page < 1 {
        anyhow::bail!("page must be a positive integer");
    }

    if !["all", "incoming", "outgoing"].contains(&record_type) {
        anyhow::bail!("type must be \"all\", \"incoming\", or \"outgoing\"");
    }

    let tid = token_id.unwrap_or("");
    if !tid.is_empty()
        && (tid.len() != 64 || !tid.chars().all(|c| c.is_ascii_hexdigit()))
    {
        anyhow::bail!("token must be a 64-character hex string");
    }

    let w = wallet::load_wallet(wallet_name).context("failed to load wallet")?;
    let bch = w.for_network(chipnet)?;
    bch.ensure_synced(5).await?;

    // Resolve token label for header
    let mut header_label = "Transaction History".to_string();
    if !tid.is_empty() {
        if let Ok(Some(info)) = bch.get_token_info(tid).await {
            if !info.symbol.is_empty() {
                header_label = format!("{} Transaction History", info.symbol);
            } else if info.name != "Unknown Token" {
                header_label = format!("{} Transaction History", info.name);
            } else {
                header_label = "Token Transaction History".to_string();
            }
        } else {
            header_label = "Token Transaction History".to_string();
        }
    }

    println!(
        "\n   {}\n",
        format!("{} — {} ({})", header_label, w.name, network_name).bold()
    );

    if !tid.is_empty() {
        println!("   {}\n", format!("Category: {}", tid).dimmed());
    }

    let result = bch
        .get_history(HistoryOptions {
            page,
            record_type: record_type.to_string(),
            token_id: tid.to_string(),
        })
        .await
        .context("failed to fetch history")?;

    if result.history.is_empty() {
        println!("   {}\n", "No transactions found.".dimmed());
        return Ok(());
    }

    let explorer = network::explorer_url(chipnet);

    for tx in &result.history {
        let is_incoming = tx.record_type == "incoming";
        let arrow = if is_incoming {
            "  IN".green().to_string()
        } else {
            " OUT".red().to_string()
        };

        let amount_str = if !tid.is_empty() {
            format!("{}", tx.amount)
        } else if sats {
            format!("{} sats", format_sats(bch_to_sats(tx.amount)))
        } else {
            format!("{} BCH", tx.amount)
        };

        let amount_colored = if is_incoming {
            format!("+{}", amount_str).green().to_string()
        } else {
            format!("-{}", amount_str).red().to_string()
        };

        let date = if !tx.tx_timestamp.is_empty() {
            &tx.tx_timestamp
        } else {
            &tx.date_created
        };

        println!("   {}  {}", arrow, amount_colored);

        // Show token changes if any (indented under the BCH line)
        for tc in &tx.token_changes {
            let short_cat = if tc.category.len() > 16 {
                format!("{}...{}", &tc.category[..8], &tc.category[tc.category.len()-6..])
            } else {
                tc.category.clone()
            };

            if tc.amount.abs() > 0.0 {
                let ft_str = if tc.amount > 0.0 {
                    format!("+{} tokens [{}]", tc.amount as i64, short_cat).green().to_string()
                } else {
                    format!("{} tokens [{}]", tc.amount as i64, short_cat).red().to_string()
                };
                println!("         {}", ft_str);
            }
            if tc.nft_amount.abs() > 0.0 {
                let nft_str = if tc.nft_amount > 0.0 {
                    format!("+{} NFT [{}]", tc.nft_amount as i64, short_cat).green().to_string()
                } else {
                    format!("{} NFT [{}]", tc.nft_amount as i64, short_cat).red().to_string()
                };
                println!("         {}", nft_str);
            }
        }

        println!("{}", format!("         {}", date).dimmed());
        println!(
            "{}",
            format!("         {}", short_txid(&tx.txid)).dimmed()
        );
        println!(
            "{}",
            format!("         {}{}", explorer, tx.txid).dimmed()
        );
        println!();
    }

    // Pagination info
    let page_num: u32 = result.page.parse().unwrap_or(page);
    let token_flag = if !tid.is_empty() {
        format!(" --token {}", tid)
    } else {
        String::new()
    };
    let chipnet_flag = if chipnet { " --chipnet" } else { "" };
    let next_hint = if result.has_next {
        format!(
            "  --  next: cashr history --page {}{}{}",
            page_num + 1,
            token_flag,
            chipnet_flag
        )
    } else {
        String::new()
    };
    println!(
        "   {}",
        format!(
            "Page {} of {}{}",
            page_num, result.num_pages, next_hint
        )
        .dimmed()
    );

    println!();
    Ok(())
}
