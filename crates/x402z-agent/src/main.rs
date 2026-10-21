//! Demo agent. `x402z-agent [pay]`:
//! GET X402Z_TARGET_URL -> x402 402 -> spendguard (cap, allowlist, approval) -> shielded
//! testnet payment from its own wallet (memo = nonce) -> retry with `X-PAYMENT` (base64
//! `{x402Version, scheme, network, payload: {txid, memoNonce}}`) until the server settles it
//! -> print the paid response.
//!
//! Other commands: `new-wallet` (create a seed + wallet), `address`, `balance`.
//!
//! Kage Tab (one payment, N unlinkable requests):
//! - `tab-buy <n>`: buy n tokens from X402Z_TARGET_URL's server, paid from the agent's wallet;
//! - `tab-buy <n> --external`: spendguard-check n tokens and print the ZIP 321 request for any
//!   wallet to pay, then `tab-redeem <txid> [memoNonce]` to collect the tokens;
//! - `tab-fetch [url]`: GET the URL (default X402Z_TARGET_URL) spending one token;
//! - `tab-status`: tokens left per origin and pending purchases.
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
    tab, wallet,
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

fn print_bought(b: &tab::Bought) {
    println!(
        "{}",
        serde_json::json!({
            "tab": { "tokens": b.n, "keyId": b.key_id, "amountZat": b.amount_zat, "txid": b.txid, "memoNonce": b.memo_nonce,
                     "issueWaitSecs": b.issue_wait_secs, "explorer": format!("https://testnet.zecblock.com/tx/{}", b.txid) }
        })
    );
}

async fn cmd_tab_buy(args: &[String]) {
    let n: u32 = args.first().and_then(|a| a.parse().ok()).unwrap_or_else(|| die(2, "usage: x402z-agent tab-buy <n> [--external]"));
    let target = require("X402Z_TARGET_URL");
    let cfg = config();
    if args.iter().any(|a| a == "--external") {
        let p = tab::start_external(&cfg, &target, n, &ask_approval).await.unwrap_or_else(|e| die(e.exit_code(), e));
        println!(
            "{}",
            serde_json::json!({
                "pending": { "tokens": p.n, "amountZat": p.amount_zat, "payTo": p.pay_to, "memoNonce": p.memo_nonce,
                             "expiresAt": p.expires_at, "paymentUri": p.payment_uri, "keyId": p.offer.key_id }
            })
        );
        eprintln!(
            "pay {} zat to the URI above from any wallet (memo {}), then run `x402z-agent tab-redeem <txid>`",
            p.amount_zat, p.memo_nonce
        );
        return;
    }
    let b = tab::buy(&cfg, &target, n, &ask_approval).await.unwrap_or_else(|e| die(e.exit_code(), e));
    print_bought(&b);
}

async fn cmd_tab_redeem(args: &[String]) {
    let txid = args.first().unwrap_or_else(|| die(2, "usage: x402z-agent tab-redeem <txid> [memoNonce]"));
    let cfg = config();
    let b = tab::finish_external(&cfg, txid, args.get(1).map(String::as_str)).await.unwrap_or_else(|e| die(e.exit_code(), e));
    print_bought(&b);
}

async fn cmd_tab_fetch(args: &[String]) {
    let url = args.first().cloned().unwrap_or_else(|| require("X402Z_TARGET_URL"));
    let cfg = config();
    match tab::fetch(&cfg, &url).await {
        Ok(Some(f)) => {
            println!("{}", f.body);
            if let Some(s) = f.settlement {
                eprintln!("X-PAYMENT-RESPONSE: success={} transaction={}", s.success, s.transaction);
            }
            eprintln!("served {} in {} ms with a tab token; {} token(s) left", f.status, f.elapsed_ms, f.remaining);
        }
        Ok(None) => die(4, format!("no tab tokens for {url}; buy some with `x402z-agent tab-buy <n>`")),
        Err(e) => die(e.exit_code(), e),
    }
}

fn cmd_tab_status() {
    let cfg = config();
    let path = tab::tab_path(&cfg).unwrap_or_else(|e| die(e.exit_code(), e));
    let f = tab::TabFile::open(&path).unwrap_or_else(|e| die(e.exit_code(), e));
    let now = pay::now();
    let mut origins: Vec<&str> = f.state.tokens.iter().map(|t| t.origin.as_str()).collect();
    origins.sort();
    origins.dedup();
    let tokens: serde_json::Map<String, serde_json::Value> = origins.iter().map(|o| (o.to_string(), f.count(o, now).into())).collect();
    let pending: Vec<_> = f
        .state
        .pending
        .iter()
        .map(|p| serde_json::json!({ "memoNonce": p.memo_nonce, "tokens": p.n, "amountZat": p.amount_zat, "expiresAt": p.expires_at }))
        .collect();
    println!("{}", serde_json::json!({ "file": path.display().to_string(), "tokens": tokens, "pending": pending }));
}

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let rest = args.get(1..).unwrap_or_default();
    match args.first().map(String::as_str) {
        None | Some("pay") => cmd_pay().await,
        Some("new-wallet") => cmd_new_wallet().await,
        Some("address") => cmd_address().await,
        Some("balance") => cmd_balance().await,
        Some("tab-buy") => cmd_tab_buy(rest).await,
        Some("tab-redeem") => cmd_tab_redeem(rest).await,
        Some("tab-fetch") => cmd_tab_fetch(rest).await,
        Some("tab-status") => cmd_tab_status(),
        Some(other) => die(
            2,
            format!("unknown command '{other}' (usage: x402z-agent [pay|new-wallet|address|balance|tab-buy|tab-redeem|tab-fetch|tab-status])"),
        ),
    }
}
