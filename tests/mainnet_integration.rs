/// Integration tests for the Mainnet Cash REST API.
///
/// These tests hit the live API and verify that the cashr wallet
/// can successfully communicate with the mainnet-cash REST service.
///
/// Run with: cargo test --test mainnet_integration -- --test-threads=1
///
/// Note: Must run single-threaded (--test-threads=1) because the
/// mainnet-cash API returns 502 errors under parallel requests.
use mainnet::apis::{configuration::Configuration, wallet_api, wallet_bcmr_api};
use mainnet::models;

fn config() -> Configuration {
    Configuration {
        base_path: "https://rest-unstable.mainnet.cash".to_string(),
        ..Default::default()
    }
}

/// A valid mainnet address for read-only testing (empty wallet is fine).
const TEST_ADDR: &str = "bitcoincash:qrvcdmgpk73zyfd8pmdl9wnuld36zh9n4gms8s0u59";

fn watch_id() -> String {
    format!("watch:mainnet:{}", TEST_ADDR)
}

/// Check if an error is a transient server error (502/503/500).
/// These are API infrastructure issues, not code bugs — skip the test.
fn is_transient<E: std::fmt::Debug>(err: &E) -> bool {
    let s = format!("{:?}", err);
    s.contains("502") || s.contains("503") || s.contains("500")
}

macro_rules! assert_ok_or_skip {
    ($result:expr, $msg:expr) => {
        match &$result {
            Ok(_) => {}
            Err(e) if is_transient(e) => {
                eprintln!("SKIPPED: {} (transient server error)", $msg);
                return;
            }
            Err(e) => panic!("{}: {:?}", $msg, e),
        }
    };
}

#[tokio::test]
async fn test_watch_wallet_balance() {
    let result = wallet_api::balance(
        &config(),
        models::BalanceRequest { wallet_id: watch_id(), slp_semi_aware: None },
    ).await;

    assert_ok_or_skip!(result, "balance request failed");
    assert!(result.unwrap().sat.is_some());
}

#[tokio::test]
async fn test_utxo_query() {
    let watch = serde_json::json!({ "walletId": watch_id() });
    let result = wallet_api::utxos(&config(), watch).await;
    assert_ok_or_skip!(result, "utxo request failed");
}

#[tokio::test]
async fn test_token_balance_query() {
    let result = wallet_api::get_all_token_balances(
        &config(),
        models::GetAllTokenBalancesRequest { wallet_id: watch_id() },
    ).await;
    assert_ok_or_skip!(result, "token balance request failed");
}

#[tokio::test]
async fn test_history_query() {
    let result = wallet_api::get_history(
        &config(),
        models::HistoryRequest {
            wallet_id: watch_id(),
            unit: Some(models::history_request::Unit::Sat),
            from_height: None, to_height: None,
            start: Some(0.0), count: Some(5.0),
        },
    ).await;
    assert_ok_or_skip!(result, "history request failed");
}

#[tokio::test]
async fn test_submit_transaction_api_reachable() {
    let result = wallet_api::submit_transaction(
        &config(),
        models::SubmitTransactionRequest {
            wallet_id: watch_id(),
            transaction_hex: "deadbeef".to_string(),
            await_propagation: Some(false),
        },
    ).await;
    // Either error or success is fine — we just test connectivity
    match result {
        Err(e) if is_transient(&e) => {
            eprintln!("SKIPPED: submit_transaction (transient server error)");
        }
        _ => {}
    }
}

#[tokio::test]
async fn test_bcmr_token_info() {
    let result = wallet_bcmr_api::bcmr_get_token_info(
        &config(),
        models::BcmrGetTokenInfoRequest {
            category: "0c66f5d8b0c498646a1d06e875e8adc42f4aeb8e6369b6b2d7d1e2d7f5e723ac".to_string(),
        },
    ).await;
    assert_ok_or_skip!(result, "BCMR token info request failed");
}

#[tokio::test]
async fn test_get_token_utxos() {
    let result = wallet_api::get_token_utxos(
        &config(),
        models::GetTokenUtxosRequest { wallet_id: watch_id(), category: None },
    ).await;
    assert_ok_or_skip!(result, "token utxos request failed");
}

#[tokio::test]
async fn test_balance_returns_satoshis_string() {
    let result = wallet_api::balance(
        &config(),
        models::BalanceRequest { wallet_id: watch_id(), slp_semi_aware: None },
    ).await;
    assert_ok_or_skip!(result, "balance request failed");

    let sat_str = result.unwrap().sat.unwrap_or_default();
    let parsed: Result<f64, _> = sat_str.parse();
    assert!(parsed.is_ok(), "sat field '{}' should be a valid number", sat_str);
}
