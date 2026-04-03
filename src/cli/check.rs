use anyhow::{Context, Result};
use owo_colors::OwoColorize;
use serde_json::json;

use crate::wallet;
use crate::x402::protocol::{parse_payment_required, select_bch_requirements};

/// Parse HTTP headers from "Key: Value" strings.
fn parse_headers(raw: &[String]) -> Result<Vec<(String, String)>> {
    let mut headers = Vec::new();
    for h in raw {
        let idx = h.find(':').ok_or_else(|| {
            anyhow::anyhow!(
                "invalid header format: {}. Expected \"Key: Value\"",
                h
            )
        })?;
        let key = h[..idx].trim().to_string();
        let value = h[idx + 1..].trim().to_string();
        headers.push((key, value));
    }
    Ok(headers)
}

/// Check if a URL requires x402 payment.
pub async fn run(
    wallet_name: Option<&str>,
    url: &str,
    method: &str,
    raw_headers: &[String],
    body: Option<&str>,
    chipnet: bool,
    json_output: bool,
) -> Result<()> {
    let network_name = if chipnet { "chipnet" } else { "mainnet" };
    let method_upper = method.to_uppercase();

    let parsed_headers = parse_headers(raw_headers)?;

    // Ensure wallet exists (needed to determine network compatibility)
    let _w = wallet::load_wallet(wallet_name).context("failed to load wallet")?;

    if !json_output {
        println!("\n   {} {}", "CHECK".bold(), url);
        println!(
            "   {}",
            format!("Network: {}", network_name.cyan()).dimmed()
        );
        println!("   {}", format!("Method: {}", method_upper).dimmed());
        println!();
    }

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
    let is_402 = status.as_u16() == 402;

    let mut accepts_x402 = false;
    let mut accepts_bch = false;
    let mut estimated_cost_sats = String::new();
    let mut cost_in_bch = String::new();
    let mut payment_url = String::new();
    let mut max_timeout = 0u32;
    let mut resource_url = String::new();

    if is_402 {
        let response_body: serde_json::Value = response
            .json()
            .await
            .unwrap_or(json!(null));

        if let Some(pr) = parse_payment_required(&response_body) {
            accepts_x402 = pr.x402_version == 2;
            resource_url = pr.resource.url.clone();

            if let Some(reqs) = select_bch_requirements(&pr, chipnet) {
                accepts_bch = true;
                payment_url = reqs.pay_to.clone();
                estimated_cost_sats = reqs.amount.clone();
                let sats_f: f64 = reqs.amount.parse().unwrap_or(0.0);
                cost_in_bch = format!("{:.8}", sats_f / 1e8);
                max_timeout = reqs.max_timeout_seconds;
            }
        }
    }

    if json_output {
        let mut result = json!({
            "url": url,
            "acceptsX402": accepts_x402,
            "acceptsBch": accepts_bch,
            "paymentRequired": is_402,
        });

        if accepts_bch {
            result["estimatedCostSats"] = json!(estimated_cost_sats);
            result["costInBch"] = json!(cost_in_bch);
            result["paymentUrl"] = json!(payment_url);
            result["maxTimeoutSeconds"] = json!(max_timeout);
            result["resourceUrl"] = json!(resource_url);
        }

        println!(
            "{}",
            serde_json::to_string_pretty(&result)?
        );
    } else if is_402 {
        println!("   {}", "Payment Required".yellow());

        if accepts_x402 {
            println!(
                "   {}",
                "Accepts x402-bch v2.2 protocol".green()
            );

            if accepts_bch {
                println!("   {}", "Accepts BCH payment".green());
                println!(
                    "   {}",
                    format!(
                        "  Amount:      {} sats ({} BCH)",
                        estimated_cost_sats, cost_in_bch
                    )
                    .dimmed()
                );
                println!(
                    "   {}",
                    format!("  Payment URL: {}", payment_url).dimmed()
                );
                println!(
                    "   {}",
                    format!("  Timeout:     {}s", max_timeout).dimmed()
                );
                println!(
                    "   {}",
                    format!("  Resource:    {}", resource_url).dimmed()
                );
            } else {
                println!("   {}", "Does not accept BCH".red());
            }
        } else {
            println!(
                "   {}",
                "Unknown payment protocol (not x402-bch v2.2)".red()
            );
        }
    } else {
        println!("   {}", "No payment required".green());
        println!(
            "   {}",
            format!("  Status: {} is free to access", url).dimmed()
        );
    }

    if !json_output {
        println!();
    }

    Ok(())
}
