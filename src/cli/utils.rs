/// Shared CLI utility functions.
/// Convert BCH to satoshis.
pub fn bch_to_sats(bch: f64) -> i64 {
    (bch * 1e8).round() as i64
}

/// Format satoshis with thousands separators.
pub fn format_sats(sats: i64) -> String {
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

/// Truncate a hex string for display (e.g. "abcdef...123456").
pub fn short_hex(hex: &str, len: usize) -> String {
    if hex.len() <= len * 2 + 3 {
        return hex.to_string();
    }
    format!("{}...{}", &hex[..len], &hex[hex.len() - len..])
}

/// Truncate a txid for display.
pub fn short_txid(txid: &str) -> String {
    if txid.len() <= 20 {
        return txid.to_string();
    }
    format!("{}...{}", &txid[..10], &txid[txid.len() - 10..])
}

/// Format a token amount with decimal scaling.
pub fn format_token_amount(raw_amount: f64, decimals: u32) -> String {
    if decimals == 0 {
        return format!("{}", raw_amount as u64);
    }
    let scaled = raw_amount / 10f64.powi(decimals as i32);
    format!("{:.prec$}", scaled, prec = decimals as usize)
}
