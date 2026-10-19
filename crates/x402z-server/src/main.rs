//! Paywalled /api/quote server: returns 402 challenge, accepts X-PAYMENT-RECEIPT.
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
    let _ = require("X402Z_MERCHANT_UA");
    let _ = require("X402Z_FACILITATOR_URL");
    eprintln!("error: x402z-server not implemented yet (milestone M2)");
    exit(1);
}
