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

/// A payment that was broadcast and is waiting for the server to settle it (the tx needs its
/// confirmation). Present it again with [`redeem_payment`] or by calling the paid fetch again for
/// the same URL; neither pays a second time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingPayment {
    pub url: String,
    pub txid: String,
    pub memo_nonce: String,
    pub amount_zat: u64,
    /// The server's latest word on it ("payment found ... with 0 confirmation(s); need 1").
    pub message: String,
    /// True when this call presented an earlier payment instead of making one.
    pub resumed: bool,
}

/// What a non-waiting paid fetch returns.
#[derive(Debug, Clone)]
pub enum PaidFetch {
    Done(Fetched),
    Pending(PendingPayment),
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

/// The agent's HTTP client. It never follows redirects: spendguard checks the host of the URL it
/// was given, so a redirect would let another host's 402 be paid under this host's allowlist entry
/// and budget, and would carry `X-PAYMENT` to that host too.
pub(crate) fn http_client() -> Result<reqwest::Client, PayError> {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(120))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|e| PayError::Runtime(e.to_string()))
}

/// A 3xx answer is an error naming where it pointed; nothing is followed or paid.
pub(crate) fn refuse_redirect(url: &str, resp: &reqwest::Response) -> Result<(), PayError> {
    if !resp.status().is_redirection() {
        return Ok(());
    }
    let to = resp.headers().get("location").and_then(|v| v.to_str().ok()).unwrap_or("(no Location)");
    Err(PayError::Runtime(format!(
        "{url} answered {} redirecting to {to}; the agent does not follow redirects for paid requests (spendguard \
         checked the host you gave, not the redirect target). Call the final URL directly if you trust it. Nothing was paid.",
        resp.status()
    )))
}

pub(crate) fn settlement_header(resp: &reqwest::Response) -> Option<SettlementResponse> {
    resp.headers().get(PAYMENT_RESPONSE_HEADER).and_then(|v| v.to_str().ok()).and_then(|v| decode_header(v).ok())
}

pub(crate) async fn connect_lwd(url: &str) -> Result<(lwd::Client, BlockHeight), PayError> {
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

/// A payment spendguard has allowed, holding the ledger lock until the outcome is recorded (so
/// concurrent runs can't both pass the cap).
pub struct Gate {
    _lock: crate::ledger::LedgerLock,
    pub ledger: LedgerFile,
    pub audit: AuditEntry,
    pub spent_before: u64,
    agent: String,
    policy: Policy,
}

impl Gate {
    /// Record that the allowed payment was not made (`why`), then release the lock.
    pub fn record_failure(mut self, why: &str) {
        let mut a = self.audit.clone();
        a.reason = Some(format!("allowed, but not paid: {why}"));
        self.ledger.upsert_agent(&self.agent, &self.policy, self.spent_before);
        self.ledger.append(a);
        let _ = self.ledger.save();
    }

    /// Record a broadcast payment (counts against today's cap), then release the lock.
    pub fn record_paid(mut self, txid: &str, reason: Option<&str>) -> Result<(), PayError> {
        self.audit.txid = Some(txid.to_string());
        match reason {
            Some(r) => self.audit.reason = Some(r.to_string()),
            None => {
                self.audit.reason.get_or_insert_with(|| "paid".into());
            }
        }
        let total = self.spent_before.saturating_add(self.audit.amount_zat);
        self.ledger.upsert_agent(&self.agent, &self.policy, total);
        self.ledger.append(self.audit.clone());
        self.ledger.save().map_err(PayError::Runtime)
    }
}

/// Spendguard, before any wallet work: allowlist, cap and approval threshold for paying
/// `amount_zat` to `host` (challenge `memo_nonce`). A refusal is written to the ledger and
/// returned as [`PayError::Refused`]; an allowance returns the [`Gate`] still holding the lock.
/// `approve(host, amount_zat, threshold_zat)` is asked only at or above the threshold.
pub fn spendguard_gate(
    cfg: &AgentConfig,
    host: &str,
    amount_zat: u64,
    memo_nonce: &str,
    approve: &(dyn Fn(&str, u64, u64) -> bool + Sync),
) -> Result<Gate, PayError> {
    let ledger_path = cfg
        .ledger_path
        .as_ref()
        .ok_or_else(|| PayError::Config("X402Z_AGENT_LEDGER not configured: spendguard needs it to enforce the daily cap. Not paying.".into()))?;
    let lock = LedgerFile::lock(ledger_path).map_err(PayError::Config)?;
    let mut ledger = LedgerFile::load(ledger_path).map_err(PayError::Config)?;
    let agent = cfg.agent.as_str();
    let policy = &cfg.policy;
    let t = now();
    let guard = ledger.guard(agent, policy, t);
    let req = SpendRequest { agent, host, amount_zat, now_unix: t };
    let mut audit = AuditEntry {
        ts: t,
        agent: agent.to_string(),
        host: host.to_string(),
        amount_zat,
        decision: "allowed".into(),
        reason: None,
        memo_nonce: Some(memo_nonce.to_string()),
        txid: None,
        url: None,
        settled: None,
    };
    let spent_before = guard.spent_today(agent, t);
    let refusal = match guard.check(&req) {
        Decision::Allow => None,
        Decision::Deny(d) => Some(d.to_string()),
        Decision::NeedsApproval { threshold_zat } => {
            if approve(host, amount_zat, threshold_zat) {
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
    eprintln!("spendguard: allowed {amount_zat} zat ({} of {} zat already spent today)", spent_before, policy.daily_cap_zat);
    Ok(Gate { _lock: lock, ledger, audit, spent_before, agent: agent.to_string(), policy: policy.clone() })
}

/// Record what a broadcast came to, under the [`Gate`] that allowed it.
///
/// Once transactions are created the money may move, so anything short of a definite rejection
/// counts against today's cap: an unknown outcome (transport error, timeout) is recorded with its
/// txid and charged. `seen` (lightwalletd asked afterwards) can only upgrade an unknown outcome to
/// paid; "not seen yet" doesn't prove the node never got it. A definite rejection is not charged.
pub fn record_broadcast(gate: Gate, txid: &str, outcome: wallet::Broadcast, seen: Option<bool>) -> Result<String, PayError> {
    match outcome {
        wallet::Broadcast::Sent => {
            gate.record_paid(txid, None)?;
            Ok(txid.to_string())
        }
        wallet::Broadcast::Rejected(e) => {
            gate.record_failure(&e);
            Err(PayError::Runtime(format!("{e}. Nothing was paid.")))
        }
        wallet::Broadcast::Unknown(e) if seen == Some(true) => {
            gate.record_paid(txid, Some(&format!("paid; the broadcast answer was lost ({e}) but lightwalletd has the tx")))?;
            Ok(txid.to_string())
        }
        wallet::Broadcast::Unknown(e) => {
            let lookup = match seen {
                Some(false) => "lightwalletd does not show it yet",
                _ => "lightwalletd could not be asked",
            };
            gate.record_paid(txid, Some(&format!("broadcast status unknown ({e}; {lookup}); counted against the cap")))?;
            Err(PayError::Runtime(format!(
                "broadcast of tx {txid} has an unknown outcome ({e}; {lookup}). It may still be mined, so it counts \
                 against today's cap. Check https://testnet.zecblock.com/tx/{txid} before paying again."
            )))
        }
    }
}

/// Everything before the money can move: lightwalletd, wallet, key, sync, balance, building and
/// signing. Any failure here is recorded by the caller as "allowed, but not paid".
async fn prepare_payment(
    cfg: &AgentConfig,
    seed: &SecretVec<u8>,
    pay_to: &str,
    amount_zat: u64,
    memo: &str,
) -> Result<(lwd::Client, Vec<(zcash_protocol::TxId, Vec<u8>)>), PayError> {
    let (mut client, tip) = connect_lwd(&cfg.lwd_url).await?;
    let (mut db, id) = open_wallet(cfg, &mut client, tip).await?;
    let usk = wallet::spending_key(&db, id, seed).map_err(PayError::Config)?;
    eprintln!("syncing wallet in {}...", cfg.wallet_dir.display());
    wallet::sync(&mut db, &mut client).await.map_err(PayError::Runtime)?;
    let bal = wallet::balance(&db, id).map_err(PayError::Runtime)?;
    let need = amount_zat + wallet::MIN_FEE_ZAT;
    if bal.spendable_zat < need {
        let ua = wallet::ufvk(&db, id).and_then(|u| wallet::address(&u)).unwrap_or_default();
        return Err(PayError::Wallet(format!(
            "insufficient testnet balance: spendable {} zat (total {}, pending {}), need at least {} zat ({} + fee). \
             Send TAZ to {ua} and wait for 1 confirmation. Not paying.",
            bal.spendable_zat, bal.total_zat, bal.pending_zat, need, amount_zat
        )));
    }
    let txs = wallet::create(&mut db, id, usk, pay_to, amount_zat, memo).map_err(PayError::Runtime)?;
    Ok((client, txs))
}

/// Pay `amount_zat` to `pay_to` with text memo `memo` from the agent's own wallet
/// (`X402Z_AGENT_SEED`), under an allowed [`Gate`], and record the outcome in the ledger: every
/// allowed decision ends in exactly one audit line. Returns the txid.
pub async fn pay_from_agent_wallet(cfg: &AgentConfig, gate: Gate, pay_to: &str, amount_zat: u64, memo: &str) -> Result<String, PayError> {
    let Some(seed) = cfg.seed.as_ref() else {
        gate.record_failure("X402Z_AGENT_SEED not configured");
        return Err(PayError::Wallet(format!(
            "X402Z_AGENT_SEED not configured: no funded testnet wallet to pay from. Set X402Z_AGENT_SEED (24-word \
             BIP 39 phrase of a wallet holding at least {} zat of testnet ZEC) and X402Z_AGENT_BIRTHDAY, or run \
             `x402z-agent new-wallet` and fund the printed address. Not paying.",
            amount_zat + wallet::MIN_FEE_ZAT
        )));
    };
    let (mut client, txs) = match prepare_payment(cfg, seed, pay_to, amount_zat, memo).await {
        Ok(x) => x,
        Err(e) => {
            gate.record_failure(e.message());
            return Err(e);
        }
    };
    let Some((last, _)) = txs.last() else {
        gate.record_failure("the proposal produced no transaction");
        return Err(PayError::Runtime("the payment proposal produced no transaction".into()));
    };
    let (last, txid) = (*last, last.to_string());
    let outcome = wallet::broadcast(&mut client, &txs).await;
    let seen = match &outcome {
        wallet::Broadcast::Unknown(_) => wallet::seen_by_lightwalletd(&mut client, &last).await,
        _ => None,
    };
    let txid = record_broadcast(gate, &txid, outcome, seen)?;
    eprintln!("paid: tx {txid} ({amount_zat} zat, memo {memo})");
    Ok(txid)
}

/// Fetch `url`, paying its x402 402 with a shielded testnet payment if spendguard allows it, and
/// wait (up to `X402Z_AGENT_CONFIRM_TIMEOUT_SECS`) until the server settles it. If an earlier
/// payment for `url` was broadcast and never settled, that one is presented instead of paying
/// again. `approve(host, amount_zat, threshold_zat)` is asked only for payments at or above the
/// approval threshold.
pub async fn paid_fetch(cfg: &AgentConfig, url: &str, approve: &(dyn Fn(&str, u64, u64) -> bool + Sync)) -> Result<Fetched, PayError> {
    match fetch_or_pay(cfg, url, approve, true).await? {
        PaidFetch::Done(f) => Ok(f),
        PaidFetch::Pending(p) => Err(PayError::Runtime(format!(
            "tx {} not settled before timeout; it may still confirm. Retry later: the next paid fetch of {} presents \
             it again (memoNonce {}) and does not pay twice",
            p.txid, p.url, p.memo_nonce
        ))),
    }
}

/// [`paid_fetch`] that returns as soon as the payment is broadcast (or an earlier one is
/// presented and still pending), for callers with a request timeout such as MCP clients.
pub async fn paid_fetch_nowait(cfg: &AgentConfig, url: &str, approve: &(dyn Fn(&str, u64, u64) -> bool + Sync)) -> Result<PaidFetch, PayError> {
    fetch_or_pay(cfg, url, approve, false).await
}

/// Present (memoNonce, txid) for `url` once with `X-PAYMENT`. Never pays. A 200 or a final
/// refusal is written to the ledger entry of that txid, if there is one.
pub async fn redeem_payment(cfg: &AgentConfig, url: &str, txid: &str, memo_nonce: &str) -> Result<PaidFetch, PayError> {
    let txid = txid.trim().to_ascii_lowercase();
    if !x402z_protocol::valid_txid(&txid) || !x402z_protocol::valid_nonce(memo_nonce.trim()) {
        return Err(PayError::Config("txid must be 64 hex characters and memoNonce 1-64 of [A-Za-z0-9_-]".into()));
    }
    host_of(url)?;
    let amount = ledger_entry(cfg, &txid).map(|e| e.amount_zat).unwrap_or(0);
    let http = http_client()?;
    settle(cfg, &http, url, &txid, memo_nonce.trim(), amount, false, true).await
}

fn ledger_entry(cfg: &AgentConfig, txid: &str) -> Option<AuditEntry> {
    let l = LedgerFile::load(cfg.ledger_path.as_ref()?).ok()?;
    l.data.audit.iter().rev().find(|e| e.txid.as_deref() == Some(txid)).cloned()
}

/// Write the server's final answer for `txid` into the ledger (best effort: the answer itself
/// is what the caller gets either way).
fn mark_settled(cfg: &AgentConfig, txid: &str, settled: bool) {
    let Some(path) = cfg.ledger_path.as_ref() else { return };
    let result = LedgerFile::lock(path).and_then(|_lock| {
        let mut l = LedgerFile::load(path)?;
        if l.mark_settled(txid, settled) {
            l.save()?;
        }
        Ok(())
    });
    if let Err(e) = result {
        eprintln!("warning: could not record the outcome of tx {txid} in the ledger: {e}");
    }
}

async fn fetch_or_pay(cfg: &AgentConfig, url: &str, approve: &(dyn Fn(&str, u64, u64) -> bool + Sync), wait: bool) -> Result<PaidFetch, PayError> {
    let host = host_of(url)?;
    let http = http_client()?;

    // 0. A payment for this URL that was broadcast and never settled is presented again; paying
    //    a second time would only be a second payment for the same thing.
    if let Some(path) = cfg.ledger_path.as_ref() {
        let ledger = LedgerFile::load(path).map_err(PayError::Config)?;
        if let Some(e) = ledger.unresolved_payment(&cfg.agent, url, now()) {
            let (txid, nonce) = (e.txid.clone().unwrap_or_default(), e.memo_nonce.clone().unwrap_or_default());
            eprintln!("resuming tx {txid} (memo {nonce}) for {url}: broadcast earlier and not settled yet; not paying again");
            return settle(cfg, &http, url, &txid, &nonce, e.amount_zat, wait, true).await;
        }
    }

    // 1. Ask for the resource.
    let resp = http.get(url).send().await.map_err(|e| PayError::Runtime(format!("GET {url} failed: {e}")))?;
    refuse_redirect(url, &resp)?;
    let status = resp.status();
    if status.is_success() {
        eprintln!("{url} answered {status} without payment");
        let body = resp.text().await.unwrap_or_default();
        return Ok(PaidFetch::Done(Fetched { status: status.as_u16(), body, paid: None, settlement: None }));
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

    // 2. spendguard, before any wallet work. 3. Pay from the wallet (the ledger entry names the
    //    URL, so a retry resumes this payment).
    let mut gate = spendguard_gate(cfg, &host, ch.amount_zat, &ch.memo_nonce, approve)?;
    gate.audit.url = Some(url.to_string());
    let txid = pay_from_agent_wallet(cfg, gate, &ch.pay_to, ch.amount_zat, &ch.memo_nonce).await?;

    // 4. Present it with X-PAYMENT: until settled (CLI), or once (MCP).
    settle(cfg, &http, url, &txid, &ch.memo_nonce, ch.amount_zat, wait, false).await
}

/// Present `X-PAYMENT` for (txid, memo_nonce) until the server settles it, or once if `!wait`.
/// Pending answers (402 settlement_pending, 502, 429, a network error) are retried while
/// waiting; any other answer is final and is written to the ledger.
#[allow(clippy::too_many_arguments)]
async fn settle(
    cfg: &AgentConfig,
    http: &reqwest::Client,
    url: &str,
    txid: &str,
    memo_nonce: &str,
    amount_zat: u64,
    wait: bool,
    resumed: bool,
) -> Result<PaidFetch, PayError> {
    let x_payment = encode_header(&PaymentPayload::zcash(txid, memo_nonce));
    let deadline = now() + cfg.confirm_timeout_secs;
    let paid = Some((txid.to_string(), amount_zat, memo_nonce.to_string()));
    let pending = |message: String| PendingPayment {
        url: url.to_string(),
        txid: txid.to_string(),
        memo_nonce: memo_nonce.to_string(),
        amount_zat,
        message,
        resumed,
    };
    loop {
        let message = match http.get(url).header(PAYMENT_HEADER, &x_payment).send().await {
            Err(e) => format!("GET {url} with X-PAYMENT failed: {e}"),
            Ok(resp) => {
                let status = resp.status();
                let settlement = settlement_header(&resp);
                let body = resp.text().await.unwrap_or_default();
                if status.is_success() {
                    eprintln!("{url} answered {status}: paid content delivered");
                    mark_settled(cfg, txid, true);
                    return Ok(PaidFetch::Done(Fetched { status: status.as_u16(), body, paid, settlement }));
                }
                let why = settlement.as_ref().and_then(|s| s.error_reason.clone()).unwrap_or_default();
                let detail = serde_json::from_str::<serde_json::Value>(&body)
                    .ok()
                    .and_then(|v| v.get("error").and_then(|e| e.as_str()).map(str::to_string))
                    .unwrap_or(body);
                let retry = (status.as_u16() == 402 && why == reason::PENDING) || matches!(status.as_u16(), 429 | 502 | 503);
                if !retry {
                    mark_settled(cfg, txid, false);
                    return Err(PayError::Rejected(format!("server refused payment tx {txid} ({status}, {why}): {detail}")));
                }
                format!("not settled yet ({status}, {why}): {detail}")
            }
        };
        eprintln!("{message}");
        if !wait {
            return Ok(PaidFetch::Pending(pending(message)));
        }
        if now() >= deadline {
            return Ok(PaidFetch::Pending(pending(message)));
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

    fn gate_cfg(name: &str) -> AgentConfig {
        let ledger = std::env::temp_dir().join(format!("x402z-pay-{}-{name}.json", std::process::id()));
        let _ = std::fs::remove_file(&ledger);
        AgentConfig {
            ledger_path: Some(ledger),
            wallet_dir: std::env::temp_dir(),
            agent: "gate-test".into(),
            policy: Policy { daily_cap_zat: 1_000_000, allowed_hosts: vec!["127.0.0.1:4020".into()], approval_threshold_zat: None },
            seed: None,
            birthday: None,
            lwd_url: String::new(),
            confirm_timeout_secs: 60,
            poll_secs: 0,
        }
    }

    fn spent_and_last(cfg: &AgentConfig) -> (u64, AuditEntry) {
        let l = LedgerFile::load(cfg.ledger_path.as_ref().unwrap()).unwrap();
        (l.guard(&cfg.agent, &cfg.policy, now()).spent_today(&cfg.agent, now()), l.data.audit.last().unwrap().clone())
    }

    #[test]
    fn an_unknown_broadcast_outcome_is_charged_and_audited_with_its_txid() {
        let cfg = gate_cfg("unknown");
        let tx = "ab".repeat(32);
        let gate = spendguard_gate(&cfg, "127.0.0.1:4020", 300_000, "x402z-n1", &|_, _, _| false).unwrap();
        let e = record_broadcast(gate, &tx, wallet::Broadcast::Unknown("SendTransaction RPC timed out".into()), Some(false)).unwrap_err();
        assert!(matches!(&e, PayError::Runtime(m) if m.contains(&tx) && m.contains("unknown outcome")), "{e:?}");
        let (spent, last) = spent_and_last(&cfg);
        assert_eq!(spent, 300_000, "the cap is charged: the money may have moved");
        assert_eq!(last.txid.as_deref(), Some(tx.as_str()));
        assert!(last.reason.as_deref().unwrap().contains("broadcast status unknown"), "{last:?}");

        // a retry within the cap sees the earlier, unknown payment as spent
        let gate = spendguard_gate(&cfg, "127.0.0.1:4020", 800_000, "x402z-n2", &|_, _, _| false);
        assert!(matches!(gate, Err(PayError::Refused(m)) if m.contains("daily cap exceeded")));
        let _ = std::fs::remove_file(cfg.ledger_path.as_ref().unwrap());
    }

    #[test]
    fn a_lost_answer_for_a_tx_lightwalletd_has_is_paid_and_a_rejection_is_not_charged() {
        let cfg = gate_cfg("seen");
        let tx = "cd".repeat(32);
        let gate = spendguard_gate(&cfg, "127.0.0.1:4020", 100_000, "x402z-n1", &|_, _, _| false).unwrap();
        assert_eq!(record_broadcast(gate, &tx, wallet::Broadcast::Unknown("timeout".into()), Some(true)).unwrap(), tx);
        let (spent, last) = spent_and_last(&cfg);
        assert_eq!((spent, last.txid.as_deref()), (100_000, Some(tx.as_str())));

        let gate = spendguard_gate(&cfg, "127.0.0.1:4020", 100_000, "x402z-n2", &|_, _, _| false).unwrap();
        let e = record_broadcast(gate, "ef", wallet::Broadcast::Rejected("lightwalletd rejected tx ef: bad-txns (-26)".into()), None).unwrap_err();
        assert!(matches!(&e, PayError::Runtime(m) if m.contains("Nothing was paid")), "{e:?}");
        let (spent, last) = spent_and_last(&cfg);
        assert_eq!(spent, 100_000, "a definite rejection is not charged");
        assert_eq!(last.txid, None);
        assert!(last.reason.as_deref().unwrap().starts_with("allowed, but not paid: lightwalletd rejected"), "{last:?}");
        let _ = std::fs::remove_file(cfg.ledger_path.as_ref().unwrap());
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
