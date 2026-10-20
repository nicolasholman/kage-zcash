//! Demo agent: on 402 runs spendguard, pays shielded on testnet, retries with receipt.
//! Scaffold only: validates config, then exits non-zero until milestone M3 lands.
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
    let _ = require("X402Z_AGENT_WALLET_DIR");
    let _ = require("X402Z_TARGET_URL");
    eprintln!("error: x402z-agent not implemented yet (milestone M3)");
    exit(1);
}
