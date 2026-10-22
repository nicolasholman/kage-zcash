//! Kage Tab, agent side: one shielded payment buys N blind-signed tokens, each spent on one
//! later request with `X-PAYMENT` scheme `zcash-shielded-tab` (see the server's `tab.rs` and the
//! README). Shared by the `x402z-agent` CLI (`tab-buy`, `tab-redeem`, `tab-fetch`, `tab-status`)
//! and the MCP tool `tab_fetch`.
//!
//! Buying goes through spendguard like any payment: the whole `n * unitPriceZat` is checked
//! against the cap, allowlist and approval threshold before anything is paid, and the broadcast
//! txid is written to the ledger. Two ways to pay:
//! - [`buy`]: from the agent's own wallet (`X402Z_AGENT_SEED`), end to end;
//! - [`start_external`] + [`finish_external`]: the agent prints a ZIP 321 URI for `n * unit` with
//!   the challenge nonce as memo, any wallet pays it, and the txid is handed back.
//!
//! Tokens are bearer secrets and live in a local JSON file (`X402Z_AGENT_TAB_FILE`, default
//! `<X402Z_AGENT_LEDGER>.tab.json`), guarded by an exclusive lock on `<file>.lock`. A token is
//! removed from the file *before* it is sent, so two runs can't both send it; it is put back only
//! if the request never reached the server. Nothing here prints to stdout.

use std::{
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use blind_rsa_signatures::{BlindSignature, BlindingResult, DefaultRng, PublicKey, Randomized, Sha384, PSS};
use serde::{Deserialize, Serialize};
use x402z_protocol::{
    b64, b64_any, encode_header, zip321_uri, PaymentPayload, PaymentRequired, PaymentRequiredResponse, SettlementResponse,
    TabIssueRequest, TabIssueResponse, TabOffer, TabPayload, PAYMENT_HEADER, TAB_SCHEME, TAB_SUITE,
};

use crate::pay::{self, now, AgentConfig, PayError};

type Pk = PublicKey<Sha384, PSS, Randomized>;

/// Default tab size the MCP tool buys when it has no token (`X402Z_AGENT_TAB_SIZE`).
pub const DEFAULT_TAB_SIZE: u32 = 5;

/// One unblinded token, ready to spend at `origin`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StoredToken {
    pub origin: String,
    pub key_id: String,
    pub epoch_ends_at: u64,
    pub msg: String,
    pub randomizer: String,
    pub sig: String,
}

/// A tab the agent has authorised: waiting for an external wallet to pay it (`txid` unset), or
/// paid and waiting for the server to issue its tokens (`txid` set; never paid again).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PendingPurchase {
    pub url: String,
    pub n: u32,
    pub amount_zat: u64,
    pub pay_to: String,
    pub memo_nonce: String,
    pub expires_at: u64,
    pub payment_uri: String,
    pub offer: TabOffer,
    pub created_at: u64,
    /// The payment, once broadcast by the agent's own wallet.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub txid: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TabState {
    #[serde(default)]
    pub tokens: Vec<StoredToken>,
    #[serde(default)]
    pub pending: Vec<PendingPurchase>,
}

/// The token file, loaded under its lock (released on drop).
pub struct TabFile {
    path: PathBuf,
    _lock: std::fs::File,
    pub state: TabState,
}

impl TabFile {
    pub fn open(path: &Path) -> Result<Self, PayError> {
        let lock_path = path.with_extension("json.lock");
        let lock = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(&lock_path)
            .map_err(|e| PayError::Config(format!("cannot open tab lock {}: {e}", lock_path.display())))?;
        lock.lock().map_err(|e| PayError::Config(format!("cannot lock {}: {e}", lock_path.display())))?;
        let state = match std::fs::read(path) {
            Ok(raw) => serde_json::from_slice(&raw)
                .map_err(|e| PayError::Config(format!("tab file {} is not valid JSON: {e}", path.display())))?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => TabState::default(),
            Err(e) => return Err(PayError::Config(format!("tab file {} unreadable: {e}", path.display()))),
        };
        Ok(Self { path: path.to_path_buf(), _lock: lock, state })
    }

    /// Atomic replace, so a crash never leaves half a token file.
    pub fn save(&self) -> Result<(), PayError> {
        let tmp = self.path.with_extension("json.tmp");
        let raw = serde_json::to_vec_pretty(&self.state).map_err(|e| PayError::Runtime(e.to_string()))?;
        std::fs::write(&tmp, raw).map_err(|e| PayError::Runtime(format!("cannot write {}: {e}", tmp.display())))?;
        std::fs::rename(&tmp, &self.path).map_err(|e| PayError::Runtime(format!("cannot replace {}: {e}", self.path.display())))
    }

    /// Spendable tokens for `origin` (epoch not over).
    pub fn count(&self, origin: &str, now_unix: u64) -> usize {
        self.state.tokens.iter().filter(|t| t.origin == origin && t.epoch_ends_at > now_unix).count()
    }

    /// Remove and return one spendable token for `origin`; tokens whose epoch is over are dropped.
    pub fn take(&mut self, origin: &str, now_unix: u64) -> Option<StoredToken> {
        self.state.tokens.retain(|t| t.epoch_ends_at > now_unix);
        let i = self.state.tokens.iter().position(|t| t.origin == origin)?;
        Some(self.state.tokens.remove(i))
    }
}

/// `X402Z_AGENT_TAB_FILE`, else `<X402Z_AGENT_LEDGER>.tab.json`.
pub fn tab_path(cfg: &AgentConfig) -> Result<PathBuf, PayError> {
    if let Some(p) = std::env::var("X402Z_AGENT_TAB_FILE").ok().map(|v| v.trim().to_string()).filter(|v| !v.is_empty()) {
        return Ok(PathBuf::from(p));
    }
    cfg.ledger_path
        .as_ref()
        .map(|l| l.with_extension("tab.json"))
        .ok_or_else(|| PayError::Config("X402Z_AGENT_TAB_FILE or X402Z_AGENT_LEDGER must be set to keep tab tokens".into()))
}

/// `scheme://host[:port]` of a URL: tokens are kept per origin.
pub fn origin_of(url: &str) -> Result<String, PayError> {
    let u = reqwest::Url::parse(url).map_err(|e| PayError::Config(format!("invalid URL {url}: {e}")))?;
    Ok(u.origin().ascii_serialization())
}

/// The payable Zcash challenge of a 402 body and its `extra.tab` offer.
pub fn offer_from_402(body: &str, now_unix: u64) -> Result<(PaymentRequired, TabOffer), PayError> {
    let ch = pay::challenge_from_402(body, now_unix)?;
    let r: PaymentRequiredResponse = serde_json::from_str(body).map_err(|e| PayError::Runtime(format!("402 body: {e}")))?;
    let extra = r
        .zcash_option()
        .and_then(|o| o.zcash_extra().ok())
        .ok_or_else(|| PayError::Runtime("402 has no zcash-shielded extra".into()))?;
    let tab = extra.tab.ok_or_else(|| PayError::Runtime("this server does not offer a Kage Tab (no extra.tab in its 402)".into()))?;
    let offer: TabOffer = serde_json::from_value(tab).map_err(|e| PayError::Runtime(format!("extra.tab is not a tab offer: {e}")))?;
    if offer.scheme != TAB_SCHEME || offer.suite != TAB_SUITE {
        return Err(PayError::Runtime(format!("unsupported tab offer {} / {}", offer.scheme, offer.suite)));
    }
    if offer.epoch_ends_at <= now_unix {
        return Err(PayError::Runtime("the tab offer's key epoch is already over".into()));
    }
    Ok((ch, offer))
}

fn check_n(offer: &TabOffer, n: u32) -> Result<u64, PayError> {
    if n == 0 || n > offer.max_tokens {
        return Err(PayError::Config(format!("a tab holds 1 to {} tokens on this server; asked for {n}", offer.max_tokens)));
    }
    offer
        .unit_price_zat
        .checked_mul(n as u64)
        .ok_or_else(|| PayError::Config("tab price overflows".into()))
}

async fn fetch_offer(http: &reqwest::Client, url: &str) -> Result<(PaymentRequired, TabOffer), PayError> {
    let resp = http.get(url).send().await.map_err(|e| PayError::Runtime(format!("GET {url} failed: {e}")))?;
    pay::refuse_redirect(url, &resp)?;
    let status = resp.status();
    let body = resp.text().await.unwrap_or_default();
    if status.as_u16() != 402 {
        return Err(PayError::Runtime(format!("{url} answered {status}, expected a 402 with a tab offer: {body}")));
    }
    offer_from_402(&body, now())
}

/// Where to post the blinded messages: `issuePath` resolved against the resource, which must stay
/// on the resource's own origin (the paid challenge and txid go there).
fn issue_url(url: &str, offer: &TabOffer) -> Result<String, PayError> {
    let base = reqwest::Url::parse(url).map_err(|e| PayError::Config(format!("invalid URL {url}: {e}")))?;
    let issue = base.join(&offer.issue_path).map_err(|e| PayError::Runtime(format!("bad issuePath {}: {e}", offer.issue_path)))?;
    if issue.origin() != base.origin() {
        return Err(PayError::Runtime(format!(
            "tab offer's issuePath {} leaves {}; refusing to send the payment to another origin",
            offer.issue_path,
            base.origin().ascii_serialization()
        )));
    }
    Ok(issue.to_string())
}

/// What a tab purchase came to.
#[derive(Debug, Clone)]
pub enum BuyOutcome {
    Bought(Bought),
    /// Paid, and the server will issue once the payment confirms. Calling [`buy`] again for the
    /// same origin collects it; it never pays twice.
    Pending { purchase: PendingPurchase, message: String },
}

/// What a finished purchase produced.
#[derive(Debug, Clone)]
pub struct Bought {
    pub txid: String,
    pub memo_nonce: String,
    pub n: u32,
    pub amount_zat: u64,
    pub key_id: String,
    /// Seconds from the issue request to the blind signatures (includes waiting for the block).
    pub issue_wait_secs: u64,
}

/// Blind `n` fresh messages, have the server sign them for the paid challenge (retrying while the
/// payment confirms, or asking once if `!wait`), unblind and verify each signature, and store the
/// tokens. `Ok(Err(message))` means "not issued yet" (only when `!wait`).
#[allow(clippy::too_many_arguments)]
async fn issue_and_store(
    cfg: &AgentConfig,
    http: &reqwest::Client,
    url: &str,
    offer: &TabOffer,
    n: u32,
    memo_nonce: &str,
    txid: &str,
    wait: bool,
) -> Result<Result<u64, String>, PayError> {
    let pk = Pk::from_spki(&b64_any(&offer.public_key).ok_or_else(|| PayError::Runtime("tab publicKey is not base64".into()))?)
        .map_err(|e| PayError::Runtime(format!("tab publicKey is not an RSA SPKI: {e:?}")))?;
    let mut blinded: Vec<(Vec<u8>, BlindingResult)> = Vec::with_capacity(n as usize);
    for _ in 0..n {
        let mut msg = vec![0u8; 32];
        rand::fill(&mut msg[..]);
        let br = pk.blind(&mut DefaultRng, &msg).map_err(|e| PayError::Runtime(format!("blinding failed: {e:?}")))?;
        blinded.push((msg, br));
    }
    let req = TabIssueRequest {
        memo_nonce: memo_nonce.to_string(),
        txid: txid.to_string(),
        key_id: offer.key_id.clone(),
        blinded: blinded.iter().map(|(_, br)| b64(&br.blind_message)).collect(),
    };
    let issue = issue_url(url, offer)?;
    let started = Instant::now();
    let deadline = now() + cfg.confirm_timeout_secs;
    let answer: TabIssueResponse = loop {
        let message = match http.post(&issue).json(&req).send().await {
            Err(e) => format!("POST {issue} failed: {e}"),
            Ok(resp) => {
                let status = resp.status();
                let body = resp.text().await.unwrap_or_default();
                match status.as_u16() {
                    200 => {
                        break serde_json::from_str(&body)
                            .map_err(|e| PayError::Runtime(format!("tab issue answer is not {{keyId, blindSignatures}}: {e}")))?
                    }
                    202 | 429 | 502 | 503 => format!("tab issue: not yet ({status}): {body}"),
                    _ => return Err(PayError::Rejected(format!("tab issue for tx {txid} refused ({status}): {body}"))),
                }
            }
        };
        eprintln!("{message}");
        if !wait {
            return Ok(Err(message));
        }
        if now() >= deadline {
            return Err(PayError::Runtime(format!(
                "tx {txid} not confirmed for the tab before timeout; it may still confirm. Retry with \
                 `x402z-agent tab-redeem {txid}` (memoNonce {memo_nonce})"
            )));
        }
        tokio::time::sleep(Duration::from_secs(cfg.poll_secs)).await;
    };
    if answer.blind_signatures.len() != blinded.len() || answer.key_id != offer.key_id {
        return Err(PayError::Runtime(format!(
            "tab issue returned {} signatures under key {} for {} blinded messages under key {}",
            answer.blind_signatures.len(),
            answer.key_id,
            blinded.len(),
            offer.key_id
        )));
    }
    let origin = origin_of(url)?;
    let mut tokens = Vec::with_capacity(blinded.len());
    for ((msg, br), s) in blinded.iter().zip(&answer.blind_signatures) {
        let bs = BlindSignature(b64_any(s).ok_or_else(|| PayError::Runtime("blind signature is not base64".into()))?);
        // finalize also verifies the unblinded signature against the offered public key
        let sig = pk.finalize(&bs, br, msg).map_err(|e| PayError::Runtime(format!("blind signature does not verify: {e:?}")))?;
        let r = br.msg_randomizer.ok_or_else(|| PayError::Runtime("randomized suite without a message randomizer".into()))?;
        tokens.push(StoredToken {
            origin: origin.clone(),
            key_id: offer.key_id.clone(),
            epoch_ends_at: offer.epoch_ends_at,
            msg: b64(msg),
            randomizer: b64(&r.0),
            sig: b64(&sig),
        });
    }
    let mut f = TabFile::open(&tab_path(cfg)?)?;
    f.state.tokens.extend(tokens);
    f.state.pending.retain(|p| p.memo_nonce != memo_nonce);
    f.save()?;
    Ok(Ok(started.elapsed().as_secs()))
}

/// Buy a tab of `n` tokens for `url`'s server, paying from the agent's own wallet, and wait for
/// the tokens (CLI `tab-buy`).
pub async fn buy(cfg: &AgentConfig, url: &str, n: u32, approve: &(dyn Fn(&str, u64, u64) -> bool + Sync)) -> Result<Bought, PayError> {
    match buy_or_resume(cfg, url, n, approve, true).await? {
        BuyOutcome::Bought(b) => Ok(b),
        BuyOutcome::Pending { purchase, message } => Err(PayError::Runtime(format!(
            "tab paid by tx {} but not issued before timeout ({message}); `x402z-agent tab-buy` for the same server collects it \
             without paying again",
            purchase.txid.unwrap_or_default()
        ))),
    }
}

/// Collect a tab this agent already paid for at `url`'s origin, or else buy one of `n` tokens.
/// With `wait` it waits for the tokens; without, it asks the server once and returns
/// [`BuyOutcome::Pending`] while the payment confirms (MCP). It never pays while a paid tab for
/// the same origin is still waiting to be issued.
pub async fn buy_or_resume(cfg: &AgentConfig, url: &str, n: u32, approve: &(dyn Fn(&str, u64, u64) -> bool + Sync), wait: bool) -> Result<BuyOutcome, PayError> {
    let host = pay::host_of(url)?;
    let origin = origin_of(url)?;
    let tab_file = tab_path(cfg)?;
    let http = pay::http_client()?;
    let paid_earlier = TabFile::open(&tab_file)?
        .state
        .pending
        .iter()
        .filter(|p| p.txid.is_some() && origin_of(&p.url).ok().as_deref() == Some(origin.as_str()))
        .max_by_key(|p| p.created_at)
        .cloned();
    if let Some(p) = paid_earlier {
        eprintln!("tab already paid by tx {} for {origin}; collecting it instead of paying again", p.txid.as_deref().unwrap_or_default());
        return collect(cfg, &http, p, wait).await;
    }
    let (ch, offer) = fetch_offer(&http, url).await?;
    let amount = check_n(&offer, n)?;
    eprintln!("tab offer: key {} at {} zat a token; buying {n} for {amount} zat (memo {})", offer.key_id, offer.unit_price_zat, ch.memo_nonce);
    drop(TabFile::open(&tab_file)?); // fail on an unusable token file before paying
    let gate = pay::spendguard_gate(cfg, &host, amount, &ch.memo_nonce, approve)?;
    let txid = pay::pay_from_agent_wallet(cfg, gate, &ch.pay_to, amount, &ch.memo_nonce).await?;
    // Remember the paid purchase before asking for the tokens, so a timeout or a crash can't
    // lead to paying for it again.
    let p = PendingPurchase {
        url: url.to_string(),
        n,
        amount_zat: amount,
        payment_uri: zip321_uri(&ch.pay_to, amount, &ch.memo_nonce),
        pay_to: ch.pay_to,
        memo_nonce: ch.memo_nonce,
        expires_at: ch.expires_at,
        offer,
        created_at: now(),
        txid: Some(txid),
    };
    let mut f = TabFile::open(&tab_file)?;
    f.state.pending.push(p.clone());
    f.save()?;
    drop(f);
    collect(cfg, &http, p, wait).await
}

/// Ask the server for the tokens of a paid purchase.
async fn collect(cfg: &AgentConfig, http: &reqwest::Client, p: PendingPurchase, wait: bool) -> Result<BuyOutcome, PayError> {
    let txid = p.txid.clone().unwrap_or_default();
    match issue_and_store(cfg, http, &p.url, &p.offer, p.n, &p.memo_nonce, &txid, wait).await? {
        Ok(issue_wait_secs) => Ok(BuyOutcome::Bought(Bought {
            txid,
            memo_nonce: p.memo_nonce,
            n: p.n,
            amount_zat: p.amount_zat,
            key_id: p.offer.key_id,
            issue_wait_secs,
        })),
        Err(message) => Ok(BuyOutcome::Pending { purchase: p, message }),
    }
}

/// Authorise a tab of `n` tokens through spendguard and return the ZIP 321 request for any
/// wallet to pay. Nothing counts against the cap until [`finish_external`] records the txid.
pub async fn start_external(cfg: &AgentConfig, url: &str, n: u32, approve: &(dyn Fn(&str, u64, u64) -> bool + Sync)) -> Result<PendingPurchase, PayError> {
    let host = pay::host_of(url)?;
    let http = pay::http_client()?;
    let (ch, offer) = fetch_offer(&http, url).await?;
    let amount = check_n(&offer, n)?;
    let mut f = TabFile::open(&tab_path(cfg)?)?;
    let gate = pay::spendguard_gate(cfg, &host, amount, &ch.memo_nonce, approve)?;
    gate.record_failure("awaiting payment from an external wallet (ZIP 321)");
    let p = PendingPurchase {
        url: url.to_string(),
        n,
        amount_zat: amount,
        payment_uri: zip321_uri(&ch.pay_to, amount, &ch.memo_nonce),
        pay_to: ch.pay_to,
        memo_nonce: ch.memo_nonce,
        expires_at: ch.expires_at,
        offer,
        created_at: now(),
        txid: None,
    };
    f.state.pending.push(p.clone());
    f.save()?;
    Ok(p)
}

/// Record the external wallet's `txid` for a pending tab (the one with `memo_nonce`, or the
/// newest), then fetch, unblind and store its tokens.
pub async fn finish_external(cfg: &AgentConfig, txid: &str, memo_nonce: Option<&str>) -> Result<Bought, PayError> {
    let txid = txid.trim().to_ascii_lowercase();
    if txid.len() != 64 || !txid.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(PayError::Config("txid must be 64 hex characters".into()));
    }
    let p = {
        let f = TabFile::open(&tab_path(cfg)?)?;
        let found = match memo_nonce {
            Some(m) => f.state.pending.iter().find(|p| p.memo_nonce == m),
            None => f.state.pending.iter().max_by_key(|p| p.created_at),
        };
        found.cloned().ok_or_else(|| PayError::Config("no pending tab purchase; start one with `x402z-agent tab-buy <n> --external`".into()))?
    };
    // The payment happened: it counts against today's cap from here on.
    let ledger_path = cfg.ledger_path.as_ref().ok_or_else(|| PayError::Config("X402Z_AGENT_LEDGER not configured".into()))?;
    {
        let _lock = crate::ledger::LedgerFile::lock(ledger_path).map_err(PayError::Config)?;
        let mut ledger = crate::ledger::LedgerFile::load(ledger_path).map_err(PayError::Config)?;
        let host = pay::host_of(&p.url)?;
        let t = now();
        let already = ledger.data.audit.iter().any(|e| e.txid.as_deref() == Some(txid.as_str()));
        if !already {
            ledger.append(crate::ledger::AuditEntry {
                ts: t,
                agent: cfg.agent.clone(),
                host,
                amount_zat: p.amount_zat,
                decision: "allowed".into(),
                reason: Some(format!("tab of {} tokens, paid by an external wallet", p.n)),
                memo_nonce: Some(p.memo_nonce.clone()),
                txid: Some(txid.clone()),
                url: None,
                settled: None,
            });
            let spent = ledger.guard(&cfg.agent, &cfg.policy, t).spent_today(&cfg.agent, t);
            ledger.upsert_agent(&cfg.agent, &cfg.policy, spent);
            ledger.save().map_err(PayError::Runtime)?;
        }
    }
    let http = pay::http_client()?;
    let wait = issue_and_store(cfg, &http, &p.url, &p.offer, p.n, &p.memo_nonce, &txid, true)
        .await?
        .map_err(|m| PayError::Runtime(format!("tab for tx {txid} not issued: {m}")))?;
    Ok(Bought { txid, memo_nonce: p.memo_nonce, n: p.n, amount_zat: p.amount_zat, key_id: p.offer.key_id, issue_wait_secs: wait })
}

/// One request paid with a tab token.
#[derive(Debug, Clone)]
pub struct TabFetched {
    pub status: u16,
    pub body: String,
    pub settlement: Option<SettlementResponse>,
    pub remaining: usize,
    pub elapsed_ms: u128,
}

/// GET `url` spending one stored token. `Ok(None)` when there is no token for its origin.
pub async fn fetch(cfg: &AgentConfig, url: &str) -> Result<Option<TabFetched>, PayError> {
    let origin = origin_of(url)?;
    let path = tab_path(cfg)?;
    let token = {
        let mut f = TabFile::open(&path)?;
        let Some(t) = f.take(&origin, now()) else { return Ok(None) };
        f.save()?; // gone from the file before it is sent
        t
    };
    let header = encode_header(&PaymentPayload::tab(&TabPayload {
        key_id: token.key_id.clone(),
        msg: token.msg.clone(),
        randomizer: token.randomizer.clone(),
        sig: token.sig.clone(),
    }));
    let http = pay::http_client()?;
    let started = Instant::now();
    let resp = match http.get(url).header(PAYMENT_HEADER, header).send().await {
        Ok(r) => r,
        Err(e) => {
            if e.is_connect() {
                // never reached the server: the token is still unspent
                let mut f = TabFile::open(&path)?;
                f.state.tokens.push(token);
                f.save()?;
            }
            return Err(PayError::Runtime(format!("GET {url} with a tab token failed: {e}")));
        }
    };
    let status = resp.status();
    let settlement = pay::settlement_header(&resp);
    let body = resp.text().await.unwrap_or_default();
    let elapsed_ms = started.elapsed().as_millis();
    let remaining = TabFile::open(&path)?.count(&origin, now());
    if !status.is_success() {
        let why = settlement.as_ref().and_then(|s| s.error_reason.clone()).unwrap_or_default();
        let detail = serde_json::from_str::<serde_json::Value>(&body)
            .ok()
            .and_then(|v| v.get("error").and_then(|e| e.as_str()).map(str::to_string))
            .unwrap_or(body);
        return Err(PayError::Rejected(format!("server refused the tab token ({status}, {why}): {detail}")));
    }
    Ok(Some(TabFetched { status: status.as_u16(), body, settlement, remaining, elapsed_ms }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!("x402z-agent-tab-{}-{name}.json", std::process::id()));
        let _ = std::fs::remove_file(&p);
        p
    }

    fn tok(origin: &str, ends: u64, n: u8) -> StoredToken {
        StoredToken { origin: origin.into(), key_id: "k".into(), epoch_ends_at: ends, msg: b64(&[n]), randomizer: b64(&[0; 32]), sig: b64(&[n]) }
    }

    #[test]
    fn token_file_takes_per_origin_drops_expired_and_survives_reopen() {
        let p = tmp("file");
        {
            let mut f = TabFile::open(&p).unwrap();
            f.state.tokens = vec![tok("http://a:1", 100, 1), tok("http://b:1", 1_000, 2), tok("http://a:1", 1_000, 3)];
            f.save().unwrap();
        }
        let mut f = TabFile::open(&p).unwrap();
        assert_eq!(f.count("http://a:1", 500), 1); // the first one's epoch is over
        assert_eq!(f.take("http://a:1", 500).unwrap().msg, b64(&[3]));
        assert!(f.take("http://a:1", 500).is_none());
        f.save().unwrap();
        drop(f);
        let f = TabFile::open(&p).unwrap();
        assert_eq!(f.state.tokens, vec![tok("http://b:1", 1_000, 2)]);
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn invalid_token_file_is_an_error_not_a_reset() {
        let p = tmp("bad");
        std::fs::write(&p, b"{not json").unwrap();
        assert!(matches!(TabFile::open(&p), Err(PayError::Config(_))));
        assert_eq!(std::fs::read(&p).unwrap(), b"{not json");
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn offer_parsing_and_tab_size_limits() {
        let body = r#"{"x402Version":1,"error":"X-PAYMENT header is required","accepts":[
            {"scheme":"zcash-shielded","network":"zcash-testnet","maxAmountRequired":"100000","resource":"r","description":"d","payTo":"utest1sampleaddressforschemaonly000000","maxTimeoutSeconds":900,"asset":"ZEC",
             "extra":{"memoNonce":"x402z-1","expiresAt":2000000000,"tab":{"scheme":"zcash-shielded-tab","suite":"RSABSSA-SHA384-PSS-Randomized","keyId":"ab","publicKey":"AA==","unitPriceZat":100000,"maxTokens":20,"issuePath":"/tab/issue","epochEndsAt":2100000000}}}]}"#;
        let (ch, offer) = offer_from_402(body, 1_900_000_000).unwrap();
        assert_eq!((ch.memo_nonce.as_str(), offer.unit_price_zat, offer.max_tokens), ("x402z-1", 100_000, 20));
        assert_eq!(check_n(&offer, 5).unwrap(), 500_000);
        assert!(check_n(&offer, 0).is_err() && check_n(&offer, 21).is_err());
        assert_eq!(issue_url("http://127.0.0.1:4020/api/quote", &offer).unwrap(), "http://127.0.0.1:4020/tab/issue");
        // an issuePath on another origin (absolute or scheme-relative) is refused
        for off in ["http://evil.example/tab/issue", "//evil.example/tab/issue", "https://127.0.0.1:4020/tab/issue", "http://127.0.0.1:4021/tab/issue"] {
            let o = TabOffer { issue_path: off.into(), ..offer.clone() };
            assert!(matches!(issue_url("http://127.0.0.1:4020/api/quote", &o), Err(PayError::Runtime(m)) if m.contains("another origin")), "{off}");
        }
        assert_eq!(origin_of("http://127.0.0.1:4020/api/quote").unwrap(), "http://127.0.0.1:4020");
        // epoch already over
        assert!(offer_from_402(body, 2_050_000_000).is_err());
        // a 402 without a tab offer
        let plain = body.replace(r#","tab":{"scheme":"zcash-shielded-tab","suite":"RSABSSA-SHA384-PSS-Randomized","keyId":"ab","publicKey":"AA==","unitPriceZat":100000,"maxTokens":20,"issuePath":"/tab/issue","epochEndsAt":2100000000}"#, "");
        assert!(offer_from_402(&plain, 1_900_000_000).unwrap_err().message().contains("does not offer a Kage Tab"));
    }
}
