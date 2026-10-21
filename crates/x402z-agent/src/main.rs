//! Demo agent. `x402z-agent [pay]`:
//! GET X402Z_TARGET_URL -> x402 402 -> spendguard (cap, allowlist, approval) -> shielded
//! testnet payment from its own wallet (memo = nonce) -> retry with `X-PAYMENT` (base64
//! `{x402Version, scheme, network, payload: {txid, memoNonce}}`) until the server settles it
//! -> print the paid response.
//!
//! Other commands: `new-wallet` (create a seed + wallet), `address`, `balance`.
//!
//! Exit codes: 0 paid/served, 1 runtime failure (network, server, facilitator), 2 missing or
//! invalid config, 3 refused by spendguard (nothing built or broadcast), 4 wallet not usable
//! (no seed, or not enough testnet ZEC), 5 the server/facilitator rejected the payment.

use std::{
    io::{IsTerminal, Write},
    path::PathBuf,
    process::exit,
};

use x402z_agent::{
    pay::{self, AgentConfig},
    wallet,
};
use x402z_facilitator::lwd;
use zcash_protocol::consensus::BlockHeight;

fn require(var: &str) -> String {
    match std::env::var(var) {
        Ok(v) if !v.trim().is_empty() => v.trim().to_string(),
        _ => {
            eprintln!("error: {var} not configured (see .env.example)");
            exit(2);
        }
    }
}

fn optional(var: &str) -> Option<String> {
    std::env::var(var).ok().map(|v| v.trim().to_string()).filter(|v| !v.is_empty())
}

fn die(code: i32, msg: impl std::fmt::Display) -> ! {
    eprintln!("error: {msg}");
    exit(code);
}

fn lwd_url() -> String {
    optional("ZCASH_LWD_URL").unwrap_or_else(|| {
        eprintln!("ZCASH_LWD_URL not set; using {}", lwd::DEFAULT_LWD_URL);
        lwd::DEFAULT_LWD_URL.to_string()
    })
}

async fn connect_lwd() -> (lwd::Client, BlockHeight) {
    let url = lwd_url();
    let mut client = lwd::connect(&url).await.unwrap_or_else(|e| die(1, e));
    let info = lwd::info(&mut client).await.unwrap_or_else(|e| die(1, format!("{e} ({url})")));
    if info.chain_name != "test" {
        die(1, format!("lightwalletd at {url} serves chain '{}', expected testnet", info.chain_name));
    }
    let tip = lwd::chain_tip(&mut client).await.unwrap_or_else(|e| die(1, format!("{e} ({url})")));
    eprintln!("lightwalletd {url}: testnet, chain tip {tip}");
    (client, tip)
}

fn wallet_dir() -> PathBuf {
    PathBuf::from(require("X402Z_AGENT_WALLET_DIR"))
}

async fn cmd_new_wallet() {
    let dir = wallet_dir();
    let (mut client, tip) = connect_lwd().await;
    let mut db = wallet::open(&dir, None).unwrap_or_else(|e| die(4, e));
    if wallet::account(&db).unwrap_or_else(|e| die(4, e)).is_some() {
        die(2, format!("{} already holds a wallet; refusing to overwrite it", dir.display()));
    }
    let phrase = wallet::new_phrase();
    let seed = wallet::seed_from_phrase(&phrase).unwrap_or_else(|e| die(1, e));
    let id = wallet::create_account(&mut db, &mut client, &seed, tip).await.unwrap_or_else(|e| die(1, e));
    let ufvk = wallet::ufvk(&db, id).unwrap_or_else(|e| die(1, e));
    let ua = wallet::address(&ufvk).unwrap_or_else(|e| die(1, e));
    println!("# New testnet wallet in {} (birthday {tip}). Put these in .env; the phrase is the only backup.", dir.display());
    println!("X402Z_AGENT_SEED=\"{phrase}\"");
    println!("X402Z_AGENT_BIRTHDAY={tip}");
    println!("# Fund this shielded address with testnet ZEC (TAZ):");
    println!("address={ua}");
    println!("# Incoming viewing key (for X402Z_MERCHANT_UIVK if this wallet is a merchant):");
    println!("X402Z_MERCHANT_UIVK={}", wallet::encode_uivk(&ufvk).unwrap_or_else(|e| die(1, e)));
}

async fn cmd_address() {
    let db = wallet::open(&wallet_dir(), None).unwrap_or_else(|e| die(4, e));
    let id = wallet::account(&db)
        .unwrap_or_else(|e| die(4, e))
        .unwrap_or_else(|| die(4, "no wallet account yet; run `x402z-agent new-wallet` or `x402z-agent balance` with X402Z_AGENT_SEED"));
    let ufvk = wallet::ufvk(&db, id).unwrap_or_else(|e| die(1, e));
    println!("address={}", wallet::address(&ufvk).unwrap_or_else(|e| die(1, e)));
    println!("uivk={}", wallet::encode_uivk(&ufvk).unwrap_or_else(|e| die(1, e)));
}

fn ask_approval(host: &str, amount_zat: u64, threshold: u64) -> bool {
    if !std::io::stdin().is_terminal() {
        eprintln!("spendguard: {amount_zat} zat >= approval threshold {threshold} zat and stdin is not a terminal; declining");
        return false;
    }
    eprint!("spendguard: pay {amount_zat} zat ({:.8} TAZ) to {host}? [y/N] ", amount_zat as f64 / 1e8);
    let _ = std::io::stderr().flush();
    let mut line = String::new();
    std::io::stdin().read_line(&mut line).is_ok() && matches!(line.trim(), "y" | "Y" | "yes")
}

fn config() -> AgentConfig {
    AgentConfig::from_env().unwrap_or_else(|e| die(e.exit_code(), e))
}

async fn cmd_balance() {
    let cfg = config();
    let (mut client, tip) = connect_lwd().await;
    let (mut db, id) = pay::open_wallet(&cfg, &mut client, tip).await.unwrap_or_else(|e| die(e.exit_code(), e));
    eprintln!("syncing wallet...");
    wallet::sync(&mut db, &mut client).await.unwrap_or_else(|e| die(1, e));
    let b = wallet::balance(&db, id).unwrap_or_else(|e| die(1, e));
    let ufvk = wallet::ufvk(&db, id).unwrap_or_else(|e| die(1, e));
    println!("address={}", wallet::address(&ufvk).unwrap_or_else(|e| die(1, e)));
    println!(
        "spendable={} zat total={} zat pending={} zat (scanned to {} / tip {})",
        b.spendable_zat, b.total_zat, b.pending_zat, b.scanned_to, b.chain_tip
    );
}

async fn cmd_pay() {
    let target = require("X402Z_TARGET_URL");
    let cfg = config();
    match pay::paid_fetch(&cfg, &target, &ask_approval).await {
        Ok(f) => {
            println!("{}", f.body);
            if let Some(s) = f.settlement {
                eprintln!("X-PAYMENT-RESPONSE: success={} transaction={}", s.success, s.transaction);
            }
        }
        Err(e) => die(e.exit_code(), e),
    }
}

#[tokio::main]
async fn main() {
    match std::env::args().nth(1).as_deref() {
        None | Some("pay") => cmd_pay().await,
        Some("new-wallet") => cmd_new_wallet().await,
        Some("address") => cmd_address().await,
        Some("balance") => cmd_balance().await,
        Some(other) => die(2, format!("unknown command '{other}' (usage: x402z-agent [pay|new-wallet|address|balance])")),
    }
}
