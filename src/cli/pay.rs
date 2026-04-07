use anyhow::{Context, Result};
use owo_colors::OwoColorize;
use serde_json::json;

use crate::network;
use crate::wallet;
use crate::x402::payer::X402Payer;
use crate::x402::protocol::{parse_payment_required, select_bch_requirements};

/// Parse HTTP headers from "Key: Value" strings.
fn parse_headers(raw: &[String]) -> Result<Vec<(String, String)>> {
    let mut headers = Vec::new();
    for h in raw {
        let idx = h
            .find(':')
            .ok_or_else(|| anyhow::anyhow!("invalid header format: {}. Expected \"Key: Value\"", h))?;
        let key = h[..idx].trim().to_string();
        let value = h[idx + 1..].trim().to_string();
        headers.push((key, value));
    }
    Ok(headers)
}

/// Make a paid HTTP request via x402 protocol.
#[allow(clippy::too_many_arguments)]
pub async fn run(
    wallet_name: Option<&str>,
    url: &str,
    method: &str,
    raw_headers: &[String],
    body: Option<&str>,
    chipnet: bool,
    max_amount: Option<u64>,
    change_address: Option<&str>,
    payer_index: Option<u32>,
    dry_run: bool,
    json_output: bool,
    confirmed: bool,
) -> Result<()> {
    let network_name = if chipnet { "chipnet" } else { "mainnet" };
    let method_upper = method.to_uppercase();

    let parsed_headers = parse_headers(raw_headers)?;

    let w = wallet::load_wallet(wallet_name).context("failed to load wallet")?;
    let bch = w.for_network(chipnet)?;
    let hd = w.hd_wallet(chipnet)?;

    let addr_index = payer_index.unwrap_or(0);
    let x402_payer = X402Payer::new(&hd, addr_index)?;
    let payer_addr = x402_payer.payer_address().to_string();

    if json_output {
        return run_json(
            url,
            &method_upper,
            &parsed_headers,
            body,
            chipnet,
            &bch,
            &x402_payer,
            &payer_addr,
            change_address,
            confirmed,
            max_amount,
        )
        .await;
    }

    if dry_run {
        return run_dry_run(
            url,
            &method_upper,
            &parsed_headers,
            body,
            chipnet,
            &bch,
            &payer_addr,
            change_address,
        )
        .await;
    }

    // Human-readable mode
    println!("\n   {} {}", method_upper.bold(), url);
    println!(
        "   {}",
        format!("Network: {}", network_name.cyan()).dimmed()
    );
    println!("   {}", format!("Payer: {}", payer_addr).dimmed());
    if !parsed_headers.is_empty() {
        let hdrs: Vec<String> = parsed_headers
            .iter()
            .map(|(k, v)| format!("{}: {}", k, v))
            .collect();
        println!("   {}", format!("Headers: {}", hdrs.join(", ")).dimmed());
    }
    println!();

    let client = reqwest::Client::new();
    let mut req_builder = client.request(
        method_upper.parse().unwrap_or(reqwest::Method::GET),
        url,
    );
    for (k, v) in &parsed_headers {
        req_builder = req_builder.header(k, v);
    }
    if ["POST", "PUT", "PATCH"].contains(&method_upper.as_str()) {
        if let Some(b) = body {
            req_builder = req_builder.body(b.to_string());
        }
    }

    let response = req_builder.send().await.context("HTTP request failed")?;
    let status = response.status();

    if status.as_u16() == 402 {
        let response_body: serde_json::Value = response
            .json()
            .await
            .context("failed to parse 402 response body")?;

        let payment_required = parse_payment_required(&response_body)
            .ok_or_else(|| anyhow::anyhow!("could not parse PaymentRequired from 402 response body"))?;

        let requirements = select_bch_requirements(&payment_required, chipnet)
            .ok_or_else(|| anyhow::anyhow!("server does not accept BCH payment"))?;

        let amount_sats = &requirements.amount;
        let amount_bch = amount_sats.parse::<f64>().unwrap_or(0.0) / 1e8;
        let pay_to = &requirements.pay_to;
        let change = change_address
            .map(|s| s.to_string())
            .or_else(|| bch.get_address_set_at(0).ok().map(|a| a.change))
            .unwrap_or_default();

        // Enforce max_amount safety limit
        if let Some(max) = max_amount {
            let requested: u64 = amount_sats.parse().unwrap_or(0);
            if requested > max {
                anyhow::bail!(
                    "server requested {} sats but --max-amount is {} sats",
                    requested,
                    max
                );
            }
        }

        if !confirmed {
            println!("   {}", "Payment Required".yellow());
            println!(
                "   {}",
                format!(
                    "Amount:     {} BCH ({} sats)",
                    amount_bch, amount_sats
                )
                .dimmed()
            );
            println!("   {}", format!("To:         {}", pay_to).dimmed());
            println!("   {}", format!("Change:     {}", change).dimmed());
            println!("   {}", format!("Payer:      {}", payer_addr).dimmed());

            let answer = inquire::Confirm::new("Confirm payment?")
                .with_default(false)
                .prompt()
                .context("failed to read confirmation")?;
            if !answer {
                println!("\n   {}\n", "Payment cancelled.".red());
                return Ok(());
            }
        }

        // Send BCH payment
        let send_result = bch
            .send_bch(amount_bch, pay_to, Some(&change))
            .await
            .context("BCH payment failed")?;

        if !send_result.success {
            let err = send_result.error.unwrap_or_else(|| "Unknown error".to_string());
            anyhow::bail!("payment transaction failed: {}", err);
        }

        let txid = send_result
            .txid
            .ok_or_else(|| anyhow::anyhow!("transaction succeeded but no txid returned"))?;

        // Build signed payload
        let payment_payload = x402_payer.create_payment_payload(
            requirements,
            &payment_required.resource.url,
            &txid,
            Some(0),
            Some(amount_sats),
        )?;

        let payload_json =
            serde_json::to_string(&payment_payload).context("failed to serialize payment payload")?;

        // Retry with PAYMENT-SIGNATURE header
        let mut retry_builder = client.request(
            method_upper.parse().unwrap_or(reqwest::Method::GET),
            url,
        );
        for (k, v) in &parsed_headers {
            retry_builder = retry_builder.header(k, v);
        }
        retry_builder = retry_builder.header("PAYMENT-SIGNATURE", &payload_json);
        if ["POST", "PUT", "PATCH"].contains(&method_upper.as_str()) {
            if let Some(b) = body {
                retry_builder = retry_builder.body(b.to_string());
            }
        }

        let explorer = network::explorer_url(chipnet);
        println!(
            "\n   {}",
            format!("Payment txid: {}{}", explorer, txid).dimmed()
        );
        println!(
            "   {}",
            format!("Recipient:   {}", pay_to).dimmed()
        );

        let retry_response = retry_builder
            .send()
            .await
            .context("retry request failed")?;
        let retry_status = retry_response.status();
        let retry_text = retry_response.text().await.unwrap_or_default();

        println!(
            "\n   {}",
            format!("Response: {} {}", retry_status.as_u16(), retry_status.canonical_reason().unwrap_or("")).green()
        );
        println!();
        println!("{}", format_response_text(&retry_text));
    } else {
        let response_text = response.text().await.unwrap_or_default();
        println!(
            "   {}",
            format!(
                "Response: {} {}",
                status.as_u16(),
                status.canonical_reason().unwrap_or("")
            )
            .green()
        );
        println!();
        println!("{}", format_response_text(&response_text));
    }

    println!();
    Ok(())
}

/// Dry-run mode: check what would happen without paying.
#[allow(clippy::too_many_arguments)]
async fn run_dry_run(
    url: &str,
    method: &str,
    headers: &[(String, String)],
    body: Option<&str>,
    chipnet: bool,
    bch: &wallet::bch::BchWallet,
    payer_addr: &str,
    change_address: Option<&str>,
) -> Result<()> {
    let network_name = if chipnet { "chipnet" } else { "mainnet" };

    println!(
        "\n   {} {} {}",
        method.bold(),
        url,
        "[DRY RUN]".dimmed()
    );
    println!(
        "   {}",
        format!("Network: {}", network_name.cyan()).dimmed()
    );
    println!("   {}", format!("Payer: {}", payer_addr).dimmed());
    println!();

    let client = reqwest::Client::new();
    let mut req_builder = client.request(
        method.parse().unwrap_or(reqwest::Method::GET),
        url,
    );
    for (k, v) in headers {
        req_builder = req_builder.header(k, v);
    }
    if ["POST", "PUT", "PATCH"].contains(&method) {
        if let Some(b) = body {
            req_builder = req_builder.body(b.to_string());
        }
    }

    let response = req_builder.send().await.context("HTTP request failed")?;
    let status = response.status();

    if status.as_u16() == 402 {
        let response_body: serde_json::Value = response
            .json()
            .await
            .context("failed to parse 402 response body")?;

        let payment_required = match parse_payment_required(&response_body) {
            Some(pr) => pr,
            None => {
                println!(
                    "   {}",
                    "Error: Could not parse PaymentRequired from 402 response body".red()
                );
                return Ok(());
            }
        };

        let requirements = match select_bch_requirements(&payment_required, chipnet) {
            Some(r) => r,
            None => {
                println!(
                    "   {}",
                    "Error: Server does not accept BCH payment".red()
                );
                return Ok(());
            }
        };

        let change = change_address
            .map(|s| s.to_string())
            .or_else(|| bch.get_address_set_at(0).ok().map(|a| a.change))
            .unwrap_or_default();

        let amount_bch = requirements
            .amount
            .parse::<f64>()
            .unwrap_or(0.0)
            / 1e8;

        println!("   {}", "402 PAYMENT REQUIRED".yellow());
        println!("   {}", "Payment details:".dimmed());
        println!(
            "   {}",
            format!("  PayTo:      {}", requirements.pay_to).dimmed()
        );
        println!(
            "   {}",
            format!(
                "  Amount:     {} sats ({:.8} BCH)",
                requirements.amount, amount_bch
            )
            .dimmed()
        );
        println!(
            "   {}",
            format!("  Timeout:    {}s", requirements.max_timeout_seconds).dimmed()
        );
        println!(
            "   {}",
            format!(
                "  Resource:   {}",
                payment_required.resource.url
            )
            .dimmed()
        );
        println!();
        println!("   {}", "Wallet:".dimmed());
        println!(
            "   {}",
            format!("  Payer:      {}", payer_addr).dimmed()
        );
        println!(
            "   {}",
            format!("  Change:     {}", change).dimmed()
        );
        println!();

        // Check balance
        match bch.get_balance().await {
            Ok(bal) => {
                let available = (bal.spendable * 1e8) as u64;
                let required: u64 = requirements.amount.parse().unwrap_or(0);
                if available >= required {
                    println!(
                        "   {}",
                        format!(
                            "Balance OK: {} sats available, {} sats required",
                            available, required
                        )
                        .green()
                    );
                } else {
                    println!(
                        "   {}",
                        format!(
                            "Insufficient: {} sats available, {} sats required",
                            available, required
                        )
                        .red()
                    );
                }
            }
            Err(e) => {
                println!(
                    "   {}",
                    format!("(Could not check balance: {})", e).dimmed()
                );
            }
        }
    } else {
        println!(
            "   {}",
            format!(
                "Response: {} {} (no payment required)",
                status.as_u16(),
                status.canonical_reason().unwrap_or("")
            )
            .green()
        );
    }

    println!();
    println!("   {}", format!("To execute: cashr pay {}", url).dimmed());
    println!();
    Ok(())
}

/// JSON output mode.
#[allow(clippy::too_many_arguments)]
async fn run_json(
    url: &str,
    method: &str,
    headers: &[(String, String)],
    body: Option<&str>,
    chipnet: bool,
    bch: &wallet::bch::BchWallet,
    x402_payer: &X402Payer,
    payer_addr: &str,
    change_address: Option<&str>,
    confirmed: bool,
    max_amount: Option<u64>,
) -> Result<()> {
    let client = reqwest::Client::new();
    let mut req_builder = client.request(
        method.parse().unwrap_or(reqwest::Method::GET),
        url,
    );
    for (k, v) in headers {
        req_builder = req_builder.header(k, v);
    }
    if ["POST", "PUT", "PATCH"].contains(&method) {
        if let Some(b) = body {
            req_builder = req_builder.body(b.to_string());
        }
    }

    let response = req_builder.send().await.context("HTTP request failed")?;
    let status = response.status();
    let response_text = response.text().await.unwrap_or_default();
    let response_data: serde_json::Value =
        serde_json::from_str(&response_text).unwrap_or(json!(response_text));

    if status.as_u16() == 402 {
        let payment_required = parse_payment_required(&response_data);
        let payment_required = match payment_required {
            Some(pr) => pr,
            None => {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&json!({
                        "success": false,
                        "status": 402,
                        "error": "Could not parse PaymentRequired from 402 response body"
                    }))?
                );
                return Ok(());
            }
        };

        let requirements = match select_bch_requirements(&payment_required, chipnet) {
            Some(r) => r,
            None => {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&json!({
                        "success": false,
                        "status": 402,
                        "error": "Server does not accept BCH payment"
                    }))?
                );
                return Ok(());
            }
        };

        let amount_sats = &requirements.amount;
        let amount_bch = amount_sats.parse::<f64>().unwrap_or(0.0) / 1e8;
        let pay_to = &requirements.pay_to;
        let change = change_address
            .map(|s| s.to_string())
            .or_else(|| bch.get_address_set_at(0).ok().map(|a| a.change))
            .unwrap_or_default();

        // Enforce max_amount safety limit
        if let Some(max) = max_amount {
            let requested: u64 = amount_sats.parse().unwrap_or(0);
            if requested > max {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&json!({
                        "success": false,
                        "status": 402,
                        "error": format!("server requested {} sats but --max-amount is {} sats", requested, max)
                    }))?
                );
                return Ok(());
            }
        }

        if !confirmed {
            // In JSON mode without --confirmed, still prompt
            let answer = inquire::Confirm::new("Confirm payment?")
                .with_default(false)
                .prompt()
                .unwrap_or(false);
            if !answer {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&json!({
                        "success": false,
                        "status": 402,
                        "payment": { "required": true, "error": "Payment rejected by user" },
                        "error": "Payment rejected by user"
                    }))?
                );
                return Ok(());
            }
        }

        // Send BCH
        let send_result = bch
            .send_bch(amount_bch, pay_to, Some(&change))
            .await
            .context("BCH payment failed")?;

        if !send_result.success {
            let err = send_result.error.unwrap_or_else(|| "Unknown error".to_string());
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({
                    "success": false,
                    "status": 402,
                    "payment": { "required": true, "error": err },
                    "error": err
                }))?
            );
            return Ok(());
        }

        let txid = send_result.txid.unwrap_or_default();

        // Build signed payload
        let payment_payload = x402_payer.create_payment_payload(
            requirements,
            &payment_required.resource.url,
            &txid,
            Some(0),
            Some(amount_sats),
        )?;
        let payload_json = serde_json::to_string(&payment_payload)?;

        // Retry with PAYMENT-SIGNATURE
        let mut retry_builder = client.request(
            method.parse().unwrap_or(reqwest::Method::GET),
            url,
        );
        for (k, v) in headers {
            retry_builder = retry_builder.header(k, v);
        }
        retry_builder = retry_builder.header("PAYMENT-SIGNATURE", &payload_json);
        if ["POST", "PUT", "PATCH"].contains(&method) {
            if let Some(b) = body {
                retry_builder = retry_builder.body(b.to_string());
            }
        }

        let retry_response = retry_builder.send().await.context("retry request failed")?;
        let retry_status = retry_response.status();
        let retry_text = retry_response.text().await.unwrap_or_default();
        let retry_data: serde_json::Value =
            serde_json::from_str(&retry_text).unwrap_or(json!(retry_text));

        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "success": retry_status.is_success(),
                "status": retry_status.as_u16(),
                "statusText": retry_status.canonical_reason().unwrap_or(""),
                "data": retry_data,
                "payment": {
                    "required": true,
                    "txid": txid,
                    "recipientAddress": pay_to,
                    "payer": payer_addr
                }
            }))?
        );
    } else {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "success": status.is_success(),
                "status": status.as_u16(),
                "statusText": status.canonical_reason().unwrap_or(""),
                "data": response_data,
                "payment": { "required": false }
            }))?
        );
    }

    Ok(())
}

fn format_response_text(text: &str) -> String {
    // Try to pretty-print JSON, otherwise return as-is
    match serde_json::from_str::<serde_json::Value>(text) {
        Ok(v) => serde_json::to_string_pretty(&v).unwrap_or_else(|_| text.to_string()),
        Err(_) => text.to_string(),
    }
}
