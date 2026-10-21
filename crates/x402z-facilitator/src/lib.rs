//! x402z facilitator library: everything except the HTTP shell.
//!
//! - [`lwd`]: lightwalletd gRPC client (chain tip, fetch a full transaction by txid).
//! - [`verify`]: trial-decrypts a fetched transaction with the merchant UIVK and checks it pays
//!   a 402 challenge (recipient = `payTo`, memo = `memoNonce`, value >= `amountZat`).
//! - [`receipt`]: ed25519-signed receipts in the `X-PAYMENT-RECEIPT` wire format.
//! - [`store`]: durable nonce -> txid bindings (SQLite).
//!
//! The facilitator only ever holds an incoming viewing key; nothing here can spend.

pub mod lwd;
pub mod receipt;
pub mod store;
pub mod verify;

/// The facilitator is testnet-only (no mainnet).
pub const NETWORK: zcash_protocol::consensus::Network = zcash_protocol::consensus::Network::TestNetwork;
