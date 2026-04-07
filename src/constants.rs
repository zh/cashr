/// Shared constants for the cashr wallet.

/// Satoshis per BCH (1 BCH = 100,000,000 satoshis).
pub const SATS_PER_BCH: f64 = 1e8;

/// Default number of transactions per page in history.
pub const DEFAULT_PAGE_SIZE: usize = 10;

/// Satoshis reserved for transaction fees in token/NFT sends.
pub const FEE_RESERVE_SATS: u64 = 2000;

/// Default fee rate (satoshis per byte).
pub const DEFAULT_FEE_RATE: f64 = 1.2;

/// Maximum number of concurrent API requests to the REST server.
pub const MAX_CONCURRENT_REQUESTS: usize = 4;
