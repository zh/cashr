/// Shared constants for the cashr wallet.
/// Satoshis per BCH (1 BCH = 100,000,000 satoshis).
pub const SATS_PER_BCH: f64 = 1e8;

/// Satoshis reserved for transaction fees in token/NFT sends.
pub const FEE_RESERVE_SATS: u64 = 2000;

/// Default fee rate (satoshis per byte).
pub const DEFAULT_FEE_RATE: f64 = 1.2;
