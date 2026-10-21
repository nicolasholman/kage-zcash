//! Demo agent. `x402z-agent [pay]`:
//! GET X402Z_TARGET_URL -> 402 challenge -> spendguard (cap, allowlist, approval) -> shielded
//! testnet payment from a real wallet (memo = nonce) -> POST txid to the facilitator until it
//! returns a receipt -> retry with X-PAYMENT-RECEIPT -> print the paid response.
//!
//! Other commands: `new-wallet` (create a seed + wallet), `address`, `balance`.
//!
//! Exit codes: 0 paid/served, 1 runtime failure (network, server, facilitator), 2 missing or
//! invalid config, 3 refused by spendguard (nothing built or broadcast), 4 wallet not usable
//! (no seed, or not enough testnet ZEC), 5 the facilitator rejected the payment.

mod cache;
mod ledger;
mod wallet;

use std::{
    io::{IsTerminal, Write},
    path::PathBuf,
    process::exit,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use secrecy::SecretVec;
use spendguard::{Decision, Policy, SpendRequest};
use x402z_facilitator::lwd;
use x402z_protocol::{PaymentRequired, VerifyRequest, VerifyResponse, VerifyStatus, RECEIPT_HEADER, VERIFY_PATH};
use zcash_protocol::consensus::BlockHeight;

use ledger::{AuditEntry, LedgerFile};

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

fn parse_u64(var: &str) -> Option<u64> {
    optional(var).map(|s| {
        s.parse::<u64>().unwrap_or_else(|_| {
            eprintln!("error: {var} must be a non-negative integer");
            exit(2);
        })
    })
}

fn die(code: i32, msg: impl std::fmt::Display) -> ! {
    eprintln!("error: {msg}");
    exit(code);
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
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

fn seed() -> Option<SecretVec<u8>> {
    optional("X402Z_AGENT_SEED").map(|p| {
        wallet::seed_from_phrase(&p).unwrap_or_else(|e| die(2, format!("X402Z_AGENT_SEED invalid: {e}")))
    })
}

/// Open the wallet, creating account 0 from the seed if the database has none.
async fn open_wallet(client: &mut lwd::Client, tip: BlockHeight) -> (wallet::Db, zcash_client_sqlite::AccountUuid, Option<SecretVec<u8>>) {
    let dir = wallet_dir();
    let seed = seed();
    let mut db = wallet::open(&dir, seed.as_ref()).unwrap_or_else(|e| die(4, e));
    let id = match wallet::account(&db).unwrap_or_else(|e| die(4, e)) {
        Some(id) => id,
        None => {
            let Some(seed) = seed.as_ref() else {
                die(
                    4,
                    format!(
                        "no wallet: {} has no account and X402Z_AGENT_SEED is not configured. \
                         Set X402Z_AGENT_SEED to a funded testnet wallet's 24-word BIP 39 phrase \
                         (plus X402Z_AGENT_BIRTHDAY, the height it was created at), or run \
                         `x402z-agent new-wallet` and send testnet ZEC (TAZ) to the address it prints. \
                         Not paying.",
                        dir.display()
                    ),
                )
            };
            let birthday = parse_u64("X402Z_AGENT_BIRTHDAY")
                .map(|h| BlockHeight::from_u32(h as u32))
                .unwrap_or_else(|| {
                    eprintln!("warning: X402Z_AGENT_BIRTHDAY not set; scanning from the current tip only (older funds will not be found)");
                    tip
                });
            wallet::create_account(&mut db, client, seed, birthday).await.unwrap_or_else(|e| die(1, e))
        }
    };
    (db, id, seed)
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

async fn cmd_balance() {
    let (mut client, tip) = connect_lwd().await;
    let (mut db, id, _) = open_wallet(&mut client, tip).await;
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

fn policy() -> Policy {
    Policy {
        daily_cap_zat: parse_u64("X402Z_AGENT_DAILY_CAP_ZAT").unwrap_or(500_000),
        allowed_hosts: optional("X402Z_AGENT_ALLOWED_HOSTS")
            .map(|s| s.split(',').map(|h| h.trim().to_string()).filter(|h| !h.is_empty()).collect())
            .unwrap_or_default(),
        approval_threshold_zat: parse_u64("X402Z_AGENT_APPROVAL_ZAT"),
    }
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

async fn fetch(http: &reqwest::Client, url: &str, receipt: Option<&str>) -> reqwest::Response {
    let mut req = http.get(url);
    if let Some(r) = receipt {
        req = req.header(RECEIPT_HEADER, r);
    }
    req.send().await.unwrap_or_else(|e| die(1, format!("GET {url} failed: {e}")))
}

async fn cmd_pay() {
    let target = require("X402Z_TARGET_URL");
    let ledger_path = PathBuf::from(require("X402Z_AGENT_LEDGER"));
    let facilitator = require("X402Z_FACILITATOR_URL");
    let _ = wallet_dir();
    let agent = optional("X402Z_AGENT_NAME").unwrap_or_else(|| "agent-1".into());
    let policy = policy();
    let mut ledger = LedgerFile::load(&ledger_path).unwrap_or_else(|e| die(2, e));
    let http = reqwest::Client::builder().timeout(Duration::from_secs(30)).build().unwrap_or_else(|e| die(1, e));

    // 1. Ask for the resource.
    let url = reqwest::Url::parse(&target).unwrap_or_else(|e| die(2, format!("X402Z_TARGET_URL invalid: {e}")));
    let host = match (url.host_str(), url.port()) {
        (Some(h), Some(p)) => format!("{h}:{p}"),
        (Some(h), None) => h.to_string(),
        _ => die(2, "X402Z_TARGET_URL has no host"),
    };
    let resp = fetch(&http, &target, None).await;
    let status = resp.status();
    if status.is_success() {
        println!("{}", resp.text().await.unwrap_or_default());
        eprintln!("{target} answered {status} without payment");
        return;
    }
    if status.as_u16() != 402 {
        die(1, format!("{target} answered {status}, expected 402: {}", resp.text().await.unwrap_or_default()));
    }
    let ch: PaymentRequired = resp.json().await.unwrap_or_else(|e| die(1, format!("402 body is not an x402z challenge: {e}")));
    ch.validate(now()).unwrap_or_else(|e| die(1, format!("402 challenge rejected: {e:?}")));
    eprintln!(
        "402 challenge: pay {} zat to {}… memo {} (expires in {}s)",
        ch.amount_zat,
        &ch.pay_to[..20.min(ch.pay_to.len())],
        ch.memo_nonce,
        ch.expires_at.saturating_sub(now())
    );

    // 2. spendguard, before any wallet work.
    let t = now();
    let guard = ledger.guard(&agent, &policy, t);
    let req = SpendRequest { agent: &agent, host: &host, amount_zat: ch.amount_zat, now_unix: t };
    let mut audit = AuditEntry {
        ts: t,
        agent: agent.clone(),
        host: host.clone(),
        amount_zat: ch.amount_zat,
        decision: "allowed".into(),
        reason: None,
        memo_nonce: Some(ch.memo_nonce.clone()),
        txid: None,
    };
    let spent_before = guard.spent_today(&agent, t);
    let refusal = match guard.check(&req) {
        Decision::Allow => None,
        Decision::Deny(d) => Some(d.to_string()),
        Decision::NeedsApproval { threshold_zat } => {
            if ask_approval(&host, ch.amount_zat, threshold_zat) {
                audit.reason = Some(format!("approved by operator (>= {threshold_zat} zat)"));
                None
            } else {
                audit.decision = "needs_approval".into();
                Some(format!("approval required (>= {threshold_zat} zat) and not given"))
            }
        }
    };
    if let Some(reason) = refusal {
        if audit.decision == "allowed" {
            audit.decision = "denied".into();
        }
        audit.reason = Some(format!("{reason}; nothing broadcast"));
        ledger.upsert_agent(&agent, &policy, spent_before);
        ledger.append(audit);
        ledger.save().unwrap_or_else(|e| die(1, e));
        die(3, format!("spendguard refused the payment: {reason}. No transaction was built or broadcast."));
    }
    eprintln!("spendguard: allowed ({} of {} zat already spent today)", spent_before, policy.daily_cap_zat);

    // 3. Pay from the wallet. Every failure from here is recorded with the allowed decision.
    let record_failure = |ledger: &mut LedgerFile, audit: &AuditEntry, why: &str| {
        let mut a = audit.clone();
        a.reason = Some(format!("allowed, but not paid: {why}"));
        ledger.upsert_agent(&agent, &policy, spent_before);
        ledger.append(a);
        let _ = ledger.save();
    };
    let (mut client, tip) = connect_lwd().await;
    let dir = wallet_dir();
    let seed = seed();
    let Some(seed) = seed else {
        let why = format!(
            "X402Z_AGENT_SEED not configured: no funded testnet wallet to pay from. Set X402Z_AGENT_SEED \
             (24-word BIP 39 phrase of a wallet holding at least {} zat of testnet ZEC) and \
             X402Z_AGENT_BIRTHDAY, or run `x402z-agent new-wallet` and fund the printed address",
            ch.amount_zat + wallet::MIN_FEE_ZAT
        );
        record_failure(&mut ledger, &audit, "X402Z_AGENT_SEED not configured");
        die(4, format!("{why}. Not paying."));
    };
    let (mut db, id, _) = open_wallet(&mut client, tip).await;
    let usk = wallet::spending_key(&db, id, &seed).unwrap_or_else(|e| die(2, e));
    eprintln!("syncing wallet in {}...", dir.display());
    wallet::sync(&mut db, &mut client).await.unwrap_or_else(|e| die(1, e));
    let bal = wallet::balance(&db, id).unwrap_or_else(|e| die(1, e));
    let need = ch.amount_zat + wallet::MIN_FEE_ZAT;
    if bal.spendable_zat < need {
        let ua = wallet::ufvk(&db, id).and_then(|u| wallet::address(&u)).unwrap_or_default();
        let why = format!(
            "insufficient testnet balance: spendable {} zat (total {}, pending {}), need at least {} zat \
             ({} + fee). Send TAZ to {ua} and wait for 1 confirmation",
            bal.spendable_zat, bal.total_zat, bal.pending_zat, need, ch.amount_zat
        );
        record_failure(&mut ledger, &audit, &format!("insufficient balance ({} zat spendable)", bal.spendable_zat));
        die(4, format!("{why}. Not paying."));
    }
    let txid = match wallet::pay(&mut db, &mut client, id, usk, &ch.pay_to, ch.amount_zat, &ch.memo_nonce).await {
        Ok(t) => t,
        Err(e) => {
            record_failure(&mut ledger, &audit, &e);
            die(1, e);
        }
    };
    let txid_hex = txid.to_string();
    audit.txid = Some(txid_hex.clone());
    audit.reason.get_or_insert_with(|| "paid".into());
    ledger.upsert_agent(&agent, &policy, spent_before.saturating_add(ch.amount_zat));
    ledger.append(audit);
    ledger.save().unwrap_or_else(|e| die(1, e));
    eprintln!("paid: tx {txid_hex} ({} zat, memo {})", ch.amount_zat, ch.memo_nonce);

    // 4. Get a receipt from the facilitator.
    let verify_url = format!("{}{VERIFY_PATH}", facilitator.trim_end_matches('/'));
    let deadline = now() + parse_u64("X402Z_AGENT_CONFIRM_TIMEOUT_SECS").unwrap_or(900);
    let body = VerifyRequest { challenge: ch.clone(), txid: txid_hex.clone() };
    let receipt = loop {
        let r = http.post(&verify_url).json(&body).send().await;
        let v: Option<VerifyResponse> = match r {
            Ok(r) => r.json().await.ok(),
            Err(e) => {
                eprintln!("facilitator {verify_url} unreachable: {e}");
                None
            }
        };
        match v {
            Some(VerifyResponse { status: VerifyStatus::Confirmed, receipt: Some(rc), .. }) => break rc,
            Some(VerifyResponse { status: VerifyStatus::Rejected, error, .. }) => {
                die(5, format!("facilitator rejected payment tx {txid_hex}: {}", error.unwrap_or_default()))
            }
            Some(v) => eprintln!("facilitator: {:?}: {}", v.status, v.error.unwrap_or_default()),
            None => {}
        }
        if now() >= deadline {
            die(1, format!("no receipt for tx {txid_hex} before timeout; it may still confirm. Retry verification later."));
        }
        tokio::time::sleep(Duration::from_secs(15)).await;
    };
    eprintln!("receipt issued");

    // 5. Retry with the receipt.
    let resp = fetch(&http, &target, Some(&receipt)).await;
    let status = resp.status();
    let err = resp.headers().get("x-payment-error").and_then(|v| v.to_str().ok()).map(str::to_string);
    let text = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        die(1, format!("{target} answered {status} with the receipt: {}", err.unwrap_or(text)));
    }
    println!("{text}");
    eprintln!("{target} answered {status}: paid content delivered");
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
