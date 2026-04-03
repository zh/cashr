use anyhow::{Context, Result};
use owo_colors::OwoColorize;
use qrcode::QrCode;

use crate::wallet;

/// Print QR code matching JS qrcode-terminal `{ small: true }` style.
/// White background, black modules, 1-module white quiet zone, half-block chars.
fn print_qr(data: &str) -> Result<()> {
    let code = QrCode::new(data.as_bytes()).context("failed to generate QR code")?;
    let width = code.width();
    let modules = code.to_colors();
    let quiet = 1_i32;
    let total = width as i32 + quiet * 2;

    let is_dark = |r: i32, c: i32| -> bool {
        let mr = r - quiet;
        let mc = c - quiet;
        if mr < 0 || mc < 0 || mr >= width as i32 || mc >= width as i32 {
            return false;
        }
        modules[mr as usize * width + mc as usize] == qrcode::Color::Dark
    };

    // Use ▄ (lower half block) with foreground/background colors.
    // Each character = 2 vertical rows. Top row = bg color, bottom row = fg color.
    // White = \x1b[97m (bright white fg) / \x1b[107m (bright white bg)
    // Black = \x1b[30m (black fg) / \x1b[40m (black bg)
    for row in (0..total).step_by(2) {
        print!("   ");
        for col in 0..total {
            let top = is_dark(row, col);
            let bot = is_dark(row + 1, col);
            // ▄ char: foreground = bottom half, background = top half
            match (top, bot) {
                (false, false) => print!("\x1b[107m \x1b[0m"),          // white top, white bottom
                (true, true) => print!("\x1b[40m \x1b[0m"),             // black top, black bottom
                (false, true) => print!("\x1b[107;30m▄\x1b[0m"),        // white top, black bottom
                (true, false) => print!("\x1b[40;97m▄\x1b[0m"),         // black top, white bottom
            };
        }
        println!();
    }
    Ok(())
}

/// Build a BIP21 payment URI for BCH.
fn build_bch_payment_uri(address: &str, amount: Option<f64>) -> String {
    let bare = address
        .strip_prefix("bitcoincash:")
        .or_else(|| address.strip_prefix("bchtest:"))
        .unwrap_or(address);
    let mut uri = format!("bitcoincash:{}", bare);
    if let Some(amt) = amount {
        if amt > 0.0 {
            uri.push_str(&format!("?amount={}", amt));
        }
    }
    uri
}

/// Build a PayPro payment URI for CashToken receiving.
fn build_token_payment_uri(
    address: &str,
    category: &str,
    base_unit_amount: Option<f64>,
) -> String {
    let bare = address
        .strip_prefix("bitcoincash:")
        .or_else(|| address.strip_prefix("bchtest:"))
        .unwrap_or(address);
    let mut uri = format!("bitcoincash:{}?c={}", bare, category);
    if let Some(amt) = base_unit_amount {
        if amt > 0.0 {
            uri.push_str(&format!("&f={}", amt.round() as u64));
        }
    }
    uri
}

/// Display receiving address and QR code.
pub async fn run(
    wallet_name: Option<&str>,
    index: Option<u32>,
    chipnet: bool,
    token: Option<&str>,
    amount: Option<&str>,
    unit: &str,
    no_qr: bool,
) -> Result<()> {
    let network = if chipnet { "chipnet" } else { "mainnet" };
    let idx = index.unwrap_or(0);

    let is_token = token.is_some();
    let category = token.unwrap_or("");
    let has_category = !category.is_empty();

    // Validate category format if provided
    if has_category
        && (category.len() != 64 || !category.chars().all(|c| c.is_ascii_hexdigit()))
    {
        anyhow::bail!("token category must be a 64-character hex string");
    }

    // Parse amount — convert sats to BCH for the URI (BIP21 uses BCH)
    let raw_amount: Option<f64> = match amount {
        Some(s) => {
            let v: f64 = s.parse().context("amount must be a valid number")?;
            if v <= 0.0 {
                anyhow::bail!("amount must be a positive number");
            }
            if !is_token && unit == "sats" {
                Some(v / 1e8)
            } else {
                Some(v)
            }
        }
        None => None,
    };

    let display_amount: Option<String> = match amount {
        Some(s) => Some(s.to_string()),
        None => None,
    };

    if raw_amount.is_some() && is_token && !has_category {
        anyhow::bail!(
            "--amount with --token requires a category ID.\n   Usage: cashr receive --token <category> --amount <amount>"
        );
    }

    let w = wallet::load_wallet(wallet_name).context("failed to load wallet")?;
    let bch = w.for_network(chipnet)?;

    let address = if is_token {
        bch.get_token_address_set_at(idx)?.receiving
    } else {
        bch.get_address_set_at(idx)?.receiving
    };

    // Resolve token metadata if category specified
    let mut token_name = String::new();
    if has_category {
        if let Ok(Some(info)) = bch.get_token_info(category).await {
            if !info.symbol.is_empty() {
                token_name = info.symbol;
            } else if info.name != "Unknown Token" {
                token_name = info.name;
            }
        }
    }

    // Build payment URI
    let payment_uri = if has_category {
        let decimals = if !token_name.is_empty() {
            // Re-fetch to get decimals (or cache from above)
            bch.get_token_info(category)
                .await
                .ok()
                .flatten()
                .map(|t| t.decimals)
                .unwrap_or(0)
        } else {
            0
        };
        let base_unit_amount = raw_amount.map(|a| a * 10f64.powi(decimals as i32));
        Some(build_token_payment_uri(&address, category, base_unit_amount))
    } else if raw_amount.is_some() {
        Some(build_bch_payment_uri(&address, raw_amount))
    } else {
        None
    };

    let qr_content = payment_uri.as_deref().unwrap_or(&address);

    // Output
    let label = if has_category {
        let tn = if token_name.is_empty() {
            "CashTokens"
        } else {
            &token_name
        };
        format!("Receive {}", tn)
    } else if is_token {
        "Receive CashTokens".to_string()
    } else {
        "Receive BCH".to_string()
    };

    println!("\n   {}\n", format!("{} — {} ({})", label, w.name, network).bold());
    println!("   Address:  {}", address);
    println!("   {}", format!("Index:    {}", idx).dimmed());
    if is_token {
        println!("   {}", "Type:     token-aware (z-prefix)".dimmed());
    }
    if has_category {
        println!("   {}", format!("Category: {}", category).dimmed());
    }
    if let Some(ref display_amt) = display_amount {
        let display_unit = if has_category {
            if token_name.is_empty() {
                "tokens"
            } else {
                &token_name
            }
        } else if unit == "sats" {
            "sats"
        } else {
            "BCH"
        };
        println!("   Amount:   {} {}", display_amt, display_unit);
    }
    if let Some(ref uri) = payment_uri {
        println!("   URI:      {}", uri);
    }

    if !no_qr {
        println!();
        print_qr(qr_content)?;
    }

    println!();
    Ok(())
}
