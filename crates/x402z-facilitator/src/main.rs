//! Facilitator: merchant UFVK only (no spend key), scans via lightwalletd, signs ed25519 receipts.
//! Scaffold only: validates config, then exits non-zero until milestone M2 lands.
use std::process::exit;

fn require(var: &str) -> String {
    match std::env::var(var) {
        Ok(v) if !v.trim().is_empty() => v,
        _ => {
            eprintln!("error: {var} not configured (see .env.example)");
            exit(2);
        }
    }
}

fn main() {
    let _ = require("ZCASH_LWD_URL");
    let _ = require("X402Z_MERCHANT_UFVK");
    let _ = require("X402Z_RECEIPT_SIGNING_KEY");
    eprintln!("error: x402z-facilitator not implemented yet (milestone M2)");
    exit(1);
}
