//! The paying flow, shared by the `x402z-agent` binary and the MCP server:
//! GET url -> x402 402 -> spendguard (cap, allowlist, approval) -> shielded testnet payment from
//! the agent's own wallet (memo = nonce) -> retry with `X-PAYMENT` until the server answers 200
//! (it settles through the merchant's facilitator) -> paid body + `X-PAYMENT-RESPONSE`.
//!
//! Nothing here prints to stdout (the MCP server owns stdout); progress goes to stderr.

use std::{
    path::PathBuf,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use secrecy::SecretVec;
use spendguard::{Decision, Policy, SpendRequest};
use x402z_facilitator::lwd;
use x402z_protocol::{
    decode_header, encode_header, reason, PaymentPayload, PaymentRequired, PaymentRequiredResponse, SettlementResponse,
    PAYMENT_HEADER, PAYMENT_RESPONSE_HEADER,
};
use zcash_protocol::consensus::BlockHeight;

use crate::{
    ledger::{AuditEntry, LedgerFile},
    wallet,
};

/// Why a paid fetch stopped. Each maps to one `x402z-agent` exit code.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PayError {
    /// 1: network, server or facilitator failure.
    Runtime(String),
    /// 2: missing or invalid configuration.
    Config(String),
    /// 3: spendguard refused; nothing was built or broadcast.
    Refused(String),
    /// 4: no usable wallet (no seed, or not enough testnet ZEC).
    Wallet(String),
    /// 5: the server/facilitator rejected the payment.
    Rejected(String),
}

impl PayError {
    pub fn exit_code(&self) -> i32 {
        match self {
            PayError::Runtime(_) => 1,
            PayError::Config(_) => 2,
            PayError::Refused(_) => 3,
            PayError::Wallet(_) => 4,
            PayError::Rejected(_) => 5,
        }
    }

    pub fn kind(&self) -> &'static str {
        match self {
            PayError::Runtime(_) => "runtime",
            PayError::Config(_) => "config",
            PayError::Refused(_) => "spendguard_refused",
            PayError::Wallet(_) => "wallet",
            PayError::Rejected(_) => "payment_rejected",
        }
    }

    pub fn message(&self) -> &str {
        match self {
            PayError::Runtime(m) | PayError::Config(m) | PayError::Refused(m) | PayError::Wallet(m) | PayError::Rejected(m) => m,
        }
    }
}

impl std::fmt::Display for PayError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.message())
    }
}

/// Agent configuration (from `X402Z_AGENT_*`, see `.env.example`).
pub struct AgentConfig {
    /// Required to pay (spendguard state); `balance` works without it.
    pub ledger_path: Option<PathBuf>,
    pub wallet_dir: PathBuf,
    pub agent: String,
    pub policy: Policy,
    pub seed: Option<SecretVec<u8>>,
    pub birthday: Option<u32>,
    pub lwd_url: String,
    pub confirm_timeout_secs: u64,
    pub poll_secs: u64,
}

fn optional(var: &str) -> Option<String> {
    std::env::var(var).ok().map(|v| v.trim().to_string()).filter(|v| !v.is_empty())
}

fn require(var: &str) -> Result<String, PayError> {
    optional(var).ok_or_else(|| PayError::Config(format!("{var} not configured (see .env.example)")))
}

fn parse_u64(var: &str) -> Result<Option<u64>, PayError> {
    optional(var)
        .map(|s| s.parse::<u64>().map_err(|_| PayError::Config(format!("{var} must be a non-negative integer"))))
        .transpose()
}

pub fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

impl AgentConfig {
    pub fn from_env() -> Result<Self, PayError> {
        let seed = optional("X402Z_AGENT_SEED")
            .map(|p| wallet::seed_from_phrase(&p).map_err(|e| PayError::Config(format!("X402Z_AGENT_SEED invalid: {e}"))))
            .transpose()?;
        Ok(Self {
            ledger_path: optional("X402Z_AGENT_LEDGER").map(PathBuf::from),
            wallet_dir: PathBuf::from(require("X402Z_AGENT_WALLET_DIR")?),
            agent: optional("X402Z_AGENT_NAME").unwrap_or_else(|| "agent-1".into()),
            policy: Self::policy_from_env()?,
            seed,
            birthday: parse_u64("X402Z_AGENT_BIRTHDAY")?.map(|h| h as u32),
            lwd_url: optional("ZCASH_LWD_URL").unwrap_or_else(|| lwd::DEFAULT_LWD_URL.to_string()),
            confirm_timeout_secs: parse_u64("X402Z_AGENT_CONFIRM_TIMEOUT_SECS")?.unwrap_or(900),
            poll_secs: 15,
        })
    }

    pub fn policy_from_env() -> Result<Policy, PayError> {
        Ok(Policy {
            daily_cap_zat: parse_u64("X402Z_AGENT_DAILY_CAP_ZAT")?.unwrap_or(500_000),
            allowed_hosts: optional("X402Z_AGENT_ALLOWED_HOSTS")
                .map(|s| s.split(',').map(|h| h.trim().to_string()).filter(|h| !h.is_empty()).collect())
                .unwrap_or_default(),
            approval_threshold_zat: parse_u64("X402Z_AGENT_APPROVAL_ZAT")?,
        })
    }
}

/// What a paid fetch returns.
#[derive(Debug, Clone)]
pub struct Fetched {
    pub status: u16,
    pub body: String,
    /// Set when this call paid: (txid, amount_zat, memo_nonce).
    pub paid: Option<(String, u64, String)>,
    /// Decoded `X-PAYMENT-RESPONSE` of the final answer, if any.
    pub settlement: Option<SettlementResponse>,
}

/// `host[:port]` of a URL, as spendguard matches it.
pub fn host_of(url: &str) -> Result<String, PayError> {
    let u = reqwest::Url::parse(url).map_err(|e| PayError::Config(format!("invalid URL {url}: {e}")))?;
    match (u.host_str(), u.port()) {
        (Some(h), Some(p)) => Ok(format!("{h}:{p}")),
        (Some(h), None) => Ok(h.to_string()),
        _ => Err(PayError::Config(format!("URL {url} has no host"))),
    }
}

/// Parse an x402 402 body and pick the payable `zcash-shielded` option.
pub fn challenge_from_402(body: &str, now_unix: u64) -> Result<PaymentRequired, PayError> {
    let r: PaymentRequiredResponse = serde_json::from_str(body)
        .map_err(|e| PayError::Runtime(format!("402 body is not an x402 PaymentRequiredResponse: {e}")))?;
    let opt = r
        .zcash_option()
        .ok_or_else(|| PayError::Runtime("402 offers no zcash-shielded / zcash-testnet payment option".into()))?;
    let ch = opt.to_challenge().map_err(|e| PayError::Runtime(format!("402 payment option invalid: {e}")))?;
    ch.validate(now_unix).map_err(|e| PayError::Runtime(format!("402 challenge rejected: {e:?}")))?;
    Ok(ch)
}

fn settlement_header(resp: &reqwest::Response) -> Option<SettlementResponse> {
    resp.headers().get(PAYMENT_RESPONSE_HEADER).and_then(|v| v.to_str().ok()).and_then(|v| decode_header(v).ok())
}

async fn connect_lwd(url: &str) -> Result<(lwd::Client, BlockHeight), PayError> {
    let mut client = lwd::connect(url).await.map_err(|e| PayError::Runtime(e.to_string()))?;
    let info = lwd::info(&mut client).await.map_err(|e| PayError::Runtime(format!("{e} ({url})")))?;
    if info.chain_name != "test" {
        return Err(PayError::Runtime(format!("lightwalletd at {url} serves chain '{}', expected testnet", info.chain_name)));
    }
    let tip = lwd::chain_tip(&mut client).await.map_err(|e| PayError::Runtime(format!("{e} ({url})")))?;
    eprintln!("lightwalletd {url}: testnet, chain tip {tip}");
    Ok((client, tip))
}

/// Open the wallet, creating account 0 from the seed if the database has none.
pub async fn open_wallet(
    cfg: &AgentConfig,
    client: &mut lwd::Client,
    tip: BlockHeight,
) -> Result<(wallet::Db, zcash_client_sqlite::AccountUuid), PayError> {
    let mut db = wallet::open(&cfg.wallet_dir, cfg.seed.as_ref()).map_err(PayError::Wallet)?;
    let id = match wallet::account(&db).map_err(PayError::Wallet)? {
        Some(id) => id,
        None => {
            let Some(seed) = cfg.seed.as_ref() else {
                return Err(PayError::Wallet(format!(
                    "no wallet: {} has no account and X402Z_AGENT_SEED is not configured. Set X402Z_AGENT_SEED to a \
                     funded testnet wallet's 24-word BIP 39 phrase (plus X402Z_AGENT_BIRTHDAY, the height it was created \
                     at), or run `x402z-agent new-wallet` and send testnet ZEC (TAZ) to the address it prints. Not paying.",
                    cfg.wallet_dir.display()
                )));
            };
            let birthday = cfg.birthday.map(BlockHeight::from_u32).unwrap_or_else(|| {
                eprintln!("warning: X402Z_AGENT_BIRTHDAY not set; scanning from the current tip only (older funds will not be found)");
                tip
            });
            wallet::create_account(&mut db, client, seed, birthday).await.map_err(PayError::Runtime)?
        }
    };
    Ok((db, id))
}

/// Fetch `url`, paying its x402 402 with a shielded testnet payment if spendguard allows it.
/// `approve(host, amount_zat, threshold_zat)` is asked only for payments at or above the
/// approval threshold.
pub async fn paid_fetch(cfg: &AgentConfig, url: &str, approve: &dyn Fn(&str, u64, u64) -> bool) -> Result<Fetched, PayError> {
    let host = host_of(url)?;
    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(120))
        .build()
        .map_err(|e| PayError::Runtime(e.to_string()))?;

    // 1. Ask for the resource.
    let resp = http.get(url).send().await.map_err(|e| PayError::Runtime(format!("GET {url} failed: {e}")))?;
    let status = resp.status();
    if status.is_success() {
        eprintln!("{url} answered {status} without payment");
        let body = resp.text().await.unwrap_or_default();
        return Ok(Fetched { status: status.as_u16(), body, paid: None, settlement: None });
    }
    if status.as_u16() != 402 {
        let body = resp.text().await.unwrap_or_default();
        return Err(PayError::Runtime(format!("{url} answered {status}, expected 402: {body}")));
    }
    let ch = challenge_from_402(&resp.text().await.unwrap_or_default(), now())?;
    eprintln!(
        "402 challenge: pay {} zat to {}… memo {} (expires in {}s)",
        ch.amount_zat,
        &ch.pay_to[..20.min(ch.pay_to.len())],
        ch.memo_nonce,
        ch.expires_at.saturating_sub(now())
    );

    // 2. spendguard, before any wallet work, under the ledger lock (held until the broadcast
    //    is recorded so concurrent runs can't both pass the cap).
    let ledger_path = cfg
        .ledger_path
        .as_ref()
        .ok_or_else(|| PayError::Config("X402Z_AGENT_LEDGER not configured: spendguard needs it to enforce the daily cap. Not paying.".into()))?;
    let ledger_lock = LedgerFile::lock(ledger_path).map_err(PayError::Config)?;
    let mut ledger = LedgerFile::load(ledger_path).map_err(PayError::Config)?;
    let agent = cfg.agent.as_str();
    let policy = &cfg.policy;
    let t = now();
    let guard = ledger.guard(agent, policy, t);
    let req = SpendRequest { agent, host: &host, amount_zat: ch.amount_zat, now_unix: t };
    let mut audit = AuditEntry {
        ts: t,
        agent: agent.to_string(),
        host: host.clone(),
        amount_zat: ch.amount_zat,
        decision: "allowed".into(),
        reason: None,
        memo_nonce: Some(ch.memo_nonce.clone()),
        txid: None,
    };
    let spent_before = guard.spent_today(agent, t);
    let refusal = match guard.check(&req) {
        Decision::Allow => None,
        Decision::Deny(d) => Some(d.to_string()),
        Decision::NeedsApproval { threshold_zat } => {
            if approve(&host, ch.amount_zat, threshold_zat) {
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
        ledger.upsert_agent(agent, policy, spent_before);
        ledger.append(audit);
        ledger.save().map_err(PayError::Runtime)?;
        return Err(PayError::Refused(format!(
            "spendguard refused the payment: {reason}. No transaction was built or broadcast."
        )));
    }
    eprintln!("spendguard: allowed ({} of {} zat already spent today)", spent_before, policy.daily_cap_zat);

    // 3. Pay from the wallet. Every failure from here is recorded with the allowed decision.
    let record_failure = |ledger: &mut LedgerFile, why: &str| {
        let mut a = audit.clone();
        a.reason = Some(format!("allowed, but not paid: {why}"));
        ledger.upsert_agent(agent, policy, spent_before);
        ledger.append(a);
        let _ = ledger.save();
    };
    let Some(seed) = cfg.seed.as_ref() else {
        record_failure(&mut ledger, "X402Z_AGENT_SEED not configured");
        return Err(PayError::Wallet(format!(
            "X402Z_AGENT_SEED not configured: no funded testnet wallet to pay from. Set X402Z_AGENT_SEED (24-word \
             BIP 39 phrase of a wallet holding at least {} zat of testnet ZEC) and X402Z_AGENT_BIRTHDAY, or run \
             `x402z-agent new-wallet` and fund the printed address. Not paying.",
            ch.amount_zat + wallet::MIN_FEE_ZAT
        )));
    };
    let (mut client, tip) = match connect_lwd(&cfg.lwd_url).await {
        Ok(c) => c,
        Err(e) => {
            record_failure(&mut ledger, e.message());
            return Err(e);
        }
    };
    let (mut db, id) = open_wallet(cfg, &mut client, tip).await?;
    let usk = wallet::spending_key(&db, id, seed).map_err(PayError::Config)?;
    eprintln!("syncing wallet in {}...", cfg.wallet_dir.display());
    wallet::sync(&mut db, &mut client).await.map_err(PayError::Runtime)?;
    let bal = wallet::balance(&db, id).map_err(PayError::Runtime)?;
    let need = ch.amount_zat + wallet::MIN_FEE_ZAT;
    if bal.spendable_zat < need {
        let ua = wallet::ufvk(&db, id).and_then(|u| wallet::address(&u)).unwrap_or_default();
        record_failure(&mut ledger, &format!("insufficient balance ({} zat spendable)", bal.spendable_zat));
        return Err(PayError::Wallet(format!(
            "insufficient testnet balance: spendable {} zat (total {}, pending {}), need at least {} zat ({} + fee). \
             Send TAZ to {ua} and wait for 1 confirmation. Not paying.",
            bal.spendable_zat, bal.total_zat, bal.pending_zat, need, ch.amount_zat
        )));
    }
    let txid = match wallet::pay(&mut db, &mut client, id, usk, &ch.pay_to, ch.amount_zat, &ch.memo_nonce).await {
        Ok(t) => t.to_string(),
        Err(e) => {
            record_failure(&mut ledger, &e);
            return Err(PayError::Runtime(e));
        }
    };
    audit.txid = Some(txid.clone());
    audit.reason.get_or_insert_with(|| "paid".into());
    ledger.upsert_agent(agent, policy, spent_before.saturating_add(ch.amount_zat));
    ledger.append(audit);
    ledger.save().map_err(PayError::Runtime)?;
    drop(ledger_lock);
    eprintln!("paid: tx {txid} ({} zat, memo {})", ch.amount_zat, ch.memo_nonce);

    // 4. Retry with X-PAYMENT until the server settles it (it asks the facilitator, which waits
    //    for the confirmation policy).
    let x_payment = encode_header(&PaymentPayload::zcash(&txid, &ch.memo_nonce));
    let deadline = now() + cfg.confirm_timeout_secs;
    let paid = Some((txid.clone(), ch.amount_zat, ch.memo_nonce.clone()));
    loop {
        match http.get(url).header(PAYMENT_HEADER, &x_payment).send().await {
            Err(e) => eprintln!("GET {url} with X-PAYMENT failed: {e}; retrying"),
            Ok(resp) => {
                let status = resp.status();
                let settlement = settlement_header(&resp);
                let body = resp.text().await.unwrap_or_default();
                if status.is_success() {
                    eprintln!("{url} answered {status}: paid content delivered");
                    return Ok(Fetched { status: status.as_u16(), body, paid, settlement });
                }
                let why = settlement.as_ref().and_then(|s| s.error_reason.clone()).unwrap_or_default();
                let detail = serde_json::from_str::<serde_json::Value>(&body)
                    .ok()
                    .and_then(|v| v.get("error").and_then(|e| e.as_str()).map(str::to_string))
                    .unwrap_or(body);
                let retry = (status.as_u16() == 402 && why == reason::PENDING) || status.as_u16() == 502;
                if !retry {
                    return Err(PayError::Rejected(format!(
                        "server refused payment tx {txid} ({status}, {why}): {detail}"
                    )));
                }
                eprintln!("not settled yet ({status}, {why}): {detail}");
            }
        }
        if now() >= deadline {
            return Err(PayError::Runtime(format!(
                "tx {txid} not settled before timeout; it may still confirm. Retry later with the same X-PAYMENT \
                 (txid {txid}, memoNonce {})",
                ch.memo_nonce
            )));
        }
        tokio::time::sleep(Duration::from_secs(cfg.poll_secs)).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn picks_the_zcash_option_and_validates_it() {
        let body = r#"{"x402Version":1,"error":"X-PAYMENT header is required","accepts":[
            {"scheme":"exact","network":"base-sepolia","maxAmountRequired":"1","resource":"r","description":"d","payTo":"0x1","maxTimeoutSeconds":60,"asset":"0x2"},
            {"scheme":"zcash-shielded","network":"zcash-testnet","maxAmountRequired":"100000","resource":"r","description":"d","payTo":"utest1sampleaddressforschemaonly000000","maxTimeoutSeconds":900,"asset":"ZEC","extra":{"memoNonce":"x402z-1","expiresAt":2000000000}}]}"#;
        let ch = challenge_from_402(body, 1_900_000_000).unwrap();
        assert_eq!((ch.amount_zat, ch.memo_nonce.as_str()), (100_000, "x402z-1"));
        // expired by the time the agent looks at it
        assert!(matches!(challenge_from_402(body, 2_000_000_000), Err(PayError::Runtime(_))));
        // no zcash option
        let evm_only = r#"{"x402Version":1,"error":"","accepts":[{"scheme":"exact","network":"base","maxAmountRequired":"1","resource":"r","description":"d","payTo":"0x1","maxTimeoutSeconds":60,"asset":"0x2"}]}"#;
        assert!(challenge_from_402(evm_only, 0).unwrap_err().message().contains("no zcash-shielded"));
        // the legacy flat challenge is not an x402 body
        assert!(challenge_from_402(r#"{"scheme":"zcash-shielded"}"#, 0).is_err());
    }

    #[test]
    fn host_of_keeps_the_port_for_spendguard() {
        assert_eq!(host_of("http://127.0.0.1:4020/api/quote").unwrap(), "127.0.0.1:4020");
        assert_eq!(host_of("https://api.example.com/x").unwrap(), "api.example.com");
        assert!(matches!(host_of("not a url"), Err(PayError::Config(_))));
    }

    #[test]
    fn exit_codes_are_stable() {
        let codes: Vec<i32> = [
            PayError::Runtime(String::new()),
            PayError::Config(String::new()),
            PayError::Refused(String::new()),
            PayError::Wallet(String::new()),
            PayError::Rejected(String::new()),
        ]
        .iter()
        .map(PayError::exit_code)
        .collect();
        assert_eq!(codes, vec![1, 2, 3, 4, 5]);
    }
}
