use anyhow::Result;
use clap::{Parser, Subcommand};

mod bcmr;
mod cli;
mod config;
mod constants;
mod crypto;
mod electrumx;
mod network;
mod storage;
mod transaction;
mod types;
mod wallet;
mod x402;

#[derive(Parser)]
#[command(name = "cashr", version, about = "cashr -- Bitcoin Cash wallet CLI")]
struct Cli {
    /// Wallet name (uses default wallet if omitted)
    #[arg(short = 'n', long = "name", global = true)]
    name: Option<String>,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Wallet management (create, import, info, export, list)
    Wallet {
        #[command(subcommand)]
        command: WalletCommand,
    },
    /// Address derivation and listing
    Address {
        #[command(subcommand)]
        command: AddressCommand,
    },
    /// Check BCH balance
    Balance {
        /// Use chipnet (testnet)
        #[arg(long)]
        chipnet: bool,
        /// Show balance for a specific CashToken category
        #[arg(long)]
        token: Option<String>,
        /// Display amounts in satoshis only
        #[arg(long)]
        sats: bool,
        /// Show per-address balance breakdown
        #[arg(long, short)]
        verbose: bool,
    },
    /// Send BCH to an address
    Send {
        /// Recipient CashAddress
        address: String,
        /// Amount to send
        amount: String,
        /// Unit: bch or sats
        #[arg(long, default_value = "bch")]
        unit: String,
        /// Use chipnet (testnet)
        #[arg(long)]
        chipnet: bool,
    },
    /// Send all spendable BCH to an address (drain wallet)
    SendAll {
        /// Recipient CashAddress
        address: String,
        /// Use chipnet (testnet)
        #[arg(long)]
        chipnet: bool,
    },
    /// Generate receive address and QR code
    Receive {
        /// Address index to use
        #[arg(long)]
        index: Option<u32>,
        /// Use chipnet (testnet)
        #[arg(long)]
        chipnet: bool,
        /// Generate token-aware address, optionally for a specific category
        #[arg(long, num_args = 0..=1, default_missing_value = "")]
        token: Option<String>,
        /// Requested payment amount (token units when used with --token)
        #[arg(long)]
        amount: Option<String>,
        /// Amount unit: bch or sats (default: bch)
        #[arg(long, default_value = "bch")]
        unit: String,
        /// Suppress QR code display
        #[arg(long)]
        no_qr: bool,
    },
    /// Transaction history
    History {
        /// Use chipnet (testnet)
        #[arg(long)]
        chipnet: bool,
        /// Page number
        #[arg(long, default_value = "1")]
        page: u32,
        /// Filter: all, incoming, outgoing
        #[arg(long, default_value = "all", name = "type")]
        record_type: String,
        /// Filter by CashToken category
        #[arg(long)]
        token: Option<String>,
        /// Display amounts in satoshis only
        #[arg(long)]
        sats: bool,
    },
    /// CashToken operations
    Token {
        #[command(subcommand)]
        command: TokenCommand,
    },
    /// Make a paid HTTP request via x402 protocol
    Pay {
        /// Target URL
        url: String,
        /// HTTP method
        #[arg(short = 'X', long = "method", default_value = "GET")]
        method: String,
        /// HTTP headers (repeatable)
        #[arg(short = 'H', long = "header")]
        headers: Vec<String>,
        /// Request body
        #[arg(short = 'd', long = "body")]
        body: Option<String>,
        /// Use chipnet (testnet)
        #[arg(long)]
        chipnet: bool,
        /// Maximum amount willing to pay (sats)
        #[arg(long)]
        max_amount: Option<u64>,
        /// Override change address
        #[arg(long)]
        change_address: Option<String>,
        /// Payer address index
        #[arg(long)]
        payer: Option<u32>,
        /// Show what would happen without paying
        #[arg(long)]
        dry_run: bool,
        /// Output JSON
        #[arg(long)]
        json: bool,
        /// Skip confirmation prompt
        #[arg(long)]
        confirmed: bool,
    },
    /// Check if a URL requires x402 payment
    Check {
        /// Target URL
        url: String,
        /// HTTP method
        #[arg(short = 'X', long = "method", default_value = "GET")]
        method: String,
        /// HTTP headers (repeatable)
        #[arg(short = 'H', long = "header")]
        headers: Vec<String>,
        /// Request body
        #[arg(short = 'd', long = "body")]
        body: Option<String>,
        /// Use chipnet (testnet)
        #[arg(long)]
        chipnet: bool,
        /// Output JSON
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand)]
enum WalletCommand {
    /// Create a new wallet
    Create {
        /// Wallet name
        name: String,
        /// Use chipnet (testnet)
        #[arg(long)]
        chipnet: bool,
    },
    /// Import an existing wallet from seed phrase
    Import {
        /// Wallet name
        name: String,
        /// Use chipnet (testnet)
        #[arg(long)]
        chipnet: bool,
    },
    /// Show wallet info
    Info {
        /// Use chipnet (testnet)
        #[arg(long)]
        chipnet: bool,
    },
    /// Export wallet seed phrase
    Export,
    /// Delete a wallet
    Delete {
        /// Wallet name to delete
        name: String,
    },
    /// Set a wallet as the default
    Default {
        /// Wallet name to set as default
        name: String,
    },
    /// List all stored wallets
    List {
        /// Show chipnet addresses instead of mainnet
        #[arg(long)]
        chipnet: bool,
    },
}

#[derive(Subcommand)]
enum AddressCommand {
    /// Derive address at a specific index
    Derive {
        /// Address index (default: 0)
        #[arg(default_value = "0")]
        index: u32,
        /// Use chipnet (testnet)
        #[arg(long)]
        chipnet: bool,
        /// Derive token-aware address
        #[arg(long)]
        token: bool,
    },
    /// List multiple addresses
    List {
        /// Number of addresses to show
        #[arg(short = 'c', long = "count", default_value = "5")]
        count: u32,
        /// Use chipnet (testnet)
        #[arg(long)]
        chipnet: bool,
        /// Show token-aware addresses
        #[arg(long)]
        token: bool,
    },
}

#[derive(Subcommand)]
enum TokenCommand {
    /// List fungible CashTokens
    List {
        /// Use chipnet (testnet)
        #[arg(long)]
        chipnet: bool,
        /// Show per-address token breakdown
        #[arg(long, short)]
        verbose: bool,
    },
    /// Show info for a specific CashToken
    Info {
        /// Token category ID (64-char hex)
        category: String,
        /// Use chipnet (testnet)
        #[arg(long)]
        chipnet: bool,
    },
    /// Send fungible CashTokens
    Send {
        /// Recipient address
        address: String,
        /// Amount in base units
        amount: String,
        /// Token category ID
        #[arg(long)]
        token: String,
        /// Use chipnet (testnet)
        #[arg(long)]
        chipnet: bool,
    },
    /// Send an NFT
    SendNft {
        /// Recipient address
        address: String,
        /// Token category ID
        #[arg(long)]
        token: String,
        /// NFT commitment (hex)
        #[arg(long)]
        commitment: String,
        /// NFT capability: none, minting, mutable
        #[arg(long, default_value = "none")]
        capability: String,
        /// UTXO txid (auto-detect if omitted)
        #[arg(long)]
        txid: Option<String>,
        /// UTXO vout (auto-detect if omitted)
        #[arg(long)]
        vout: Option<u32>,
        /// Use chipnet (testnet)
        #[arg(long)]
        chipnet: bool,
    },
}

fn main() {
    if let Err(e) = run() {
        eprintln!("{}", format!("Error: {:#}", e).as_str());
        std::process::exit(1);
    }
}

#[tokio::main]
async fn run() -> Result<()> {
    let cli = Cli::parse();
    let wallet_name = cli.name.as_deref();

    // Auto-detect network from wallet's stored .net file.
    // If --chipnet is explicitly passed, it overrides the stored value.
    let auto_chipnet = crate::storage::resolve_chipnet(wallet_name);

    match cli.command {
        Commands::Wallet { command } => match command {
            WalletCommand::Create { name, chipnet } => {
                cli::wallet::create(&name, chipnet).await?;
            }
            WalletCommand::Import { name, chipnet } => {
                cli::wallet::import(&name, chipnet).await?;
            }
            WalletCommand::Info { chipnet } => {
                cli::wallet::info(wallet_name, chipnet || auto_chipnet).await?;
            }
            WalletCommand::Export => {
                cli::wallet::export(wallet_name)?;
            }
            WalletCommand::Delete { name } => {
                cli::wallet::delete(&name)?;
            }
            WalletCommand::Default { name } => {
                cli::wallet::set_default(&name)?;
            }
            WalletCommand::List { chipnet } => {
                cli::wallet::list(wallet_name, chipnet || auto_chipnet)?;
            }
        },
        Commands::Address { command } => match command {
            AddressCommand::Derive {
                index,
                chipnet,
                token,
            } => {
                cli::address::derive(wallet_name, index, chipnet || auto_chipnet, token).await?;
            }
            AddressCommand::List {
                count,
                chipnet,
                token,
            } => {
                cli::address::list(wallet_name, count, chipnet || auto_chipnet, token).await?;
            }
        },
        Commands::Balance {
            chipnet,
            token,
            sats,
            verbose,
        } => {
            cli::balance::run(wallet_name, chipnet || auto_chipnet, token.as_deref(), sats, verbose).await?;
        }
        Commands::Send {
            address,
            amount,
            unit,
            chipnet,
        } => {
            cli::send::run(wallet_name, &address, &amount, &unit, chipnet || auto_chipnet).await?;
        }
        Commands::SendAll { address, chipnet } => {
            cli::send::run_send_all(wallet_name, &address, chipnet || auto_chipnet).await?;
        }
        Commands::Receive {
            index,
            chipnet,
            token,
            amount,
            unit,
            no_qr,
        } => {
            cli::receive::run(
                wallet_name,
                index,
                chipnet || auto_chipnet,
                token.as_deref(),
                amount.as_deref(),
                &unit,
                no_qr,
            )
            .await?;
        }
        Commands::History {
            chipnet,
            page,
            record_type,
            token,
            sats,
        } => {
            cli::history::run(
                wallet_name,
                chipnet || auto_chipnet,
                page,
                &record_type,
                token.as_deref(),
                sats,
            )
            .await?;
        }
        Commands::Token { command } => match command {
            TokenCommand::List { chipnet, verbose } => {
                cli::token::list(wallet_name, chipnet || auto_chipnet, verbose).await?;
            }
            TokenCommand::Info { category, chipnet } => {
                cli::token::info(wallet_name, &category, chipnet || auto_chipnet).await?;
            }
            TokenCommand::Send {
                address,
                amount,
                token,
                chipnet,
            } => {
                cli::token::send(wallet_name, &address, &amount, &token, chipnet || auto_chipnet).await?;
            }
            TokenCommand::SendNft {
                address,
                token,
                commitment,
                capability,
                txid,
                vout,
                chipnet,
            } => {
                cli::token::send_nft(cli::token::SendNftArgs {
                    wallet_name,
                    address: &address,
                    category: &token,
                    commitment: &commitment,
                    capability: &capability,
                    txid: txid.as_deref(),
                    vout,
                    chipnet: chipnet || auto_chipnet,
                })
                .await?;
            }
        },
        Commands::Pay {
            url,
            method,
            headers,
            body,
            chipnet,
            max_amount,
            change_address,
            payer,
            dry_run,
            json,
            confirmed,
        } => {
            cli::pay::run(
                wallet_name,
                &url,
                &method,
                &headers,
                body.as_deref(),
                chipnet || auto_chipnet,
                max_amount,
                change_address.as_deref(),
                payer,
                dry_run,
                json,
                confirmed,
            )
            .await?;
        }
        Commands::Check {
            url,
            method,
            headers,
            body,
            chipnet,
            json,
        } => {
            cli::check::run(
                wallet_name,
                &url,
                &method,
                &headers,
                body.as_deref(),
                chipnet || auto_chipnet,
                json,
            )
            .await?;
        }
    }

    Ok(())
}
