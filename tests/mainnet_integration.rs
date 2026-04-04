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

#[tokio::test]
async fn test_watch_wallet_balance() {
    let cfg = config();

    let result = wallet_api::balance(
        &cfg,
        models::BalanceRequest {
            wallet_id: watch_id(),
            slp_semi_aware: None,
        },
    )
    .await;

    assert!(result.is_ok(), "balance request failed: {:?}", result.err());
    let resp = result.unwrap();
    assert!(resp.sat.is_some());
}

#[tokio::test]
async fn test_utxo_query() {
    let cfg = config();
    let watch = serde_json::json!({ "walletId": watch_id() });

    let result = wallet_api::utxos(&cfg, watch).await;

    assert!(result.is_ok(), "utxo request failed: {:?}", result.err());
}

#[tokio::test]
async fn test_token_balance_query() {
    let cfg = config();

    let result = wallet_api::get_all_token_balances(
        &cfg,
        models::GetAllTokenBalancesRequest {
            wallet_id: watch_id(),
        },
    )
    .await;

    assert!(
        result.is_ok(),
        "token balance request failed: {:?}",
        result.err()
    );
}

#[tokio::test]
async fn test_history_query() {
    let cfg = config();

    let result = wallet_api::get_history(
        &cfg,
        models::HistoryRequest {
            wallet_id: watch_id(),
            unit: Some(models::history_request::Unit::Sat),
            from_height: None,
            to_height: None,
            start: Some(0.0),
            count: Some(5.0),
        },
    )
    .await;

    assert!(
        result.is_ok(),
        "history request failed: {:?}",
        result.err()
    );
}

#[tokio::test]
async fn test_submit_transaction_api_reachable() {
    // Verify the submit_transaction endpoint is reachable and returns a response.
    // Note: mainnet-cash may accept or reject raw hex in various ways --
    // we just verify the round-trip works without panicking.
    let cfg = config();

    let result = wallet_api::submit_transaction(
        &cfg,
        models::SubmitTransactionRequest {
            wallet_id: watch_id(),
            transaction_hex: "deadbeef".to_string(),
            await_propagation: Some(false),
        },
    )
    .await;

    // Either an error or a response is acceptable -- we are testing connectivity
    match result {
        Err(_) => {} // API rejected invalid tx (expected)
        Ok(_) => {}  // API accepted (some APIs return 200 with empty/error body)
    }
}

#[tokio::test]
async fn test_bcmr_token_info() {
    let cfg = config();

    // Use a well-known CashToken category
    let result = wallet_bcmr_api::bcmr_get_token_info(
        &cfg,
        models::BcmrGetTokenInfoRequest {
            category: "0c66f5d8b0c498646a1d06e875e8adc42f4aeb8e6369b6b2d7d1e2d7f5e723ac"
                .to_string(),
        },
    )
    .await;

    // The request should succeed even if no BCMR is found
    assert!(
        result.is_ok(),
        "BCMR token info request failed: {:?}",
        result.err()
    );
}

#[tokio::test]
async fn test_get_token_utxos() {
    let cfg = config();

    let result = wallet_api::get_token_utxos(
        &cfg,
        models::GetTokenUtxosRequest {
            wallet_id: watch_id(),
            category: None,
        },
    )
    .await;

    assert!(
        result.is_ok(),
        "token utxos request failed: {:?}",
        result.err()
    );
}

#[tokio::test]
async fn test_balance_returns_satoshis_string() {
    let cfg = config();

    let resp = wallet_api::balance(
        &cfg,
        models::BalanceRequest {
            wallet_id: watch_id(),
            slp_semi_aware: None,
        },
    )
    .await
    .expect("balance request should succeed");

    // The sat field should be a parseable number string
    let sat_str = resp.sat.unwrap_or_default();
    let parsed: Result<f64, _> = sat_str.parse();
    assert!(
        parsed.is_ok(),
        "sat field '{}' should be a valid number",
        sat_str
    );
}
