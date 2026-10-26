//! Paywalled demo API + dashboard.
//! `GET /api/quote` returns a 402 challenge until it gets a valid `X-PAYMENT` (settled through the
//! facilitator) or a facilitator-signed `X-PAYMENT-RECEIPT`.
//!
//! Two listeners:
//! - public (`X402Z_BIND`, default 127.0.0.1:4020): `/api/quote`, `/api/redeem`, `/tab/issue` and
//!   the challenge QR, nothing else;
//! - admin (`X402Z_ADMIN_BIND`, default 127.0.0.1:4029, `off` to disable): the merchant dashboard
//!   `/`, its feed `/api/events` and `/api/status`, plus the public routes so the dashboard can
//!   send a request from the same origin. The feed is the merchant's own: it never leaves the
//!   admin port, and even there a pending or refused attempt shows only a nonce prefix.
//!
//! Nothing here touches the chain: payment proof is the facilitator's ed25519 receipt.
//! Issued challenges and redeemed nonces live in SQLite (`X402Z_SERVER_DB`), so a restart
//! neither loses an open challenge nor un-burns a redeemed one.
mod store;
mod tab;

use std::{
    collections::HashMap,
    net::{IpAddr, SocketAddr},
    path::PathBuf,
    sync::RwLock,
    process::exit,
    sync::{Arc, Mutex},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use axum::{
    extract::{ConnectInfo, Query, State},
    http::{HeaderMap, HeaderValue, StatusCode},
    response::{Html, IntoResponse, Response},
    routing::get,
    Json, Router,
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use x402z_protocol::{
    challenge_uri, decode_header, encode_header, paid_in_time, reason, valid_nonce, valid_txid, FacilitatorRequest, Network, PaymentPayload, PaymentRequired,
    PaymentRequiredResponse, PaymentRequirements, ReceiptClaims, SettleResponse, SettlementResponse, VerifyStatus,
    DEFAULT_EXPIRY_GRACE_SECS, PAYMENT_HEADER, PAYMENT_RESPONSE_HEADER, RECEIPT_HEADER, SCHEME, SETTLE_PATH, X402_VERSION,
    TabIssueRequest, TabIssueResponse, TAB_ISSUE_PATH, TAB_SCHEME,
};

use store::Store;

const DASHBOARD: &str = include_str!("dashboard.html");
/// ~12 testnet blocks at 75 s: room for 1 confirmation plus a slow wallet.
const CHALLENGE_TTL_SECS: u64 = 15 * 60;
const DEFAULT_PRICE_ZAT: u64 = 100_000; // 0.001 TAZ
const MAX_EVENTS: usize = 200;
/// How long after expiry (+ grace) an unredeemed challenge is kept: a payer who paid in time
/// can still redeem its receipt this long after (`X402Z_REDEEM_WINDOW_SECS`).
const DEFAULT_REDEEM_WINDOW_SECS: u64 = 24 * 60 * 60;
/// Default per-IP budget of 402 challenges per minute (`X402Z_CHALLENGE_RATE_PER_MIN`, 0 = off).
const DEFAULT_CHALLENGE_RATE_PER_MIN: u32 = 30;
/// Forget idle buckets once the table grows past this many client IPs.
const MAX_TRACKED_IPS: usize = 10_000;
/// How long the payer may present the same (memoNonce, txid) again with `X-PAYMENT` and get the
/// content once more (`X402Z_RESEND_WINDOW_SECS`, 0 = never): a 200 lost on the way back must not
/// turn a settled payment into a refusal that makes the payer pay again.
const DEFAULT_RESEND_WINDOW_SECS: u64 = 10 * 60;
/// Default admin listener (dashboard, feed, status); loopback only unless configured otherwise.
const DEFAULT_ADMIN_BIND: &str = "127.0.0.1:4029";
/// Longest feed `detail` kept; anything a client sent is cut here.
const MAX_DETAIL_CHARS: usize = 160;

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

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct Event {
    id: u64,
    ts: u64,
    kind: &'static str,
    memo_nonce: Option<String>,
    amount_zat: Option<u64>,
    txid: Option<String>,
    detail: String,
}

struct Config {
    merchant_ua: String,
    price_zat: u64,
    receipt_key: VerifyingKey,
    facilitator_url: String,
    /// Bearer token the facilitator requires on `/settle` (`X402Z_FACILITATOR_TOKEN`).
    facilitator_token: String,
    agent_ledger: Option<String>,
    /// Key the agent sealed that ledger with (`X402Z_DEMO_AGENT_LEDGER_KEY`, the agent's
    /// `X402Z_AGENT_LEDGER_KEY`); without it the demo panel shows "Setup required".
    agent_ledger_key: Option<[u8; 32]>,
    /// Receipts with fewer confirmations are refused (0 = accept mempool-only payments).
    min_confirmations: u32,
    /// A payment mined up to this long after `expiresAt` still counts.
    expiry_grace_secs: u64,
    redeem_window_secs: u64,
    /// See [`DEFAULT_RESEND_WINDOW_SECS`].
    resend_window_secs: u64,
    /// Kage Tab: most tokens one payment may buy (0 turns the tab off) and the key epoch.
    tab_max_tokens: u32,
    tab_epoch_secs: u64,
    /// How long the previous tab key still issues paid tabs and accepts tokens after its epoch.
    tab_grace_secs: u64,
    /// Reverse proxies whose `X-Forwarded-For` is believed for the per-client limiter
    /// (`X402Z_TRUSTED_PROXIES`); empty means the TCP peer is the client.
    trusted_proxies: Vec<IpAddr>,
    /// Public origin of the paid API (`X402Z_PUBLIC_BASE_URL`, e.g. `https://api.example`), used for
    /// the 402's `resource`. Unset: built from `Host`, with `https` only when a trusted proxy's
    /// `X-Forwarded-Proto` says so.
    public_base_url: Option<String>,
}

/// Live event feed for the dashboard (in memory; the durable state is in [`Store`]).
#[derive(Default)]
struct Book {
    events: Vec<Event>,
    next_id: u64,
}

/// First characters of a nonce, for feed rows about a nonce that may still be unspent: enough for
/// the merchant to tell attempts apart, not enough to present the nonce with a payment.
fn nonce_hint(nonce: &str) -> String {
    let head: String = nonce.chars().take(12).collect();
    if head.len() < nonce.len() {
        format!("{head}…")
    } else {
        head
    }
}

/// Feed text is capped: parts of it (error messages about a malformed header) come from clients.
fn clip(detail: String) -> String {
    if detail.chars().count() <= MAX_DETAIL_CHARS {
        return detail;
    }
    let mut d: String = detail.chars().take(MAX_DETAIL_CHARS).collect();
    d.push('…');
    d
}

impl Book {
    fn push(&mut self, kind: &'static str, nonce: Option<&str>, amount: Option<u64>, txid: Option<&str>, detail: String) {
        self.next_id += 1;
        self.events.push(Event {
            id: self.next_id,
            ts: now(),
            kind,
            memo_nonce: nonce.map(|n| clip(n.to_string())),
            amount_zat: amount,
            txid: txid.map(|t| clip(t.to_string())),
            detail: clip(detail),
        });
        if self.events.len() > MAX_EVENTS {
            let extra = self.events.len() - MAX_EVENTS;
            self.events.drain(..extra);
        }
    }
}

/// Token bucket per client IP for issuing 402 challenges. Every challenge is a row in
/// `Book.open` until it expires, so an unthrottled client could grow that map without bound.
/// Capacity is `per_min` (the burst) and it refills at `per_min` tokens per minute.
struct ChallengeLimiter {
    per_min: u32,
    buckets: HashMap<IpAddr, (f64, Instant)>,
}

/// The limiter's key for a request: the TCP peer, or, when the peer is a configured trusted proxy,
/// the right-most `X-Forwarded-For` hop that is not one of those proxies. IPv6 clients are keyed
/// by their /64, since one host usually holds a whole /64.
fn client_key(peer: IpAddr, headers: &HeaderMap, trusted: &[IpAddr]) -> IpAddr {
    let mut ip = peer;
    if trusted.contains(&peer) {
        let hops = headers.get_all("x-forwarded-for").iter().filter_map(|v| v.to_str().ok()).flat_map(|v| v.split(','));
        let hops: Vec<IpAddr> = hops.filter_map(|h| h.trim().parse().ok()).collect();
        if let Some(c) = hops.iter().rev().find(|h| !trusted.contains(h)) {
            ip = *c;
        }
    }
    match ip {
        IpAddr::V6(v6) => match v6.to_ipv4_mapped() {
            Some(v4) => IpAddr::V4(v4),
            None => {
                let s = v6.segments();
                IpAddr::from([s[0], s[1], s[2], s[3], 0, 0, 0, 0])
            }
        },
        v4 => v4,
    }
}

impl ChallengeLimiter {
    fn new(per_min: u32) -> Self {
        Self { per_min, buckets: HashMap::new() }
    }

    /// Take one token for `ip`, or return how long until one is available.
    fn try_acquire(&mut self, ip: IpAddr, now: Instant) -> Result<(), Duration> {
        if self.per_min == 0 {
            return Ok(());
        }
        let cap = f64::from(self.per_min);
        let per_sec = cap / 60.0;
        if self.buckets.len() >= MAX_TRACKED_IPS && !self.buckets.contains_key(&ip) {
            // Drop buckets that have refilled completely; they carry no state worth keeping.
            self.buckets.retain(|_, (tokens, at)| *tokens + now.saturating_duration_since(*at).as_secs_f64() * per_sec < cap);
        }
        let (tokens, at) = self.buckets.entry(ip).or_insert((cap, now));
        *tokens = (*tokens + now.saturating_duration_since(*at).as_secs_f64() * per_sec).min(cap);
        *at = now;
        if *tokens >= 1.0 {
            *tokens -= 1.0;
            Ok(())
        } else {
            Err(Duration::from_secs_f64((1.0 - *tokens) / per_sec))
        }
    }
}

struct App {
    cfg: Config,
    store: Store,
    /// Client for the facilitator's `/settle`.
    http: reqwest::Client,
    /// Kage Tab issuing key, loaded from the store on first use.
    tab: RwLock<Option<Arc<tab::TabKey>>>,
    book: Mutex<Book>,
    limiter: Mutex<ChallengeLimiter>,
    /// When expired challenges were last pruned (unix seconds); see [`PRUNE_EVERY_SECS`].
    last_prune: std::sync::atomic::AtomicU64,
}

/// Expired, unredeemed challenges are pruned at most this often, not on every 402.
const PRUNE_EVERY_SECS: u64 = 60;

fn too_many_challenges(retry_after: Duration) -> Response {
    let secs = retry_after.as_secs_f64().ceil().max(1.0) as u64;
    let mut resp = (
        StatusCode::TOO_MANY_REQUESTS,
        Json(serde_json::json!({
            "error": "too many payment challenges from this client",
            "retryAfterSecs": secs,
        })),
    )
        .into_response();
    resp.headers_mut().insert("retry-after", HeaderValue::from(secs));
    resp
}

#[derive(Debug, PartialEq, Eq)]
enum ReceiptError {
    Malformed,
    BadSignature,
    UnknownNonce,
    Replayed,
    Expired { expires_at: u64, paid_at: u64 },
    Unconfirmed { have: u32, need: u32 },
    WrongAddress,
    Underpaid { paid: u64, want: u64 },
    Storage(String),
}

impl ReceiptError {
    fn reason(&self) -> String {
        match self {
            Self::Malformed => "receipt malformed".into(),
            Self::BadSignature => "receipt signature invalid".into(),
            Self::UnknownNonce => "memo nonce was never issued here".into(),
            Self::Replayed => "receipt already used".into(),
            Self::Expired { expires_at, paid_at } => {
                format!("payment landed at {paid_at}, after the challenge expired at {expires_at} (plus grace)")
            }
            Self::Unconfirmed { have, need } => format!("payment has {have} confirmation(s), need {need}; retry once it is mined"),
            Self::WrongAddress => "paid to a different address".into(),
            Self::Underpaid { paid, want } => format!("underpaid: {paid} zat < {want} zat"),
            Self::Storage(e) => e.clone(),
        }
    }
}

/// Decode and check signature only. Business checks happen in `settle`.
fn decode_receipt(key: &VerifyingKey, header: &str) -> Result<ReceiptClaims, ReceiptError> {
    let (claims_b64, sig_b64) = header.trim().split_once('.').ok_or(ReceiptError::Malformed)?;
    let claims_raw = URL_SAFE_NO_PAD.decode(claims_b64).map_err(|_| ReceiptError::Malformed)?;
    let sig_raw = URL_SAFE_NO_PAD.decode(sig_b64).map_err(|_| ReceiptError::Malformed)?;
    let sig = Signature::from_slice(&sig_raw).map_err(|_| ReceiptError::Malformed)?;
    key.verify(&claims_raw, &sig).map_err(|_| ReceiptError::BadSignature)?;
    serde_json::from_slice(&claims_raw).map_err(|_| ReceiptError::Malformed)
}

/// Match a signed receipt to an issued challenge and burn its nonce.
///
/// Expiry is judged by when the payment was mined (`minedAt` in the signed receipt), not by
/// when the receipt is redeemed: a payment mined by `expiresAt + grace` unlocks the content
/// even if the payer redeems later (within the redeem window). A mempool-only receipt has no
/// `minedAt`, so it must be redeemed in time.
fn settle(store: &Store, merchant_ua: &str, c: &ReceiptClaims, now_unix: u64, grace_secs: u64) -> Result<(), ReceiptError> {
    let row = store.get(&c.memo_nonce).map_err(ReceiptError::Storage)?.ok_or(ReceiptError::UnknownNonce)?;
    if row.spent_txid.is_some() {
        return Err(ReceiptError::Replayed);
    }
    check_receipt(&row.challenge, merchant_ua, c, now_unix, grace_secs)?;
    if !store.mark_spent(&c.memo_nonce, &c.txid, now_unix).map_err(ReceiptError::Storage)? {
        return Err(ReceiptError::Replayed);
    }
    Ok(())
}

/// The checks `settle` makes before burning: paid in time (by `minedAt`), to us, enough.
fn check_receipt(ch: &PaymentRequired, merchant_ua: &str, c: &ReceiptClaims, now_unix: u64, grace_secs: u64) -> Result<(), ReceiptError> {
    let paid_at = c.mined_at.unwrap_or(now_unix);
    if !paid_in_time(ch.expires_at, paid_at, grace_secs) {
        return Err(ReceiptError::Expired { expires_at: ch.expires_at, paid_at });
    }
    if c.pay_to != merchant_ua || ch.pay_to != merchant_ua {
        return Err(ReceiptError::WrongAddress);
    }
    if c.amount_zat < ch.amount_zat {
        return Err(ReceiptError::Underpaid { paid: c.amount_zat, want: ch.amount_zat });
    }
    Ok(())
}

fn issue_challenge(app: &App) -> Result<PaymentRequired, String> {
    let now_unix = now();
    let last = app.last_prune.load(std::sync::atomic::Ordering::Relaxed);
    if now_unix >= last.saturating_add(PRUNE_EVERY_SECS)
        && app
            .last_prune
            .compare_exchange(last, now_unix, std::sync::atomic::Ordering::Relaxed, std::sync::atomic::Ordering::Relaxed)
            .is_ok()
    {
        let keep_after = app.cfg.expiry_grace_secs.saturating_add(app.cfg.redeem_window_secs);
        app.store.prune_unspent_expired_before(now_unix.saturating_sub(keep_after))?;
    }
    let mut raw = [0u8; 12];
    rand::thread_rng().fill_bytes(&mut raw);
    let ch = PaymentRequired {
        scheme: SCHEME.into(),
        network: Network::Testnet,
        pay_to: app.cfg.merchant_ua.clone(),
        amount_zat: app.cfg.price_zat,
        memo_nonce: format!("x402z-{}", hex::encode(raw)),
        expires_at: now_unix + CHALLENGE_TTL_SECS,
    };
    app.store.insert(&ch)?;
    Ok(ch)
}

fn storage_error(e: String) -> Response {
    (StatusCode::SERVICE_UNAVAILABLE, Json(serde_json::json!({ "error": e }))).into_response()
}

const DESCRIPTION: &str = "Kage demo quote: paid content unlocked by a shielded Zcash testnet payment";
const MIME: &str = "application/json";
const QUOTE_PATH: &str = "/api/quote";

/// Absolute URL of the paid resource, for `PaymentRequirements.resource`: the configured public
/// origin, else `Host` with the scheme a trusted TLS-terminating proxy reports in
/// `X-Forwarded-Proto` (`http` when the peer is not a trusted proxy or sends anything else).
fn resource_url(cfg: &Config, peer: IpAddr, headers: &HeaderMap) -> String {
    if let Some(base) = &cfg.public_base_url {
        return format!("{base}{QUOTE_PATH}");
    }
    let Some(host) = headers.get("host").and_then(|h| h.to_str().ok()) else {
        return QUOTE_PATH.to_string();
    };
    let forwarded_https = cfg.trusted_proxies.contains(&peer)
        && headers
            .get("x-forwarded-proto")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.split(',').next())
            .is_some_and(|p| p.trim().eq_ignore_ascii_case("https"));
    let scheme = if forwarded_https { "https" } else { "http" };
    format!("{scheme}://{host}{QUOTE_PATH}")
}

/// `X402Z_PUBLIC_BASE_URL`: an `http(s)://host[:port]` origin, no path; a trailing `/` is dropped.
fn parse_public_base_url(raw: &str) -> Result<String, String> {
    let s = raw.trim();
    let lower = s.to_ascii_lowercase();
    let scheme_len = match ["https://", "http://"].iter().find(|p| lower.starts_with(**p)) {
        Some(p) => p.len(),
        None => return Err(format!("'{raw}' must start with https:// or http://")),
    };
    let rest = s[scheme_len..].trim_end_matches('/');
    if rest.is_empty() || rest.contains('/') || rest.contains('?') || rest.contains('#') || rest.chars().any(char::is_whitespace) {
        return Err(format!("'{raw}' must be an origin like https://api.example (no path)"));
    }
    Ok(format!("{}{rest}", &lower[..scheme_len]))
}

/// The x402 `accepts` entry for a challenge, with a ZIP 321 URI any Zcash wallet can pay.
fn requirements(ch: &PaymentRequired, resource: &str) -> PaymentRequirements {
    let mut r = PaymentRequirements::from_challenge(ch, resource, DESCRIPTION, MIME, CHALLENGE_TTL_SECS);
    if let Ok(mut extra) = r.zcash_extra() {
        extra.payment_uri = Some(challenge_uri(ch));
        r.set_zcash_extra(&extra);
    }
    r
}

fn with_header(mut resp: Response, name: &'static str, value: &str) -> Response {
    if let Ok(v) = HeaderValue::from_str(value) {
        resp.headers_mut().insert(name, v);
    }
    resp
}

/// HTTP 402 with the x402 v1 body `{x402Version, accepts, error}`. A failed payment attempt also
/// gets `X-PAYMENT-RESPONSE` (base64 SettlementResponse) and the human-readable `x-payment-error`.
fn payment_required(app: &App, ch: &PaymentRequired, resource: &str, error: Option<String>, settlement: Option<SettlementResponse>) -> Response {
    let mut req = requirements(ch, resource);
    match tab_key(app) {
        Ok(Some(k)) => {
            if let Ok(mut extra) = req.zcash_extra() {
                let o = tab::offer(&k, app.cfg.price_zat, app.cfg.tab_max_tokens, app.cfg.tab_epoch_secs, app.cfg.tab_grace_secs);
                extra.tab = serde_json::to_value(o).ok();
                req.set_zcash_extra(&extra);
            }
        }
        Ok(None) => {}
        Err(e) => eprintln!("warning: tab offer left out: {e}"),
    }
    let body = PaymentRequiredResponse {
        x402_version: X402_VERSION,
        accepts: vec![req],
        error: error.clone().unwrap_or_else(|| format!("{PAYMENT_HEADER} header is required")),
    };
    let mut resp = (StatusCode::PAYMENT_REQUIRED, Json(body)).into_response();
    if let Some(e) = error {
        resp = with_header(resp, "x-payment-error", &e);
    }
    if let Some(s) = settlement {
        resp = with_header(resp, PAYMENT_RESPONSE_HEADER, &encode_header(&s));
    }
    resp
}

/// 200 with the paid content and `X-PAYMENT-RESPONSE: base64({success: true, transaction, ...})`.
fn paid_response(c: &ReceiptClaims) -> Response {
    let resp = Json(serde_json::json!({
        "product": "Kage demo quote",
        "servedAt": now(),
        "settlement": {
            "memoNonce": c.memo_nonce,
            "amountZat": c.amount_zat,
            "txid": c.txid,
            "confirmations": c.confirmations,
            "minedAt": c.mined_at,
        },
        "quote": "Paid content unlocked by a shielded Zcash testnet payment. The merchant saw it only through its incoming viewing key; the chain shows neither amount nor address.",
    }))
    .into_response();
    with_header(resp, PAYMENT_RESPONSE_HEADER, &encode_header(&SettlementResponse::ok(&c.txid)))
}

/// Take one token of `client`'s budget. On refusal the feed gets a row (without the client's
/// address) and the caller gets the 429 to send.
fn charge(app: &App, client: IpAddr, error: Option<&String>, settlement: Option<&SettlementResponse>) -> Result<(), Response> {
    let Err(wait) = app.limiter.lock().unwrap().try_acquire(client, Instant::now()) else {
        return Ok(());
    };
    app.book.lock().unwrap().push("rejected", None, None, None, format!("rate limited (retry in {}s)", wait.as_secs().max(1)));
    let mut resp = too_many_challenges(wait);
    if let Some(e) = error {
        resp = with_header(resp, "x-payment-error", e);
    }
    if let Some(s) = settlement {
        resp = with_header(resp, PAYMENT_RESPONSE_HEADER, &encode_header(s));
    }
    Err(resp)
}

/// Whether [`mint`] still has to take a limiter token: `Charge` for a fresh request, `Charged`
/// when the request already paid for its attempt (an `X-PAYMENT` takes its token up front).
#[derive(Clone, Copy, PartialEq, Eq)]
enum Budget {
    Charge,
    Charged,
}

/// Mint a new challenge (rate limited per client) and answer 402 with it.
fn mint(app: &App, client: IpAddr, budget: Budget, resource: &str, detail: &str, error: Option<String>, settlement: Option<SettlementResponse>) -> Response {
    if budget == Budget::Charge {
        if let Err(resp) = charge(app, client, error.as_ref(), settlement.as_ref()) {
            return resp;
        }
    }
    // SQLite work happens outside the feed lock; only the push holds it.
    let ch = match issue_challenge(app) {
        Ok(ch) => ch,
        Err(e) => return storage_error(e),
    };
    app.book.lock().unwrap().push("challenge", Some(&ch.memo_nonce), Some(ch.amount_zat), None, detail.into());
    payment_required(app, &ch, resource, error, settlement)
}

/// The active tab key (cached; rotated when its epoch ends). `None` when the tab is off.
fn tab_key(app: &App) -> Result<Option<Arc<tab::TabKey>>, String> {
    if app.cfg.tab_max_tokens == 0 {
        return Ok(None);
    }
    let now_unix = now();
    let fresh = |k: &Arc<tab::TabKey>| now_unix < k.created_at.saturating_add(app.cfg.tab_epoch_secs);
    if let Some(k) = app.tab.read().unwrap().as_ref().filter(|k| fresh(k)) {
        return Ok(Some(k.clone()));
    }
    // Rotate under the write lock, checking again: two requests at the epoch boundary must not
    // each install a key (the first one's offers would then name a key that is already retired).
    let mut slot = app.tab.write().unwrap();
    if let Some(k) = slot.as_ref().filter(|k| fresh(k)) {
        return Ok(Some(k.clone()));
    }
    let k = Arc::new(tab::load_or_rotate(&app.store, now_unix, app.cfg.tab_epoch_secs)?);
    *slot = Some(k.clone());
    Ok(Some(k))
}

/// The key a tab request names: the active one, or the previous one inside its grace period.
fn tab_key_for(app: &App, key_id: &str) -> Result<Option<Arc<tab::TabKey>>, String> {
    let Some(active) = tab_key(app)? else { return Ok(None) };
    if active.key_id == key_id {
        return Ok(Some(active));
    }
    Ok(tab::previous_in_grace(&app.store, now(), app.cfg.tab_epoch_secs, app.cfg.tab_grace_secs)?
        .filter(|k| k.key_id == key_id)
        .map(Arc::new))
}

/// Result of redeeming a (memoNonce, txid) pair through the facilitator.
#[derive(Debug)]
enum Redeem {
    Paid(ReceiptClaims),
    /// The same (memoNonce, txid) that already settled, presented again inside the resend window:
    /// the facilitator confirmed it again, so the content is served again. Nothing new is burned.
    Resent(ReceiptClaims),
    /// Found but not confirmed enough yet, or not visible to lightwalletd yet: retry the same txid.
    Pending { challenge: PaymentRequired, message: String },
    /// Definitively not a payment for this challenge (or the nonce is spent / unknown).
    Refused { reason: String, message: String },
    /// Facilitator or storage trouble; nothing was burned.
    Unavailable(String),
}

fn refused(reason: &str, message: impl Into<String>) -> Redeem {
    Redeem::Refused { reason: reason.into(), message: message.into() }
}

/// Ask the facilitator to settle `txid` against the stored challenge `nonce`, check its signed
/// receipt with the facilitator's public key, and burn the nonce. With `resend`, the pair that
/// already spent the nonce is answered [`Redeem::Resent`] inside the resend window.
async fn redeem(app: &App, nonce: &str, txid: &str, resource: &str, resend: Resend) -> Redeem {
    let (row, c) = match confirmed_receipt(app, nonce, txid, resource, resend).await {
        Ok(x) => x,
        Err(r) => return r,
    };
    if row.spent_txid.is_some() {
        // confirmed_receipt lets a spent row through only for its own txid inside the window
        return Redeem::Resent(c);
    }
    match settle(&app.store, &app.cfg.merchant_ua, &c, now(), app.cfg.expiry_grace_secs) {
        Ok(()) => Redeem::Paid(c),
        Err(e) => receipt_refusal(e),
    }
}

fn receipt_refusal(e: ReceiptError) -> Redeem {
    match e {
        ReceiptError::Storage(e) => Redeem::Unavailable(e),
        e @ ReceiptError::Replayed => refused(reason::NONCE_USED, e.reason()),
        e @ ReceiptError::Expired { .. } => refused(reason::EXPIRED, e.reason()),
        e => refused(reason::INVALID_PAYMENT, e.reason()),
    }
}

/// Whether a spent nonce may be presented again with the txid that spent it.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Resend {
    /// `X-PAYMENT`: yes, inside `X402Z_RESEND_WINDOW_SECS` of the first 200.
    Allowed,
    /// Paste-txid redeem and tab issuance: a spent nonce is always refused.
    Refused,
}

/// Whether `row` (already spent) may be answered again for `txid` at `now_unix`. A nonce that
/// bought a Kage Tab never is: its payment was for the tokens, not for this request.
fn resend_ok(row: &store::Row, txid: &str, now_unix: u64, window_secs: u64) -> bool {
    window_secs > 0
        && !row.spent_for_tab
        && row.spent_txid.as_deref().is_some_and(|t| t.eq_ignore_ascii_case(txid.trim()))
        && row.spent_at.is_some_and(|at| now_unix <= at.saturating_add(window_secs))
}

/// The facilitator-confirmed, signature-checked receipt for (nonce, txid). Burns nothing.
async fn confirmed_receipt(app: &App, nonce: &str, txid: &str, resource: &str, resend: Resend) -> Result<(store::Row, ReceiptClaims), Redeem> {
    let row = match app.store.get(nonce) {
        Err(e) => return Err(Redeem::Unavailable(e)),
        Ok(None) => return Err(refused(reason::UNKNOWN_CHALLENGE, ReceiptError::UnknownNonce.reason())),
        Ok(Some(r)) => r,
    };
    if row.spent_txid.is_some() && !(resend == Resend::Allowed && resend_ok(&row, txid, now(), app.cfg.resend_window_secs)) {
        return Err(refused(reason::NONCE_USED, ReceiptError::Replayed.reason()));
    }
    let url = format!("{}{SETTLE_PATH}", app.cfg.facilitator_url.trim_end_matches('/'));
    let body = FacilitatorRequest {
        x402_version: X402_VERSION,
        payment_payload: PaymentPayload::zcash(txid, nonce),
        payment_requirements: requirements(&row.challenge, resource),
    };
    let resp = match app.http.post(&url).bearer_auth(&app.cfg.facilitator_token).json(&body).send().await {
        Ok(r) => r,
        Err(e) => return Err(Redeem::Unavailable(format!("facilitator unreachable at {url}: {e}"))),
    };
    let code = resp.status();
    if code == StatusCode::UNAUTHORIZED {
        return Err(Redeem::Unavailable(format!(
            "Setup required: the facilitator at {url} refused this server's X402Z_FACILITATOR_TOKEN (401); both must hold the same value"
        )));
    }
    let s: SettleResponse = match resp.json().await {
        Ok(s) => s,
        Err(e) => return Err(Redeem::Unavailable(format!("facilitator answered {code} with an unreadable body: {e}"))),
    };
    let msg = s.error.clone().unwrap_or_default();
    match s.status {
        VerifyStatus::Pending | VerifyStatus::NotFound => {
            return Err(Redeem::Pending { challenge: row.challenge, message: if msg.is_empty() { "payment not confirmed yet".into() } else { msg } })
        }
        VerifyStatus::Rejected => {
            let why = s.settlement.error_reason.unwrap_or_else(|| reason::INVALID_PAYMENT.into());
            return Err(Redeem::Refused { reason: why, message: msg });
        }
        VerifyStatus::Error => return Err(Redeem::Unavailable(format!("facilitator: {msg}"))),
        VerifyStatus::Confirmed => {}
    }
    let Some(receipt) = s.receipt else {
        return Err(Redeem::Unavailable("facilitator confirmed without a receipt".into()));
    };
    let c = match decode_receipt(&app.cfg.receipt_key, &receipt) {
        Ok(c) => c,
        Err(e) => return Err(refused(reason::INVALID_PAYMENT, e.reason())),
    };
    if c.memo_nonce != nonce || !c.txid.eq_ignore_ascii_case(txid.trim()) {
        return Err(refused(reason::INVALID_PAYMENT, "receipt does not match the payment that was presented"));
    }
    if c.confirmations < app.cfg.min_confirmations {
        let e = ReceiptError::Unconfirmed { have: c.confirmations, need: app.cfg.min_confirmations };
        return Err(Redeem::Pending { challenge: row.challenge, message: e.reason() });
    }
    Ok((row, c))
}

fn record_paid(app: &App, c: &ReceiptClaims, via: &str) {
    let conf = if c.confirmations == 0 { "mempool".to_string() } else { format!("{} conf", c.confirmations) };
    app.book.lock().unwrap().push("paid", Some(&c.memo_nonce), Some(c.amount_zat), Some(&c.txid), format!("unlocked via {via} ({conf})"));
}

/// A client-supplied (memoNonce, txid) must have the shape the server issues and the chain uses
/// before it reaches the store, the facilitator or the feed.
fn check_pair_shape(nonce: &str, txid: &str) -> Result<(), String> {
    if !valid_nonce(nonce) {
        return Err(format!("memoNonce must be 1 to {} characters of [A-Za-z0-9_-]", x402z_protocol::MAX_NONCE_LEN));
    }
    if !valid_txid(txid) {
        return Err("txid must be 64 hex characters".into());
    }
    Ok(())
}

/// 400 for an `X-PAYMENT` whose nonce or txid can't be real. Nothing is stored, asked or logged.
fn malformed_payment(msg: String) -> Response {
    let resp = (StatusCode::BAD_REQUEST, Json(serde_json::json!({ "error": msg }))).into_response();
    let resp = with_header(resp, "x-payment-error", &msg);
    with_header(resp, PAYMENT_RESPONSE_HEADER, &encode_header(&SettlementResponse::failed(reason::INVALID_PAYLOAD)))
}

/// `X-PAYMENT` (x402 v1): base64 `{x402Version, scheme, network, payload: {txid, memoNonce}}`.
///
/// Every attempt costs one token of the client's budget before the facilitator (and through it
/// lightwalletd) is asked anything, so a client can't make the server poll the chain without
/// limit by presenting payments that stay pending.
async fn x_payment(app: &App, client: IpAddr, resource: &str, header: &str) -> Response {
    let decoded = decode_header::<PaymentPayload>(header);
    if let Ok(p) = &decoded {
        if p.scheme == TAB_SCHEME {
            return tab_payment(app, client, resource, p);
        }
    }
    let payload = decoded.map_err(|e| (e.reason(), format!("{PAYMENT_HEADER} {e}"))).and_then(|p| {
        p.zcash_payload().map_err(|e| (e.reason(), format!("{PAYMENT_HEADER} {e}")))
    });
    let p = match payload {
        Ok(mut p) => {
            p.memo_nonce = p.memo_nonce.trim().to_string();
            p.txid = p.txid.trim().to_ascii_lowercase();
            if let Err(msg) = check_pair_shape(&p.memo_nonce, &p.txid) {
                return malformed_payment(format!("{PAYMENT_HEADER} {msg}"));
            }
            p
        }
        Err((why, msg)) => {
            let settlement = SettlementResponse::failed(why);
            if let Err(resp) = charge(app, client, Some(&msg), Some(&settlement)) {
                return resp;
            }
            app.book.lock().unwrap().push("rejected", None, None, None, msg.clone());
            return mint(app, client, Budget::Charged, resource, "402 re-issued after a malformed X-PAYMENT", Some(msg), Some(settlement));
        }
    };
    if let Err(resp) = charge(app, client, None, None) {
        return resp;
    }
    match redeem(app, &p.memo_nonce, &p.txid, resource, Resend::Allowed).await {
        Redeem::Paid(c) => {
            record_paid(app, &c, PAYMENT_HEADER);
            paid_response(&c)
        }
        Redeem::Resent(c) => {
            app.book.lock().unwrap().push(
                "paid",
                Some(&c.memo_nonce),
                Some(c.amount_zat),
                Some(&c.txid),
                "served again: the same X-PAYMENT inside the resend window (nothing new burned)".into(),
            );
            paid_response(&c)
        }
        Redeem::Pending { challenge, message } => {
            // The nonce is still unspent: the feed shows only its prefix next to the txid.
            app.book.lock().unwrap().push("pending", Some(&nonce_hint(&challenge.memo_nonce)), Some(challenge.amount_zat), Some(&p.txid), message.clone());
            // Same challenge again: the payer retries the same X-PAYMENT once the tx confirms.
            payment_required(app, &challenge, resource, Some(message), Some(SettlementResponse::failed(reason::PENDING)))
        }
        Redeem::Refused { reason: why, message } => {
            app.book.lock().unwrap().push("rejected", Some(&nonce_hint(&p.memo_nonce)), None, Some(&p.txid), message.clone());
            mint(app, client, Budget::Charged, resource, "402 re-issued after rejection", Some(message), Some(SettlementResponse::failed(&why)))
        }
        Redeem::Unavailable(message) => {
            app.book.lock().unwrap().push("rejected", Some(&nonce_hint(&p.memo_nonce)), None, Some(&p.txid), message.clone());
            let resp = (StatusCode::BAD_GATEWAY, Json(serde_json::json!({ "error": message }))).into_response();
            with_header(resp, PAYMENT_RESPONSE_HEADER, &encode_header(&SettlementResponse::failed(reason::FACILITATOR_ERROR)))
        }
    }
}

/// `X-PAYMENT` with scheme `zcash-shielded-tab`: spend one blind-signed tab token.
fn tab_payment(app: &App, client: IpAddr, resource: &str, p: &PaymentPayload) -> Response {
    let payload = p.tab_payload();
    let key = match (&payload, tab_key(app)) {
        (_, Err(e)) => return storage_error(e),
        // A token from the previous key is honoured inside its grace period.
        (Ok(t), Ok(Some(active))) if t.key_id != active.key_id => match tab_key_for(app, &t.key_id) {
            Err(e) => return storage_error(e),
            Ok(k) => Ok(Some(k.unwrap_or(active))),
        },
        (_, k) => k,
    };
    let result = match (payload, key) {
        (Err(e), _) => Err((e.reason(), format!("{PAYMENT_HEADER} {e}"))),
        (_, Err(e)) => return storage_error(e),
        (_, Ok(None)) => Err((reason::INVALID_SCHEME, "this server does not offer a tab".to_string())),
        (Ok(t), Ok(Some(k))) => tab::spend(&app.store, &k, &t, now()).map_err(|e| {
            let why = match e {
                tab::TabError::DoubleSpent => reason::NONCE_USED,
                tab::TabError::OtherEpoch { .. } => reason::EXPIRED,
                _ => reason::INVALID_PAYMENT,
            };
            (why, e.reason())
        }),
    };
    match result {
        Ok(token_id) => {
            app.book.lock().unwrap().push("paid", None, Some(app.cfg.price_zat), None, format!("unlocked by tab token {}…", &token_id[..12]));
            let resp = Json(serde_json::json!({
                "product": "Kage demo quote",
                "servedAt": now(),
                "settlement": { "tab": true, "token": token_id },
                "quote": "Paid content unlocked by a blind-signed tab token. The server can't link this request to the shielded payment that bought the tab.",
            }))
            .into_response();
            with_header(resp, PAYMENT_RESPONSE_HEADER, &encode_header(&SettlementResponse::ok(&format!("tab:{}", &token_id[..16]))))
        }
        Err((why, msg)) => {
            app.book.lock().unwrap().push("rejected", None, None, None, msg.clone());
            mint(app, client, Budget::Charge, resource, "402 re-issued after a refused tab token", Some(msg), Some(SettlementResponse::failed(why)))
        }
    }
}

/// `POST /tab/issue {memoNonce, txid, keyId, blinded[n]}`: one shielded payment of at least
/// `n * price` to the challenge buys n blind signatures. The nonce is burned once (durably), so
/// it can never issue again; an underpaid n burns nothing and can be retried with fewer.
async fn tab_issue(State(app): State<Arc<App>>, ConnectInfo(peer): ConnectInfo<SocketAddr>, headers: HeaderMap, body: axum::body::Bytes) -> Response {
    let fail = |code: StatusCode, why: &str, msg: String| {
        (code, Json(serde_json::json!({ "status": "rejected", "reason": why, "message": msg }))).into_response()
    };
    let req: TabIssueRequest = match serde_json::from_slice(&body) {
        Ok(r) => r,
        Err(e) => return fail(StatusCode::BAD_REQUEST, reason::INVALID_PAYLOAD, format!("body must be {{memoNonce, txid, keyId, blinded}}: {e}")),
    };
    let active = match tab_key(&app) {
        Ok(Some(k)) => k,
        Ok(None) => return fail(StatusCode::NOT_FOUND, reason::INVALID_SCHEME, "this server does not offer a tab".into()),
        Err(e) => return storage_error(e),
    };
    // The previous key still issues a tab paid under it while its grace period lasts; any other
    // key is a 409 that burns nothing: blind again for the active key and retry the same payment.
    let k = match tab_key_for(&app, &req.key_id) {
        Ok(Some(k)) => k,
        Ok(None) => {
            return (
                StatusCode::CONFLICT,
                Json(serde_json::json!({
                    "status": "rejected",
                    "reason": reason::TAB_KEY_ROTATED,
                    "message": format!("keyId {} is not the active tab key {}; nothing was burned: blind for the active key and retry with the same memoNonce and txid", req.key_id, active.key_id),
                    "activeKeyId": active.key_id,
                })),
            )
                .into_response()
        }
        Err(e) => return storage_error(e),
    };
    let n = req.blinded.len();
    if n == 0 || n > app.cfg.tab_max_tokens as usize {
        return fail(StatusCode::BAD_REQUEST, reason::INVALID_PAYLOAD, format!("blinded must hold 1 to {} messages", app.cfg.tab_max_tokens));
    }
    let blinded = match tab::parse_blinded(&k, &req.blinded) {
        Ok(b) => b,
        Err(e) => return fail(StatusCode::BAD_REQUEST, reason::INVALID_PAYLOAD, e),
    };
    let txid = req.txid.trim().to_ascii_lowercase();
    let nonce = req.memo_nonce.trim();
    if let Err(msg) = check_pair_shape(nonce, &txid) {
        return fail(StatusCode::BAD_REQUEST, reason::INVALID_PAYLOAD, msg);
    }
    let client = client_key(peer.ip(), &headers, &app.cfg.trusted_proxies);
    if let Err(wait) = app.limiter.lock().unwrap().try_acquire(client, Instant::now()) {
        return too_many_challenges(wait);
    }
    let resource = resource_url(&app.cfg, peer.ip(), &headers);
    let (row, c) = match confirmed_receipt(&app, nonce, &txid, &resource, Resend::Refused).await {
        Ok(x) => x,
        Err(Redeem::Pending { message, .. }) => {
            return (StatusCode::ACCEPTED, Json(serde_json::json!({ "status": "pending", "reason": reason::PENDING, "message": message }))).into_response()
        }
        Err(Redeem::Refused { reason: why, message }) => return fail(StatusCode::UNPROCESSABLE_ENTITY, &why, message),
        Err(Redeem::Unavailable(m)) => return fail(StatusCode::BAD_GATEWAY, reason::FACILITATOR_ERROR, m),
        Err(Redeem::Paid(_) | Redeem::Resent(_)) => unreachable!("confirmed_receipt never burns"),
    };
    if let Err(e) = check_receipt(&row.challenge, &app.cfg.merchant_ua, &c, now(), app.cfg.expiry_grace_secs) {
        return match receipt_refusal(e) {
            Redeem::Refused { reason: why, message } => fail(StatusCode::UNPROCESSABLE_ENTITY, &why, message),
            Redeem::Unavailable(m) => fail(StatusCode::SERVICE_UNAVAILABLE, reason::FACILITATOR_ERROR, m),
            _ => fail(StatusCode::UNPROCESSABLE_ENTITY, reason::INVALID_PAYMENT, "refused".into()),
        };
    }
    let paid_for = c.amount_zat / app.cfg.price_zat.max(1);
    if (n as u64) > paid_for {
        return fail(
            StatusCode::UNPROCESSABLE_ENTITY,
            reason::INVALID_PAYMENT,
            format!("underpaid for {n} tokens: paid {} zat covers {paid_for} at {} zat each; nothing burned, retry with at most {paid_for}", c.amount_zat, app.cfg.price_zat),
        );
    }
    match app.store.spend_for_tab(&c.memo_nonce, &c.txid, &k.key_id, n as u32, now()) {
        Ok(true) => {}
        Ok(false) => return fail(StatusCode::UNPROCESSABLE_ENTITY, reason::NONCE_USED, ReceiptError::Replayed.reason()),
        Err(e) => return fail(StatusCode::SERVICE_UNAVAILABLE, reason::FACILITATOR_ERROR, e),
    }
    let sigs = match tab::blind_sign_all(&k, &blinded) {
        Ok(s) => s,
        Err(e) => return fail(StatusCode::INTERNAL_SERVER_ERROR, reason::FACILITATOR_ERROR, e),
    };
    app.book.lock().unwrap().push("paid", Some(&c.memo_nonce), Some(c.amount_zat), Some(&c.txid), format!("tab of {n} tokens issued"));
    Json(TabIssueResponse { key_id: k.key_id.clone(), blind_signatures: sigs }).into_response()
}

async fn quote(State(app): State<Arc<App>>, ConnectInfo(peer): ConnectInfo<SocketAddr>, headers: HeaderMap) -> Response {
    let resource = resource_url(&app.cfg, peer.ip(), &headers);
    let client = client_key(peer.ip(), &headers, &app.cfg.trusted_proxies);
    if let Some(h) = headers.get(PAYMENT_HEADER) {
        let h = h.to_str().unwrap_or_default().to_string();
        return x_payment(&app, client, &resource, &h).await;
    }
    // Back-compat: a facilitator receipt presented directly (`X-PAYMENT-RECEIPT`).
    let Some(receipt) = headers.get(RECEIPT_HEADER).and_then(|v| v.to_str().ok()) else {
        return mint(&app, client, Budget::Charge, &resource, "402 issued", None, None);
    };
    let result = decode_receipt(&app.cfg.receipt_key, receipt).and_then(|c| {
        // Checked before settle so an unconfirmed payment doesn't burn the nonce and can retry.
        if c.confirmations < app.cfg.min_confirmations {
            return Err(ReceiptError::Unconfirmed { have: c.confirmations, need: app.cfg.min_confirmations });
        }
        settle(&app.store, &app.cfg.merchant_ua, &c, now(), app.cfg.expiry_grace_secs).map(|_| c)
    });
    match result {
        Ok(c) => {
            record_paid(&app, &c, RECEIPT_HEADER);
            paid_response(&c)
        }
        Err(e) => {
            app.book.lock().unwrap().push("rejected", None, None, None, e.reason());
            let settlement = matches!(e, ReceiptError::Expired { .. }).then(|| SettlementResponse::failed(reason::EXPIRED));
            mint(&app, client, Budget::Charge, &resource, "402 re-issued after rejection", Some(e.reason()), settlement)
        }
    }
}

/// `POST /api/redeem` body: a challenge paid from any wallet, identified by its nonce.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RedeemRequest {
    memo_nonce: String,
    txid: String,
}

/// `POST /api/redeem`: the dashboard's "paste txid" path for payments made from any Zcash wallet
/// (for example by scanning the ZIP 321 QR). Same checks as `X-PAYMENT`, JSON answers:
/// 200 paid (with the content), 202 pending (retry the same txid), 422 rejected, 502 facilitator
/// down, 429 rate limited. Attempts share the per-IP challenge budget.
async fn redeem_api(
    State(app): State<Arc<App>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    let req: RedeemRequest = match serde_json::from_slice(&body) {
        Ok(r) => r,
        Err(e) => {
            return (StatusCode::BAD_REQUEST, Json(serde_json::json!({ "status": "rejected", "reason": reason::INVALID_PAYLOAD, "message": format!("body must be {{memoNonce, txid}}: {e}") })))
                .into_response()
        }
    };
    let txid = req.txid.trim().to_ascii_lowercase();
    let nonce = req.memo_nonce.trim().to_string();
    if let Err(msg) = check_pair_shape(&nonce, &txid) {
        return (StatusCode::BAD_REQUEST, Json(serde_json::json!({ "status": "rejected", "reason": reason::INVALID_PAYLOAD, "message": msg }))).into_response();
    }
    let client = client_key(peer.ip(), &headers, &app.cfg.trusted_proxies);
    if let Err(wait) = app.limiter.lock().unwrap().try_acquire(client, Instant::now()) {
        return too_many_challenges(wait);
    }
    let resource = resource_url(&app.cfg, peer.ip(), &headers);
    match redeem(&app, &nonce, &txid, &resource, Resend::Refused).await {
        Redeem::Resent(_) => unreachable!("the paste-txid redeem never resends"),
        Redeem::Paid(c) => {
            record_paid(&app, &c, "pasted txid");
            let content = paid_response(&c);
            let bytes = axum::body::to_bytes(content.into_body(), 1 << 20).await.unwrap_or_default();
            let content: serde_json::Value = serde_json::from_slice(&bytes).unwrap_or_default();
            Json(serde_json::json!({ "status": "paid", "settlement": SettlementResponse::ok(&c.txid), "content": content })).into_response()
        }
        Redeem::Pending { challenge, message } => {
            app.book.lock().unwrap().push("pending", Some(&nonce_hint(&challenge.memo_nonce)), Some(challenge.amount_zat), Some(&txid), message.clone());
            (StatusCode::ACCEPTED, Json(serde_json::json!({ "status": "pending", "reason": reason::PENDING, "message": message }))).into_response()
        }
        Redeem::Refused { reason: why, message } => {
            app.book.lock().unwrap().push("rejected", Some(&nonce_hint(&nonce)), None, Some(&txid), message.clone());
            (StatusCode::UNPROCESSABLE_ENTITY, Json(serde_json::json!({ "status": "rejected", "reason": why, "message": message }))).into_response()
        }
        Redeem::Unavailable(message) => {
            (StatusCode::BAD_GATEWAY, Json(serde_json::json!({ "status": "error", "reason": reason::FACILITATOR_ERROR, "message": message }))).into_response()
        }
    }
}

/// `GET /api/challenge/{nonce}/qr.svg`: QR code of the challenge's ZIP 321 URI, rendered here
/// (no external service sees the address, amount or nonce).
async fn challenge_qr(State(app): State<Arc<App>>, axum::extract::Path(nonce): axum::extract::Path<String>) -> Response {
    let row = match app.store.get(&nonce) {
        Ok(Some(r)) => r,
        Ok(None) => return (StatusCode::NOT_FOUND, "unknown challenge").into_response(),
        Err(e) => return storage_error(e),
    };
    match qr_svg(&challenge_uri(&row.challenge)) {
        Ok(svg) => ([("content-type", "image/svg+xml"), ("cache-control", "no-store")], svg).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e).into_response(),
    }
}

fn qr_svg(data: &str) -> Result<String, String> {
    let code = qrcode::QrCode::with_error_correction_level(data.as_bytes(), qrcode::EcLevel::M).map_err(|e| e.to_string())?;
    Ok(code
        .render::<qrcode::render::svg::Color>()
        .min_dimensions(240, 240)
        .quiet_zone(true)
        .dark_color(qrcode::render::svg::Color("#0c0c0d"))
        .light_color(qrcode::render::svg::Color("#ffffff"))
        .build())
}

#[derive(Deserialize)]
struct Since {
    since: Option<u64>,
}

async fn events(State(app): State<Arc<App>>, Query(q): Query<Since>) -> Json<Vec<Event>> {
    let since = q.since.unwrap_or(0);
    let book = app.book.lock().unwrap();
    Json(book.events.iter().filter(|e| e.id > since).cloned().collect())
}

/// The agent ledger is the *payer's* file, written by `x402z-agent` (spendguard state + audit
/// trail) and sealed at rest. A merchant has no business reading it, so the dashboard shows it
/// only when `X402Z_DEMO_AGENT_LEDGER` names it and `X402Z_DEMO_AGENT_LEDGER_KEY` holds the
/// agent's key (payer and merchant on one machine, for a demo). Unset => `hidden`; set but
/// unreadable, keyless or not openable => a red banner naming the problem.
fn read_ledger(path: Option<&str>, key: Option<&[u8; 32]>) -> serde_json::Value {
    let Some(path) = path else {
        return serde_json::json!({ "hidden": "merchant view: the payer's spendguard ledger is not shown (set X402Z_DEMO_AGENT_LEDGER for a one-machine demo)" });
    };
    let Some(key) = key else {
        return serde_json::json!({ "error": "Setup required: X402Z_DEMO_AGENT_LEDGER_KEY not configured; the agent ledger is encrypted at rest, give the dashboard the agent's X402Z_AGENT_LEDGER_KEY" });
    };
    let raw = match std::fs::read(path) {
        Err(e) => return serde_json::json!({ "error": format!("X402Z_DEMO_AGENT_LEDGER unreadable ({path}): {e}") }),
        Ok(raw) => raw,
    };
    let plain = match spendguard::sealed::open(key, spendguard::sealed::LEDGER_PURPOSE, &raw) {
        Err(e) => return serde_json::json!({ "error": format!("X402Z_DEMO_AGENT_LEDGER {path}: {e}") }),
        Ok(p) => p,
    };
    match serde_json::from_slice::<serde_json::Value>(&plain) {
        Err(e) => serde_json::json!({ "error": format!("X402Z_DEMO_AGENT_LEDGER is not valid JSON ({path}): {e}") }),
        Ok(v) => serde_json::json!({ "data": v }),
    }
}

/// TCP reachability of the facilitator, so the dashboard can say loudly when nobody can
/// confirm payments (otherwise a payer would send TAZ and wait for a receipt that never comes).
async fn probe_facilitator(url: &str) -> serde_json::Value {
    let rest = url.split("://").nth(1).unwrap_or(url);
    let authority = rest.split('/').next().unwrap_or(rest);
    let default_port = if url.starts_with("https://") { 443 } else { 80 };
    let addr = if authority.contains(':') { authority.to_string() } else { format!("{authority}:{default_port}") };
    match tokio::time::timeout(std::time::Duration::from_secs(2), tokio::net::TcpStream::connect(&addr)).await {
        Ok(Ok(_)) => serde_json::json!({ "reachable": true }),
        Ok(Err(e)) => serde_json::json!({ "reachable": false, "error": format!("facilitator unreachable at {url}: {e}. Payments can't be confirmed and no receipts will be issued.") }),
        Err(_) => serde_json::json!({ "reachable": false, "error": format!("facilitator at {url} timed out. Payments can't be confirmed and no receipts will be issued.") }),
    }
}

async fn status(State(app): State<Arc<App>>) -> Json<serde_json::Value> {
    let facilitator = probe_facilitator(&app.cfg.facilitator_url).await;
    let open = app.store.count_open(now()).map(serde_json::Value::from).unwrap_or_else(|e| serde_json::Value::String(e));
    Json(serde_json::json!({
        "network": "testnet",
        "merchantUa": app.cfg.merchant_ua,
        "priceZat": app.cfg.price_zat,
        "minConfirmations": app.cfg.min_confirmations,
        "facilitatorUrl": app.cfg.facilitator_url,
        "facilitator": facilitator,
        "receiptKey": hex::encode(app.cfg.receipt_key.as_bytes()),
        "openChallenges": open,
        "ledger": read_ledger(app.cfg.agent_ledger.as_deref(), app.cfg.agent_ledger_key.as_ref()),
    }))
}

fn parse_key(hex_key: &str) -> Result<VerifyingKey, String> {
    let raw = hex::decode(hex_key).map_err(|e| format!("not hex: {e}"))?;
    let arr: [u8; 32] = raw.try_into().map_err(|_| "must be 32 bytes".to_string())?;
    VerifyingKey::from_bytes(&arr).map_err(|e| e.to_string())
}

/// What the world may reach: the paid resource, the paste-txid redeem, tab issuance and the QR.
fn public_router(app: Arc<App>) -> Router {
    Router::new()
        .route(QUOTE_PATH, get(quote))
        .route("/api/redeem", axum::routing::post(redeem_api))
        .route(TAB_ISSUE_PATH, axum::routing::post(tab_issue))
        .route("/api/challenge/{nonce}/qr.svg", get(challenge_qr))
        .with_state(app)
}

/// The merchant's console: dashboard, feed and status, plus the public routes so the dashboard's
/// "Send request" and redeem work from the same origin.
fn admin_router(app: Arc<App>) -> Router {
    Router::new()
        .route("/", get(|| async { Html(DASHBOARD) }))
        .route("/api/events", get(events))
        .route("/api/status", get(status))
        .with_state(app.clone())
        .merge(public_router(app))
}

/// The `Host` values the admin console answers to: its bind address, and loopback names on its
/// port, plus `X402Z_ADMIN_HOSTS`. Anything else is a 403, so a web page the operator visits
/// can't read the feed or status through DNS rebinding (its requests carry the page's own host).
fn admin_hosts(bind: &str, extra: &[String]) -> Vec<String> {
    let mut hosts = vec![bind.to_ascii_lowercase()];
    if let Some((_, port)) = bind.rsplit_once(':') {
        for h in ["127.0.0.1", "[::1]", "localhost"] {
            hosts.push(format!("{h}:{port}"));
        }
    }
    hosts.extend(extra.iter().map(|h| h.trim().to_ascii_lowercase()).filter(|h| !h.is_empty()));
    hosts
}

async fn check_admin_host(
    State(allowed): State<Arc<Vec<String>>>,
    req: axum::extract::Request,
    next: axum::middleware::Next,
) -> Response {
    let host = req
        .headers()
        .get("host")
        .and_then(|v| v.to_str().ok())
        .map(str::to_string)
        .or_else(|| req.uri().authority().map(|a| a.to_string()))
        .unwrap_or_default()
        .to_ascii_lowercase();
    if allowed.iter().any(|h| *h == host) {
        return next.run(req).await;
    }
    let msg = format!(
        "admin console: Host '{}' is not this console's address; refused (DNS rebinding protection). Add it to X402Z_ADMIN_HOSTS if it is yours.",
        clip(host)
    );
    (StatusCode::FORBIDDEN, Json(serde_json::json!({ "error": msg }))).into_response()
}

/// [`admin_router`] behind the `Host` check of [`admin_hosts`].
fn admin_app(app: Arc<App>, bind: &str, extra_hosts: &[String]) -> Router {
    admin_router(app).layer(axum::middleware::from_fn_with_state(Arc::new(admin_hosts(bind, extra_hosts)), check_admin_host))
}

fn is_loopback_bind(bind: &str) -> bool {
    match bind.parse::<SocketAddr>() {
        Ok(a) => a.ip().is_loopback(),
        Err(_) => bind.starts_with("localhost:"),
    }
}

#[tokio::main]
async fn main() {
    let merchant_ua = require("X402Z_MERCHANT_UA");
    if !merchant_ua.starts_with("utest1") {
        eprintln!("error: X402Z_MERCHANT_UA must be a testnet unified address (utest1...)");
        exit(2);
    }
    let receipt_key = parse_key(&require("X402Z_RECEIPT_PUBKEY")).unwrap_or_else(|e| {
        eprintln!("error: X402Z_RECEIPT_PUBKEY invalid: {e}");
        exit(2);
    });
    let facilitator_url = require("X402Z_FACILITATOR_URL");
    let facilitator_token = optional("X402Z_FACILITATOR_TOKEN").unwrap_or_else(|| {
        eprintln!(
            "error: Setup required: X402Z_FACILITATOR_TOKEN not configured. The facilitator only settles for a server that \
             presents its shared token; set the same value on both (e.g. `openssl rand -hex 32`)."
        );
        exit(2);
    });
    let price_zat = match optional("X402Z_PRICE_ZAT").map(|s| s.parse::<u64>()) {
        None => DEFAULT_PRICE_ZAT,
        Some(Ok(p)) if p > 0 => p,
        Some(_) => {
            eprintln!("error: X402Z_PRICE_ZAT must be a positive integer (zatoshi)");
            exit(2);
        }
    };
    let bind = optional("X402Z_BIND").unwrap_or_else(|| "127.0.0.1:4020".into());
    let admin_bind = match optional("X402Z_ADMIN_BIND").unwrap_or_else(|| DEFAULT_ADMIN_BIND.into()) {
        b if b.eq_ignore_ascii_case("off") => None,
        b if b == bind => {
            eprintln!("error: X402Z_ADMIN_BIND must differ from X402Z_BIND: the dashboard, feed and status stay off the public port");
            exit(2);
        }
        b => Some(b),
    };
    let trusted_proxies: Vec<IpAddr> = match optional("X402Z_TRUSTED_PROXIES") {
        None => Vec::new(),
        Some(list) => list
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(|s| {
                s.parse().unwrap_or_else(|_| {
                    eprintln!("error: X402Z_TRUSTED_PROXIES must be a comma-separated list of IP addresses ('{s}' is not one)");
                    exit(2);
                })
            })
            .collect(),
    };
    let public_base_url = optional("X402Z_PUBLIC_BASE_URL").map(|raw| {
        parse_public_base_url(&raw).unwrap_or_else(|e| {
            eprintln!("error: X402Z_PUBLIC_BASE_URL {e}");
            exit(2);
        })
    });
    let min_confirmations = match optional("X402Z_MIN_CONFIRMATIONS").map(|s| s.parse::<u32>()) {
        None => 1,
        Some(Ok(n)) => n,
        Some(Err(_)) => {
            eprintln!("error: X402Z_MIN_CONFIRMATIONS must be a non-negative integer");
            exit(2);
        }
    };
    if min_confirmations == 0 {
        eprintln!("warning: X402Z_MIN_CONFIRMATIONS=0: mempool-only (unmined) payments will unlock content");
    }
    let nonneg = |var: &str, default: u64| match optional(var).map(|s| s.parse::<u64>()) {
        None => default,
        Some(Ok(n)) => n,
        Some(Err(_)) => {
            eprintln!("error: {var} must be a non-negative integer (seconds)");
            exit(2);
        }
    };
    let cfg = Config {
        merchant_ua,
        price_zat,
        receipt_key,
        facilitator_url,
        facilitator_token,
        agent_ledger: optional("X402Z_DEMO_AGENT_LEDGER"),
        agent_ledger_key: optional("X402Z_DEMO_AGENT_LEDGER_KEY").map(|k| {
            spendguard::sealed::parse_key_hex(&k).unwrap_or_else(|e| {
                eprintln!("error: X402Z_DEMO_AGENT_LEDGER_KEY {e}");
                exit(2);
            })
        }),
        min_confirmations,
        expiry_grace_secs: nonneg("X402Z_EXPIRY_GRACE_SECS", DEFAULT_EXPIRY_GRACE_SECS),
        redeem_window_secs: nonneg("X402Z_REDEEM_WINDOW_SECS", DEFAULT_REDEEM_WINDOW_SECS),
        resend_window_secs: nonneg("X402Z_RESEND_WINDOW_SECS", DEFAULT_RESEND_WINDOW_SECS),
        tab_max_tokens: u32::try_from(nonneg("X402Z_TAB_MAX_TOKENS", tab::DEFAULT_MAX_TOKENS as u64)).unwrap_or(u32::MAX),
        tab_epoch_secs: nonneg("X402Z_TAB_EPOCH_SECS", tab::DEFAULT_EPOCH_SECS).max(60),
        tab_grace_secs: nonneg("X402Z_TAB_GRACE_SECS", tab::DEFAULT_GRACE_SECS),
        trusted_proxies,
        public_base_url,
    };
    let db_path = PathBuf::from(optional("X402Z_SERVER_DB").unwrap_or_else(|| "x402z-server.sqlite".into()));
    let store = Store::open(&db_path).unwrap_or_else(|e| {
        eprintln!("error: {e}");
        exit(2);
    });
    eprintln!("challenge book: {} (X402Z_SERVER_DB)", db_path.display());
    match &cfg.agent_ledger {
        None => eprintln!("dashboard: merchant view, the agent's spendguard ledger is not shown (X402Z_DEMO_AGENT_LEDGER unset)"),
        Some(p) => eprintln!("warning: dashboard shows the agent's spendguard ledger {p} (X402Z_DEMO_AGENT_LEDGER): one-machine demo only, a real merchant never sees it"),
    }
    let rate = match optional("X402Z_CHALLENGE_RATE_PER_MIN").map(|s| s.parse::<u32>()) {
        None => DEFAULT_CHALLENGE_RATE_PER_MIN,
        Some(Ok(n)) => n,
        Some(Err(_)) => {
            eprintln!("error: X402Z_CHALLENGE_RATE_PER_MIN must be a non-negative integer (0 disables the limit)");
            exit(2);
        }
    };
    if rate == 0 {
        eprintln!("warning: X402Z_CHALLENGE_RATE_PER_MIN=0: 402 challenges are not rate limited");
    }
    let http = reqwest::Client::builder().timeout(Duration::from_secs(90)).build().unwrap_or_else(|e| {
        eprintln!("error: cannot build HTTP client: {e}");
        exit(1);
    });
    let app = Arc::new(App {
        cfg,
        store,
        http,
        tab: RwLock::new(None),
        book: Mutex::new(Book::default()),
        limiter: Mutex::new(ChallengeLimiter::new(rate)),
        last_prune: std::sync::atomic::AtomicU64::new(0),
    });
    match tab_key(&app) {
        Ok(Some(k)) => eprintln!("tab: key {} ({} tokens max per payment)", k.key_id, app.cfg.tab_max_tokens),
        Ok(None) => eprintln!("tab: off (X402Z_TAB_MAX_TOKENS=0)"),
        Err(e) => {
            eprintln!("error: {e}");
            exit(2);
        }
    }
    let bind_or_die = |addr: String| async move {
        tokio::net::TcpListener::bind(&addr).await.unwrap_or_else(|e| {
            eprintln!("error: cannot bind {addr}: {e}");
            exit(1);
        })
    };
    let listener = bind_or_die(bind.clone()).await;
    if let Some(admin) = admin_bind {
        if !is_loopback_bind(&admin) {
            eprintln!("warning: X402Z_ADMIN_BIND={admin} is not loopback: anyone who can reach it sees the payment feed and status");
        }
        let admin_listener = bind_or_die(admin.clone()).await;
        let extra: Vec<String> = optional("X402Z_ADMIN_HOSTS").map(|v| v.split(',').map(str::to_string).collect()).unwrap_or_default();
        let admin_app = admin_app(app.clone(), &admin, &extra).into_make_service_with_connect_info::<SocketAddr>();
        eprintln!("x402z-server dashboard (feed, status) on http://{admin} (X402Z_ADMIN_BIND)");
        tokio::spawn(async move {
            if let Err(e) = axum::serve(admin_listener, admin_app).await {
                eprintln!("error: admin listener stopped: {e}");
                exit(1);
            }
        });
    } else {
        eprintln!("x402z-server dashboard off (X402Z_ADMIN_BIND=off)");
    }
    eprintln!("x402z-server listening on http://{bind} (paid API only; X-PAYMENT attempts and 402 challenges limited to {rate}/min per client)");
    if let Err(e) = axum::serve(listener, public_router(app).into_make_service_with_connect_info::<SocketAddr>()).await {
        eprintln!("error: server stopped: {e}");
        exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signer, SigningKey};

    const UA: &str = "utest1merchantaddressfortestsonly0000";

    fn key() -> SigningKey {
        SigningKey::from_bytes(&[7u8; 32])
    }

    fn claims(nonce: &str, amount: u64) -> ReceiptClaims {
        ReceiptClaims {
            memo_nonce: nonce.into(),
            amount_zat: amount,
            pay_to: UA.into(),
            txid: "ab".repeat(32),
            confirmations: 1,
            issued_at: 1,
            mined_at: None,
        }
    }

    fn encode(sk: &SigningKey, c: &ReceiptClaims) -> String {
        let raw = serde_json::to_vec(c).unwrap();
        format!("{}.{}", URL_SAFE_NO_PAD.encode(&raw), URL_SAFE_NO_PAD.encode(sk.sign(&raw).to_bytes()))
    }

    const GRACE: u64 = 100;

    fn mem_store() -> Store {
        Store::open(std::path::Path::new(":memory:")).unwrap()
    }

    fn store_with(nonce: &str, amount: u64, expires_at: u64) -> Store {
        let s = mem_store();
        s.insert(&PaymentRequired {
            scheme: SCHEME.into(), network: Network::Testnet, pay_to: UA.into(),
            amount_zat: amount, memo_nonce: nonce.into(), expires_at,
        })
        .unwrap();
        s
    }

    fn tmp_db(name: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!("x402z-server-{}-{name}.sqlite", std::process::id()));
        for ext in ["", "-wal", "-shm"] {
            let _ = std::fs::remove_file(format!("{}{ext}", p.display()));
        }
        p
    }

    #[test]
    fn valid_receipt_settles_once() {
        let sk = key();
        let c = decode_receipt(&sk.verifying_key(), &encode(&sk, &claims("n1", 100))).unwrap();
        let s = store_with("n1", 100, 50);
        assert_eq!(settle(&s, UA, &c, 10, GRACE), Ok(()));
        assert_eq!(settle(&s, UA, &c, 10, GRACE), Err(ReceiptError::Replayed));
    }

    #[test]
    fn foreign_key_is_rejected() {
        let other = SigningKey::from_bytes(&[9u8; 32]);
        let r = decode_receipt(&key().verifying_key(), &encode(&other, &claims("n1", 100)));
        assert_eq!(r, Err(ReceiptError::BadSignature));
    }

    #[test]
    fn tampered_claims_are_rejected() {
        let sk = key();
        let good = encode(&sk, &claims("n1", 1));
        let sig = good.split_once('.').unwrap().1;
        let forged = format!("{}.{sig}", URL_SAFE_NO_PAD.encode(serde_json::to_vec(&claims("n1", 999)).unwrap()));
        assert_eq!(decode_receipt(&sk.verifying_key(), &forged), Err(ReceiptError::BadSignature));
    }

    #[test]
    fn wrong_amount_expired_and_unknown_are_rejected() {
        let s = store_with("n1", 100, 50);
        assert_eq!(settle(&s, UA, &claims("n1", 99), 10, GRACE), Err(ReceiptError::Underpaid { paid: 99, want: 100 }));
        // mempool-only receipt (no minedAt) redeemed after expiry + grace
        assert_eq!(settle(&s, UA, &claims("n1", 100), 151, GRACE), Err(ReceiptError::Expired { expires_at: 50, paid_at: 151 }));
        assert_eq!(settle(&s, UA, &claims("nope", 100), 10, GRACE), Err(ReceiptError::UnknownNonce));
        let mut wrong = claims("n1", 100);
        wrong.pay_to = "utest1someoneelse000000000000000".into();
        assert_eq!(settle(&s, UA, &wrong, 10, GRACE), Err(ReceiptError::WrongAddress));
        // none of the rejections burned the nonce
        assert_eq!(settle(&s, UA, &claims("n1", 100), 10, GRACE), Ok(()));
    }

    #[test]
    fn paid_before_expiry_redeemed_after_is_accepted_by_mined_time() {
        // Challenge expired at t=50. The payment was mined at t=45 (before expiry); the payer
        // only redeems at t=10_000, long after. Mined time decides, so it unlocks.
        let s = store_with("n1", 100, 50);
        let mut c = claims("n1", 100);
        c.mined_at = Some(45);
        assert_eq!(settle(&s, UA, &c, 10_000, GRACE), Ok(()));

        // Mined inside the grace window (slow block): still accepted.
        let s = store_with("n2", 100, 50);
        let mut c = claims("n2", 100);
        c.mined_at = Some(50 + GRACE);
        assert_eq!(settle(&s, UA, &c, 10_000, GRACE), Ok(()));

        // Mined after expiry + grace: refused, and the reason says when it landed.
        let s = store_with("n3", 100, 50);
        let mut c = claims("n3", 100);
        c.mined_at = Some(51 + GRACE);
        assert_eq!(settle(&s, UA, &c, 60, GRACE), Err(ReceiptError::Expired { expires_at: 50, paid_at: 51 + GRACE }));
    }

    #[test]
    fn redeemed_nonce_stays_burned_across_restart() {
        let path = tmp_db("burn");
        {
            let s = Store::open(&path).unwrap();
            s.insert(&PaymentRequired {
                scheme: SCHEME.into(), network: Network::Testnet, pay_to: UA.into(),
                amount_zat: 100, memo_nonce: "n1".into(), expires_at: 50,
            })
            .unwrap();
            assert_eq!(settle(&s, UA, &claims("n1", 100), 10, GRACE), Ok(()));
        }
        let s = Store::open(&path).unwrap();
        assert_eq!(settle(&s, UA, &claims("n1", 100), 10, GRACE), Err(ReceiptError::Replayed));
        assert_eq!(s.get("n1").unwrap().unwrap().spent_txid.as_deref(), Some("ab".repeat(32).as_str()));
    }

    #[test]
    fn prune_keeps_spent_rows_and_live_challenges() {
        let s = mem_store();
        for (n, exp) in [("old", 10u64), ("live", 1_000), ("spent", 10)] {
            s.insert(&PaymentRequired {
                scheme: SCHEME.into(), network: Network::Testnet, pay_to: UA.into(),
                amount_zat: 1, memo_nonce: n.into(), expires_at: exp,
            })
            .unwrap();
        }
        assert!(s.mark_spent("spent", "tx", 5).unwrap());
        assert!(!s.mark_spent("spent", "tx2", 6).unwrap());
        assert_eq!(s.prune_unspent_expired_before(500).unwrap(), 1);
        assert!(s.get("old").unwrap().is_none());
        assert!(s.get("live").unwrap().is_some());
        assert!(s.get("spent").unwrap().is_some());
        assert_eq!(s.count_open(500).unwrap(), 1);
    }

    fn ip(last: u8) -> IpAddr {
        IpAddr::from([10, 0, 0, last])
    }

    #[test]
    fn limiter_allows_burst_then_refuses_with_retry_after() {
        let mut l = ChallengeLimiter::new(3);
        let t0 = Instant::now();
        for _ in 0..3 {
            assert_eq!(l.try_acquire(ip(1), t0), Ok(()));
        }
        let wait = l.try_acquire(ip(1), t0).unwrap_err();
        // 3/min refills one token every 20 s
        assert!(wait > Duration::from_secs(19) && wait <= Duration::from_secs(20), "{wait:?}");
        // another client is unaffected
        assert_eq!(l.try_acquire(ip(2), t0), Ok(()));
    }

    #[test]
    fn limiter_refills_over_time_and_caps_at_burst() {
        let mut l = ChallengeLimiter::new(60); // 1 token per second
        let t0 = Instant::now();
        for _ in 0..60 {
            l.try_acquire(ip(1), t0).unwrap();
        }
        assert!(l.try_acquire(ip(1), t0).is_err());
        assert!(l.try_acquire(ip(1), t0 + Duration::from_millis(500)).is_err());
        assert_eq!(l.try_acquire(ip(1), t0 + Duration::from_secs(2)), Ok(()));
        // a long idle period refills to the burst size, not beyond
        let later = t0 + Duration::from_secs(3600);
        for _ in 0..60 {
            l.try_acquire(ip(1), later).unwrap();
        }
        assert!(l.try_acquire(ip(1), later).is_err());
    }

    #[test]
    fn limiter_zero_means_disabled() {
        let mut l = ChallengeLimiter::new(0);
        let t0 = Instant::now();
        for _ in 0..1000 {
            assert_eq!(l.try_acquire(ip(1), t0), Ok(()));
        }
        assert!(l.buckets.is_empty());
    }

    fn test_app_at(rate: u32, db: &std::path::Path) -> Arc<App> {
        let cfg = Config {
            merchant_ua: UA.into(),
            price_zat: 100_000,
            receipt_key: key().verifying_key(),
            facilitator_url: "http://127.0.0.1:9".into(),
            facilitator_token: STAND_IN_TOKEN.into(),
            agent_ledger: None,
            agent_ledger_key: None,
            min_confirmations: 1,
            expiry_grace_secs: GRACE,
            redeem_window_secs: DEFAULT_REDEEM_WINDOW_SECS,
            resend_window_secs: DEFAULT_RESEND_WINDOW_SECS,
            tab_max_tokens: 5,
            tab_epoch_secs: tab::DEFAULT_EPOCH_SECS,
            // No grace by default here: the epoch tests assert a retired key is dead at once;
            // the grace period has its own test.
            tab_grace_secs: 0,
            trusted_proxies: Vec::new(),
            public_base_url: None,
        };
        let store = Store::open(db).unwrap();
        // One RSA key for the whole test binary (generation is slow); a store that already
        // has an active key (restart tests) keeps it.
        if store.active_tab_key().unwrap().is_none() {
            tab::install(&store, test_tab_key_der(), now()).unwrap();
        }
        let http = reqwest::Client::new();
        Arc::new(App {
            cfg,
            store,
            http,
            tab: RwLock::new(None),
            book: Mutex::new(Book::default()),
            limiter: Mutex::new(ChallengeLimiter::new(rate)),
            last_prune: std::sync::atomic::AtomicU64::new(0),
        })
    }

    fn test_tab_key_der() -> &'static [u8] {
        static DER: std::sync::OnceLock<Vec<u8>> = std::sync::OnceLock::new();
        DER.get_or_init(|| {
            let kp = blind_rsa_signatures::KeyPair::<blind_rsa_signatures::Sha384, blind_rsa_signatures::PSS, blind_rsa_signatures::Randomized>::generate(
                &mut blind_rsa_signatures::DefaultRng,
                2048,
            )
            .unwrap();
            kp.sk.to_der().unwrap()
        })
    }

    fn test_app(rate: u32) -> Arc<App> {
        test_app_at(rate, std::path::Path::new(":memory:"))
    }

    fn open_nonces(app: &App) -> Vec<String> {
        let conn = app.store.conn_for_tests();
        let mut q = conn.prepare("SELECT nonce FROM challenges WHERE spent_txid IS NULL").unwrap();
        q.query_map([], |r| r.get(0)).unwrap().map(Result::unwrap).collect()
    }

    async fn get_quote(app: Arc<App>, from: [u8; 4], receipt: Option<&str>) -> Response {
        match receipt {
            Some(r) => get_quote_with(app, from, &[(RECEIPT_HEADER, r)]).await,
            None => get_quote_with(app, from, &[]).await,
        }
    }

    async fn get_quote_with(app: Arc<App>, from: [u8; 4], headers: &[(&str, &str)]) -> Response {
        use axum::{body::Body, extract::connect_info::MockConnectInfo, http::Request};
        use tower::ServiceExt;
        let mut req = Request::get("/api/quote").header("host", "kage.test");
        for (k, v) in headers {
            req = req.header(*k, *v);
        }
        admin_router(app)
            .layer(MockConnectInfo(SocketAddr::from((from, 40000))))
            .oneshot(req.body(Body::empty()).unwrap())
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn quote_returns_429_with_retry_after_once_the_ip_budget_is_spent() {
        let app = test_app(2);
        assert_eq!(get_quote(app.clone(), [10, 0, 0, 1], None).await.status(), StatusCode::PAYMENT_REQUIRED);
        assert_eq!(get_quote(app.clone(), [10, 0, 0, 1], None).await.status(), StatusCode::PAYMENT_REQUIRED);
        let r = get_quote(app.clone(), [10, 0, 0, 1], None).await;
        assert_eq!(r.status(), StatusCode::TOO_MANY_REQUESTS);
        let retry: u64 = r.headers().get("retry-after").unwrap().to_str().unwrap().parse().unwrap();
        assert!((1..=30).contains(&retry), "retry-after {retry}");
        // the refused request minted no challenge
        assert_eq!(app.store.count_open(now()).unwrap(), 2);
        // a different client still gets a challenge
        assert_eq!(get_quote(app.clone(), [10, 0, 0, 2], None).await.status(), StatusCode::PAYMENT_REQUIRED);
    }

    #[tokio::test]
    async fn rejected_receipt_reissue_is_rate_limited_too() {
        let app = test_app(1);
        assert_eq!(get_quote(app.clone(), [10, 0, 0, 1], None).await.status(), StatusCode::PAYMENT_REQUIRED);
        let forged = encode(&SigningKey::from_bytes(&[9u8; 32]), &claims("n1", 100_000));
        let r = get_quote(app.clone(), [10, 0, 0, 1], Some(&forged)).await;
        assert_eq!(r.status(), StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(r.headers().get("x-payment-error").unwrap(), "receipt signature invalid");
        assert_eq!(app.store.count_open(now()).unwrap(), 1);
    }

    #[tokio::test]
    async fn valid_receipt_is_not_rate_limited() {
        let app = test_app(1);
        let r = get_quote(app.clone(), [10, 0, 0, 1], None).await;
        assert_eq!(r.status(), StatusCode::PAYMENT_REQUIRED);
        let nonce = open_nonces(&app).remove(0);
        let receipt = encode(&key(), &claims(&nonce, 100_000));
        // budget is spent, but redeeming a receipt mints no challenge
        assert_eq!(get_quote(app.clone(), [10, 0, 0, 1], Some(&receipt)).await.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn expired_direct_receipt_reports_payment_expired_without_burning_nonce() {
        let app = test_app(0);
        let ch = PaymentRequired {
            scheme: SCHEME.into(), network: Network::Testnet, pay_to: UA.into(), amount_zat: 100_000,
            memo_nonce: "x402z-expired-receipt".into(), expires_at: now() - GRACE - 2,
        };
        app.store.insert(&ch).unwrap();
        let mut c = claims(&ch.memo_nonce, ch.amount_zat);
        c.mined_at = Some(ch.expires_at + GRACE + 1);
        let response = get_quote(app.clone(), [10, 0, 0, 1], Some(&encode(&key(), &c))).await;
        assert_eq!(response.status(), StatusCode::PAYMENT_REQUIRED);
        assert_eq!(settlement_of(&response).error_reason.as_deref(), Some(reason::EXPIRED));
        assert!(app.store.get(&ch.memo_nonce).unwrap().unwrap().spent_txid.is_none());
    }

    #[tokio::test]
    async fn restart_between_challenge_and_redeem_keeps_the_challenge() {
        let path = tmp_db("restart");
        let app = test_app_at(5, &path);
        assert_eq!(get_quote(app.clone(), [10, 0, 0, 1], None).await.status(), StatusCode::PAYMENT_REQUIRED);
        let nonce = open_nonces(&app).remove(0);
        drop(app); // server stops after issuing the 402

        let app = test_app_at(5, &path); // ...and comes back on the same database
        let receipt = encode(&key(), &claims(&nonce, 100_000));
        assert_eq!(get_quote(app.clone(), [10, 0, 0, 1], Some(&receipt)).await.status(), StatusCode::OK);
        drop(app);

        // and after another restart the same receipt is a replay
        let app = test_app_at(5, &path);
        let r = get_quote(app.clone(), [10, 0, 0, 1], Some(&receipt)).await;
        assert_eq!(r.status(), StatusCode::PAYMENT_REQUIRED);
        assert_eq!(r.headers().get("x-payment-error").unwrap(), "receipt already used");
    }

    // -- x402 X-PAYMENT ------------------------------------------------------------------------
    //
    // The server's only external dependency here is our own facilitator's `/settle`. These tests
    // run a small in-process HTTP stand-in for it on 127.0.0.1 that answers in the facilitator's
    // wire format and signs receipts with the test key; the facilitator's real chain checks are
    // tested in x402z-facilitator (real transactions) and live against testnet.

    const TX_OK: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const TX_PENDING: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
    const TX_UNDERPAID: &str = "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc";
    const TX_FOREIGN_KEY: &str = "dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd";
    /// Pays three times the challenge price (a tab of up to 3 tokens).
    const TX_PAID_3X: &str = "3333333333333333333333333333333333333333333333333333333333333333";

    /// The facilitator token the test servers send and the stand-ins require.
    const STAND_IN_TOKEN: &str = "stand-in-facilitator-token-0123456789";

    fn bearer_ok(headers: &HeaderMap) -> bool {
        headers.get("authorization").and_then(|v| v.to_str().ok()) == Some(&format!("Bearer {STAND_IN_TOKEN}"))
    }

    async fn stand_in_settle(headers: HeaderMap, Json(req): Json<FacilitatorRequest>) -> Response {
        if !bearer_ok(&headers) {
            return (StatusCode::UNAUTHORIZED, Json(serde_json::json!({ "error": "no token" }))).into_response();
        }
        let pay = req.payment_payload.zcash_payload().unwrap();
        let ch = req.payment_requirements.to_challenge().unwrap();
        let claims = ReceiptClaims {
            memo_nonce: pay.memo_nonce.clone(),
            amount_zat: if pay.txid == TX_PAID_3X { 3 * ch.amount_zat } else { ch.amount_zat },
            pay_to: ch.pay_to.clone(),
            txid: pay.txid.clone(),
            confirmations: 2,
            issued_at: now(),
            mined_at: Some(now() - 30),
        };
        let confirmed = |sk: &SigningKey| SettleResponse {
            settlement: SettlementResponse::ok(&pay.txid),
            status: VerifyStatus::Confirmed,
            receipt: Some(encode(sk, &claims)),
            claims: Some(claims.clone()),
            confirmations: Some(2),
            min_confirmations: Some(1),
            error: None,
        };
        let failed = |status, why: &str, msg: &str| SettleResponse {
            settlement: SettlementResponse::failed(why),
            status,
            receipt: None,
            claims: None,
            confirmations: None,
            min_confirmations: None,
            error: Some(msg.into()),
        };
        let r = match pay.txid.as_str() {
            TX_OK | TX_PAID_3X => confirmed(&key()),
            TX_FOREIGN_KEY => confirmed(&SigningKey::from_bytes(&[9u8; 32])),
            TX_PENDING => failed(VerifyStatus::Pending, reason::PENDING, "payment found (100000 zat) with 0 confirmation(s); need 1"),
            TX_UNDERPAID => failed(VerifyStatus::Rejected, reason::INVALID_PAYMENT, "underpaid: 99999 zat < 100000 zat"),
            _ => failed(VerifyStatus::NotFound, reason::NOT_FOUND, "lightwalletd does not know tx"),
        };
        Json(r).into_response()
    }

    async fn app_with_stand_in() -> Arc<App> {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, Router::new().route(SETTLE_PATH, axum::routing::post(stand_in_settle))).await.unwrap();
        });
        let app = test_app(100);
        // test_app's facilitator URL points nowhere; swap in the stand-in.
        let mut cfg_app = Arc::try_unwrap(app).ok().unwrap();
        cfg_app.cfg.facilitator_url = format!("http://{addr}");
        Arc::new(cfg_app)
    }

    async fn body_json(r: Response) -> serde_json::Value {
        let bytes = axum::body::to_bytes(r.into_body(), 1 << 20).await.unwrap();
        serde_json::from_slice(&bytes).unwrap()
    }

    fn x_payment_header(txid: &str, nonce: &str) -> String {
        encode_header(&PaymentPayload::zcash(txid, nonce))
    }

    fn settlement_of(r: &Response) -> SettlementResponse {
        decode_header(r.headers().get(PAYMENT_RESPONSE_HEADER).unwrap().to_str().unwrap()).unwrap()
    }

    #[tokio::test]
    async fn the_402_body_is_x402_v1_payment_required_response() {
        let app = test_app(5);
        let r = get_quote(app.clone(), [10, 0, 0, 1], None).await;
        assert_eq!(r.status(), StatusCode::PAYMENT_REQUIRED);
        let v = body_json(r).await;
        assert_eq!(v["x402Version"], 1);
        assert_eq!(v["error"], "X-PAYMENT header is required");
        let a = &v["accepts"][0];
        assert_eq!(a["scheme"], "zcash-shielded");
        assert_eq!(a["network"], "zcash-testnet");
        assert_eq!(a["maxAmountRequired"], "100000");
        assert_eq!(a["resource"], "http://kage.test/api/quote");
        assert_eq!(a["payTo"], UA);
        assert_eq!(a["asset"], "ZEC");
        assert_eq!(a["mimeType"], "application/json");
        assert_eq!(a["maxTimeoutSeconds"], CHALLENGE_TTL_SECS);
        assert!(a["description"].as_str().unwrap().contains("shielded"));
        let parsed: PaymentRequiredResponse = serde_json::from_value(v.clone()).unwrap();
        let ch = parsed.zcash_option().unwrap().to_challenge().unwrap();
        assert_eq!(open_nonces(&app), vec![ch.memo_nonce]);
    }

    #[tokio::test]
    async fn x_payment_settles_once_and_returns_x_payment_response() {
        let app = app_with_stand_in().await;
        let nonce = {
            let v = body_json(get_quote(app.clone(), [10, 0, 0, 1], None).await).await;
            v["accepts"][0]["extra"]["memoNonce"].as_str().unwrap().to_string()
        };
        let h = x_payment_header(TX_OK, &nonce);
        let r = get_quote_with(app.clone(), [10, 0, 0, 1], &[(PAYMENT_HEADER, &h)]).await;
        assert_eq!(r.status(), StatusCode::OK);
        let s = settlement_of(&r);
        assert_eq!(s, SettlementResponse::ok(TX_OK));
        let v = body_json(r).await;
        assert_eq!(v["settlement"]["txid"], TX_OK);
        assert_eq!(v["settlement"]["memoNonce"], nonce);
        assert_eq!(app.store.get(&nonce).unwrap().unwrap().spent_txid.as_deref(), Some(TX_OK));

        // another txid on the spent nonce: refused with nonce_already_used and a fresh challenge
        let other = x_payment_header(TX_PAID_3X, &nonce);
        let r = get_quote_with(app.clone(), [10, 0, 0, 1], &[(PAYMENT_HEADER, &other)]).await;
        assert_eq!(r.status(), StatusCode::PAYMENT_REQUIRED);
        assert_eq!(settlement_of(&r).error_reason.as_deref(), Some(reason::NONCE_USED));
        let v = body_json(r).await;
        assert_ne!(v["accepts"][0]["extra"]["memoNonce"], nonce);
        assert_eq!(v["error"], "receipt already used");

        // the same X-PAYMENT once the resend window is over: refused the same way
        app.store.conn_for_tests().execute("UPDATE challenges SET spent_at = spent_at - ?1", [DEFAULT_RESEND_WINDOW_SECS as i64 + 1]).unwrap();
        let r = get_quote_with(app.clone(), [10, 0, 0, 1], &[(PAYMENT_HEADER, &h)]).await;
        assert_eq!(r.status(), StatusCode::PAYMENT_REQUIRED);
        assert_eq!(settlement_of(&r).error_reason.as_deref(), Some(reason::NONCE_USED));
        let v = body_json(r).await;
        assert_ne!(v["accepts"][0]["extra"]["memoNonce"], nonce);
        assert_eq!(v["error"], "receipt already used");
    }

    #[tokio::test]
    async fn a_lost_200_is_answered_again_for_the_same_payment_without_a_second_payment() {
        let app = app_with_stand_in().await;
        let nonce = {
            let v = body_json(get_quote(app.clone(), [10, 0, 0, 1], None).await).await;
            v["accepts"][0]["extra"]["memoNonce"].as_str().unwrap().to_string()
        };
        let h = x_payment_header(TX_OK, &nonce);
        // first X-PAYMENT settles; its response is "lost" (dropped unread)
        let first = get_quote_with(app.clone(), [10, 0, 0, 1], &[(PAYMENT_HEADER, &h)]).await;
        assert_eq!(first.status(), StatusCode::OK);
        let spent_at = app.store.get(&nonce).unwrap().unwrap().spent_at;
        drop(first);
        // the payer presents the same pair again: 200, the same settlement, nothing new burned
        let again = get_quote_with(app.clone(), [10, 0, 0, 1], &[(PAYMENT_HEADER, &h)]).await;
        assert_eq!(again.status(), StatusCode::OK);
        assert_eq!(settlement_of(&again), SettlementResponse::ok(TX_OK));
        let v = body_json(again).await;
        assert_eq!(v["settlement"]["txid"], TX_OK);
        assert_eq!(v["settlement"]["memoNonce"], nonce);
        let row = app.store.get(&nonce).unwrap().unwrap();
        assert_eq!((row.spent_txid.as_deref(), row.spent_at), (Some(TX_OK), spent_at), "the spent row is unchanged");
        assert!(feed(&app).iter().any(|e| e.kind == "paid" && e.detail.starts_with("served again")));
        // the paste-txid path still refuses a second redeem of the nonce
        let r = post_redeem(app.clone(), serde_json::json!({ "memoNonce": nonce, "txid": TX_OK })).await;
        assert_eq!(r.status(), StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(body_json(r).await["reason"], reason::NONCE_USED);
    }

    #[tokio::test]
    async fn with_the_resend_window_off_a_replayed_x_payment_is_refused() {
        let app = app_with_stand_in().await;
        let mut a = Arc::try_unwrap(app).ok().unwrap();
        a.cfg.resend_window_secs = 0;
        let app = Arc::new(a);
        let nonce = {
            let v = body_json(get_quote(app.clone(), [10, 0, 0, 1], None).await).await;
            v["accepts"][0]["extra"]["memoNonce"].as_str().unwrap().to_string()
        };
        let h = x_payment_header(TX_OK, &nonce);
        assert_eq!(get_quote_with(app.clone(), [10, 0, 0, 1], &[(PAYMENT_HEADER, &h)]).await.status(), StatusCode::OK);
        // replaying the same X-PAYMENT: refused with nonce_already_used and a fresh challenge
        let r = get_quote_with(app.clone(), [10, 0, 0, 1], &[(PAYMENT_HEADER, &h)]).await;
        assert_eq!(r.status(), StatusCode::PAYMENT_REQUIRED);
        assert_eq!(settlement_of(&r).error_reason.as_deref(), Some(reason::NONCE_USED));
        let v = body_json(r).await;
        assert_ne!(v["accepts"][0]["extra"]["memoNonce"], nonce);
        assert_eq!(v["error"], "receipt already used");
    }

    #[tokio::test]
    async fn pending_payment_keeps_the_same_challenge_and_burns_nothing() {
        let app = app_with_stand_in().await;
        let v = body_json(get_quote(app.clone(), [10, 0, 0, 1], None).await).await;
        let nonce = v["accepts"][0]["extra"]["memoNonce"].as_str().unwrap().to_string();
        let r = get_quote_with(app.clone(), [10, 0, 0, 1], &[(PAYMENT_HEADER, &x_payment_header(TX_PENDING, &nonce))]).await;
        assert_eq!(r.status(), StatusCode::PAYMENT_REQUIRED);
        assert_eq!(settlement_of(&r).error_reason.as_deref(), Some(reason::PENDING));
        let v = body_json(r).await;
        assert_eq!(v["accepts"][0]["extra"]["memoNonce"], nonce, "same challenge, so the payer retries the same tx");
        assert!(v["error"].as_str().unwrap().contains("need 1"));
        assert_eq!(open_nonces(&app), vec![nonce.clone()]);
        // once it confirms, the same nonce redeems
        let r = get_quote_with(app.clone(), [10, 0, 0, 1], &[(PAYMENT_HEADER, &x_payment_header(TX_OK, &nonce))]).await;
        assert_eq!(r.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn rejected_forged_unknown_and_malformed_x_payments() {
        let app = app_with_stand_in().await;
        let v = body_json(get_quote(app.clone(), [10, 0, 0, 1], None).await).await;
        let nonce = v["accepts"][0]["extra"]["memoNonce"].as_str().unwrap().to_string();

        // facilitator says underpaid
        let r = get_quote_with(app.clone(), [10, 0, 0, 1], &[(PAYMENT_HEADER, &x_payment_header(TX_UNDERPAID, &nonce))]).await;
        assert_eq!(r.status(), StatusCode::PAYMENT_REQUIRED);
        assert_eq!(settlement_of(&r).error_reason.as_deref(), Some(reason::INVALID_PAYMENT));
        assert!(body_json(r).await["error"].as_str().unwrap().contains("underpaid"));

        // a "confirmed" answer signed by a key the server doesn't trust
        let r = get_quote_with(app.clone(), [10, 0, 0, 1], &[(PAYMENT_HEADER, &x_payment_header(TX_FOREIGN_KEY, &nonce))]).await;
        assert_eq!(r.status(), StatusCode::PAYMENT_REQUIRED);
        assert_eq!(body_json(r).await["error"], "receipt signature invalid");

        // a nonce this server never issued
        let r = get_quote_with(app.clone(), [10, 0, 0, 1], &[(PAYMENT_HEADER, &x_payment_header(TX_OK, "x402z-never-issued"))]).await;
        assert_eq!(settlement_of(&r).error_reason.as_deref(), Some(reason::UNKNOWN_CHALLENGE));

        // not base64 / wrong network
        let r = get_quote_with(app.clone(), [10, 0, 0, 1], &[(PAYMENT_HEADER, "%%%")]).await;
        assert_eq!(settlement_of(&r).error_reason.as_deref(), Some(reason::INVALID_PAYLOAD));
        let mut p = PaymentPayload::zcash(TX_OK, &nonce);
        p.network = "base-sepolia".into();
        let r = get_quote_with(app.clone(), [10, 0, 0, 1], &[(PAYMENT_HEADER, &encode_header(&p))]).await;
        assert_eq!(settlement_of(&r).error_reason.as_deref(), Some(reason::INVALID_NETWORK));

        // none of that burned the real nonce
        assert!(app.store.get(&nonce).unwrap().unwrap().spent_txid.is_none());
        let r = get_quote_with(app.clone(), [10, 0, 0, 1], &[(PAYMENT_HEADER, &x_payment_header(TX_OK, &nonce))]).await;
        assert_eq!(r.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn facilitator_down_is_a_502_and_burns_nothing() {
        let app = test_app(5); // facilitator URL points at a closed port
        let v = body_json(get_quote(app.clone(), [10, 0, 0, 1], None).await).await;
        let nonce = v["accepts"][0]["extra"]["memoNonce"].as_str().unwrap().to_string();
        let r = get_quote_with(app.clone(), [10, 0, 0, 1], &[(PAYMENT_HEADER, &x_payment_header(TX_OK, &nonce))]).await;
        assert_eq!(r.status(), StatusCode::BAD_GATEWAY);
        assert_eq!(settlement_of(&r).error_reason.as_deref(), Some(reason::FACILITATOR_ERROR));
        assert!(body_json(r).await["error"].as_str().unwrap().contains("facilitator unreachable"));
        assert_eq!(open_nonces(&app), vec![nonce]);
    }

    // -- any wallet: ZIP 321 + QR + paste txid -------------------------------------------------

    async fn call(app: Arc<App>, req: axum::http::Request<axum::body::Body>) -> Response {
        use axum::extract::connect_info::MockConnectInfo;
        use tower::ServiceExt;
        admin_router(app).layer(MockConnectInfo(SocketAddr::from(([10, 0, 0, 1], 40000)))).oneshot(req).await.unwrap()
    }

    async fn post_redeem(app: Arc<App>, body: serde_json::Value) -> Response {
        let req = axum::http::Request::post("/api/redeem")
            .header("content-type", "application/json")
            .header("host", "kage.test")
            .body(axum::body::Body::from(body.to_string()))
            .unwrap();
        call(app, req).await
    }

    #[tokio::test]
    async fn the_402_carries_a_zip321_uri_and_a_local_qr() {
        let app = test_app(5);
        let v = body_json(get_quote(app.clone(), [10, 0, 0, 1], None).await).await;
        let extra = &v["accepts"][0]["extra"];
        let nonce = extra["memoNonce"].as_str().unwrap();
        let memo_b64 = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(nonce);
        assert_eq!(extra["paymentUri"], format!("zcash:{UA}?amount=0.001&memo={memo_b64}"));

        let r = call(app.clone(), axum::http::Request::get(format!("/api/challenge/{nonce}/qr.svg")).body(axum::body::Body::empty()).unwrap()).await;
        assert_eq!(r.status(), StatusCode::OK);
        assert_eq!(r.headers().get("content-type").unwrap(), "image/svg+xml");
        let svg = axum::body::to_bytes(r.into_body(), 1 << 20).await.unwrap();
        assert!(svg.starts_with(b"<?xml") || svg.starts_with(b"<svg"), "{:?}", &svg[..20]);
        let r = call(app, axum::http::Request::get("/api/challenge/x402z-nope/qr.svg").body(axum::body::Body::empty()).unwrap()).await;
        assert_eq!(r.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn redeem_api_settles_a_pasted_txid() {
        let app = app_with_stand_in().await;
        let v = body_json(get_quote(app.clone(), [10, 0, 0, 1], None).await).await;
        let nonce = v["accepts"][0]["extra"]["memoNonce"].as_str().unwrap().to_string();

        let r = post_redeem(app.clone(), serde_json::json!({"memoNonce": nonce, "txid": TX_PENDING})).await;
        assert_eq!(r.status(), StatusCode::ACCEPTED);
        assert_eq!(body_json(r).await["status"], "pending");

        let r = post_redeem(app.clone(), serde_json::json!({"memoNonce": nonce, "txid": TX_UNDERPAID})).await;
        assert_eq!(r.status(), StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(body_json(r).await["reason"], reason::INVALID_PAYMENT);

        // uppercase / padded txid from a wallet's copy button is normalised
        let r = post_redeem(app.clone(), serde_json::json!({"memoNonce": nonce, "txid": format!(" {} ", TX_OK.to_uppercase())})).await;
        assert_eq!(r.status(), StatusCode::OK);
        let v = body_json(r).await;
        assert_eq!(v["status"], "paid");
        assert_eq!(v["settlement"]["transaction"], TX_OK);
        assert_eq!(v["content"]["settlement"]["memoNonce"], nonce);

        let r = post_redeem(app.clone(), serde_json::json!({"memoNonce": nonce, "txid": TX_OK})).await;
        assert_eq!(r.status(), StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(body_json(r).await["reason"], reason::NONCE_USED);

        let r = post_redeem(app.clone(), serde_json::json!({"memoNonce": nonce, "txid": "zz"})).await;
        assert_eq!(r.status(), StatusCode::BAD_REQUEST);
        let r = post_redeem(app.clone(), serde_json::json!({"txid": TX_OK})).await;
        assert_eq!(r.status(), StatusCode::BAD_REQUEST);
    }

    // -- Kage Tab: attack tests -------------------------------------------------------------------

    use blind_rsa_signatures::{BlindSignature, BlindingResult, DefaultRng};
    use x402z_protocol::{b64, b64_any, TabOffer, TabPayload};

    /// What a tab client does: blind n fresh random messages for the offered key.
    fn blind_n(offer: &TabOffer, n: usize) -> (tab::Pk, Vec<(Vec<u8>, BlindingResult)>) {
        let pk = tab::Pk::from_spki(&b64_any(&offer.public_key).unwrap()).unwrap();
        let toks = (0..n)
            .map(|_| {
                let mut msg = vec![0u8; 32];
                rand::thread_rng().fill_bytes(&mut msg);
                let br = pk.blind(&mut DefaultRng, &msg).unwrap();
                (msg, br)
            })
            .collect();
        (pk, toks)
    }

    async fn tab_offer_and_nonce(app: &Arc<App>) -> (TabOffer, String) {
        let v = body_json(get_quote(app.clone(), [10, 0, 0, 1], None).await).await;
        let extra = &v["accepts"][0]["extra"];
        (serde_json::from_value(extra["tab"].clone()).unwrap(), extra["memoNonce"].as_str().unwrap().to_string())
    }

    async fn issue(app: &Arc<App>, nonce: &str, txid: &str, key_id: &str, toks: &[(Vec<u8>, BlindingResult)]) -> Response {
        let body = serde_json::json!({
            "memoNonce": nonce, "txid": txid, "keyId": key_id,
            "blinded": toks.iter().map(|(_, br)| b64(&br.blind_message)).collect::<Vec<_>>(),
        });
        let req = axum::http::Request::post("/tab/issue")
            .header("content-type", "application/json")
            .header("host", "kage.test")
            .body(axum::body::Body::from(body.to_string()))
            .unwrap();
        call(app.clone(), req).await
    }

    /// Unblind the server's answer into spendable tokens.
    async fn finalize(pk: &tab::Pk, toks: &[(Vec<u8>, BlindingResult)], r: Response) -> Vec<TabPayload> {
        assert_eq!(r.status(), StatusCode::OK);
        let v = body_json(r).await;
        let key_id = v["keyId"].as_str().unwrap().to_string();
        let sigs = v["blindSignatures"].as_array().unwrap();
        assert_eq!(sigs.len(), toks.len());
        toks.iter()
            .zip(sigs)
            .map(|((msg, br), s)| {
                let bs = BlindSignature(b64_any(s.as_str().unwrap()).unwrap());
                let sig = pk.finalize(&bs, br, msg).unwrap(); // also verifies against the public key
                TabPayload { key_id: key_id.clone(), msg: b64(msg), randomizer: b64(&br.msg_randomizer.unwrap().0), sig: b64(&sig) }
            })
            .collect()
    }

    async fn spend_token(app: &Arc<App>, t: &TabPayload) -> Response {
        get_quote_with(app.clone(), [10, 0, 0, 1], &[(PAYMENT_HEADER, &encode_header(&PaymentPayload::tab(t)))]).await
    }

    #[tokio::test]
    async fn one_payment_buys_n_unlinkable_tokens_each_spendable_once() {
        let app = app_with_stand_in().await;
        let (offer, nonce) = tab_offer_and_nonce(&app).await;
        assert_eq!((offer.scheme.as_str(), offer.suite.as_str(), offer.unit_price_zat, offer.max_tokens), ("zcash-shielded-tab", "RSABSSA-SHA384-PSS-Randomized", 100_000, 5));
        let (pk, toks) = blind_n(&offer, 3);
        let tokens = finalize(&pk, &toks, issue(&app, &nonce, TX_PAID_3X, &offer.key_id, &toks).await).await;
        // the server only ever saw blinded messages, never the messages it later accepts
        for ((msg, br), t) in toks.iter().zip(&tokens) {
            assert_ne!(br.blind_message.0, *msg);
            assert_eq!(b64_any(&t.msg).unwrap(), *msg);
        }
        for t in &tokens {
            let r = spend_token(&app, t).await;
            assert_eq!(r.status(), StatusCode::OK);
            let s = settlement_of(&r);
            assert!(s.success && s.transaction.starts_with("tab:"));
        }
        // ATTACK: double-spend a token
        let r = spend_token(&app, &tokens[1]).await;
        assert_eq!(r.status(), StatusCode::PAYMENT_REQUIRED);
        assert_eq!(settlement_of(&r).error_reason.as_deref(), Some(reason::NONCE_USED));
        assert_eq!(body_json(r).await["error"], "tab token already spent");
    }

    #[tokio::test]
    async fn forged_and_tampered_tokens_are_refused() {
        let app = app_with_stand_in().await;
        let (offer, nonce) = tab_offer_and_nonce(&app).await;
        let (pk, toks) = blind_n(&offer, 1);
        let good = finalize(&pk, &toks, issue(&app, &nonce, TX_OK, &offer.key_id, &toks).await).await.remove(0);

        // ATTACK: random signature bytes
        let mut forged = good.clone();
        let mut junk = vec![0u8; 256];
        rand::thread_rng().fill_bytes(&mut junk);
        forged.sig = b64(&junk);
        assert_eq!(body_json(spend_token(&app, &forged).await).await["error"], "tab token signature invalid");
        // ATTACK: a valid signature moved onto another message
        let mut moved = good.clone();
        moved.msg = b64(b"some other message the server never signed");
        assert_eq!(body_json(spend_token(&app, &moved).await).await["error"], "tab token signature invalid");
        // ATTACK: different randomizer
        let mut rnd = good.clone();
        rnd.randomizer = b64(&[7u8; 32]);
        assert_eq!(body_json(spend_token(&app, &rnd).await).await["error"], "tab token signature invalid");
        // ATTACK: a token self-signed with the attacker's own key, claiming the server's keyId
        let mine = blind_rsa_signatures::KeyPair::<blind_rsa_signatures::Sha384, blind_rsa_signatures::PSS, blind_rsa_signatures::Randomized>::generate(&mut DefaultRng, 2048).unwrap();
        let msg = b"self-made token".to_vec();
        let br = mine.pk.blind(&mut DefaultRng, &msg).unwrap();
        let sig = mine.pk.finalize(&mine.sk.blind_sign(&br.blind_message).unwrap(), &br, &msg).unwrap();
        let fake = TabPayload { key_id: offer.key_id.clone(), msg: b64(&msg), randomizer: b64(&br.msg_randomizer.unwrap().0), sig: b64(&sig) };
        assert_eq!(body_json(spend_token(&app, &fake).await).await["error"], "tab token signature invalid");
        // none of that spent the real token
        assert_eq!(spend_token(&app, &good).await.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn tokens_from_another_key_epoch_are_refused() {
        let app = app_with_stand_in().await;
        let (offer, nonce) = tab_offer_and_nonce(&app).await;
        let (pk, toks) = blind_n(&offer, 1);
        let old = finalize(&pk, &toks, issue(&app, &nonce, TX_OK, &offer.key_id, &toks).await).await.remove(0);

        // the epoch rolls over: a new issuing key replaces the old one
        let new_key = tab::rotate(&app.store, now()).unwrap();
        *app.tab.write().unwrap() = None;
        assert_ne!(new_key.key_id, offer.key_id);
        let r = spend_token(&app, &old).await;
        assert_eq!(r.status(), StatusCode::PAYMENT_REQUIRED);
        assert_eq!(settlement_of(&r).error_reason.as_deref(), Some(reason::EXPIRED));
        assert!(body_json(r).await["error"].as_str().unwrap().contains("another epoch"));
        // ...and the old token relabelled with the new keyId fails the signature check
        let mut relabelled = old.clone();
        relabelled.key_id = new_key.key_id.clone();
        assert_eq!(body_json(spend_token(&app, &relabelled).await).await["error"], "tab token signature invalid");
        // issuing against the old keyId is refused too, as a retryable 409 that burns nothing
        let (offer2, nonce2) = tab_offer_and_nonce(&app).await;
        assert_eq!(offer2.key_id, new_key.key_id);
        let (_, toks2) = blind_n(&offer, 1); // blinded for the retired key
        let r = issue(&app, &nonce2, TX_OK, &offer.key_id, &toks2).await;
        assert_eq!(r.status(), StatusCode::CONFLICT);
        let v = body_json(r).await;
        assert_eq!((v["reason"].as_str(), v["activeKeyId"].as_str()), (Some(reason::TAB_KEY_ROTATED), Some(new_key.key_id.as_str())));
        assert!(app.store.get(&nonce2).unwrap().unwrap().spent_txid.is_none(), "nothing burned");
        // ...and blinding again for the active key with the same nonce and txid works
        let (pk2, toks3) = blind_n(&offer2, 1);
        assert_eq!(finalize(&pk2, &toks3, issue(&app, &nonce2, TX_OK, &offer2.key_id, &toks3).await).await.len(), 1);
    }

    #[tokio::test]
    async fn the_previous_key_issues_and_accepts_tokens_during_its_grace_period_only() {
        let mut a = Arc::try_unwrap(app_with_stand_in().await).ok().unwrap();
        a.cfg.tab_grace_secs = 600;
        let app = Arc::new(a);
        let (offer, nonce) = tab_offer_and_nonce(&app).await;
        assert_eq!(offer.grace_secs, 600, "the offer says how long tokens outlive the epoch");
        let (pk, toks) = blind_n(&offer, 3);
        let tokens = finalize(&pk, &toks, issue(&app, &nonce, TX_PAID_3X, &offer.key_id, &toks).await).await;
        // a tab paid under the old key just before the rotation, issued just after it
        let (_, nonce_b) = tab_offer_and_nonce(&app).await;
        let (_, late) = blind_n(&offer, 1);

        // the epoch rolls over a moment later
        let new_key = tab::rotate(&app.store, now()).unwrap();
        *app.tab.write().unwrap() = None;
        assert_ne!(new_key.key_id, offer.key_id);
        // tokens bought a minute ago still spend, once
        assert_eq!(spend_token(&app, &tokens[0]).await.status(), StatusCode::OK);
        assert_eq!(body_json(spend_token(&app, &tokens[0]).await).await["error"], "tab token already spent");
        // the late tab is still issued under the old key
        let r = issue(&app, &nonce_b, TX_OK, &offer.key_id, &late).await;
        assert_eq!(r.status(), StatusCode::OK);
        assert_eq!(body_json(r).await["keyId"], offer.key_id.as_str());

        // once the grace period is over the old key is dead
        app.store
            .conn_for_tests()
            .execute("UPDATE tab_keys SET retired_at = ?1 WHERE key_id = ?2", rusqlite::params![(now() - 601) as i64, offer.key_id])
            .unwrap();
        let r = spend_token(&app, &tokens[1]).await;
        assert_eq!(r.status(), StatusCode::PAYMENT_REQUIRED);
        assert_eq!(settlement_of(&r).error_reason.as_deref(), Some(reason::EXPIRED));
        let (_, nonce_c) = tab_offer_and_nonce(&app).await;
        assert_eq!(issue(&app, &nonce_c, TX_OK, &offer.key_id, &late).await.status(), StatusCode::CONFLICT);
    }

    #[tokio::test]
    async fn a_nonce_issues_once_and_underpaid_n_burns_nothing() {
        let app = app_with_stand_in().await;
        let (offer, nonce) = tab_offer_and_nonce(&app).await;

        // ATTACK: pay for 1, ask for 2
        let (pk, two) = blind_n(&offer, 2);
        let r = issue(&app, &nonce, TX_OK, &offer.key_id, &two).await;
        assert_eq!(r.status(), StatusCode::UNPROCESSABLE_ENTITY);
        let v = body_json(r).await;
        assert!(v["message"].as_str().unwrap().contains("underpaid for 2 tokens"), "{v}");
        assert!(app.store.get(&nonce).unwrap().unwrap().spent_txid.is_none(), "underpaid n burns nothing");

        // the honest retry with 1 works
        let one = &two[..1];
        let toks = finalize(&pk, one, issue(&app, &nonce, TX_OK, &offer.key_id, one).await).await;
        assert_eq!(toks.len(), 1);

        // ATTACK: reissue on the same nonce (same txid, fresh blinded messages)
        let (_, again) = blind_n(&offer, 1);
        let r = issue(&app, &nonce, TX_OK, &offer.key_id, &again).await;
        assert_eq!(r.status(), StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(body_json(r).await["reason"], reason::NONCE_USED);
        // ...and the nonce can't be redeemed as a single payment either
        let r = get_quote_with(app.clone(), [10, 0, 0, 1], &[(PAYMENT_HEADER, &x_payment_header(TX_OK, &nonce))]).await;
        assert_eq!(settlement_of(&r).error_reason.as_deref(), Some(reason::NONCE_USED));

        // more than maxTokens, or malformed blinded messages, are refused up front
        let (_, six) = blind_n(&offer, 6);
        let (_, n2) = tab_offer_and_nonce(&app).await;
        assert_eq!(issue(&app, &n2, TX_PAID_3X, &offer.key_id, &six).await.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn tab_key_and_spent_tokens_survive_a_restart() {
        let path = tmp_db("tab-restart");
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, Router::new().route(SETTLE_PATH, axum::routing::post(stand_in_settle))).await.unwrap();
        });
        let start = || {
            let mut a = Arc::try_unwrap(test_app_at(100, &path)).ok().unwrap();
            a.cfg.facilitator_url = format!("http://{addr}");
            Arc::new(a)
        };
        let app = start();
        let (offer, nonce) = tab_offer_and_nonce(&app).await;
        let (pk, toks) = blind_n(&offer, 2);
        let tokens = finalize(&pk, &toks, issue(&app, &nonce, TX_PAID_3X, &offer.key_id, &toks).await).await;
        assert_eq!(spend_token(&app, &tokens[0]).await.status(), StatusCode::OK);
        drop(app);

        let app = start();
        assert_eq!(tab_offer_and_nonce(&app).await.0.key_id, offer.key_id, "same key after restart");
        assert_eq!(spend_token(&app, &tokens[1]).await.status(), StatusCode::OK);
        assert_eq!(body_json(spend_token(&app, &tokens[0]).await).await["error"], "tab token already spent");
    }

    // -- public port, feed hygiene and the X-PAYMENT budget ---------------------------------------

    async fn get_on(router: Router, path: &str) -> StatusCode {
        use axum::{body::Body, extract::connect_info::MockConnectInfo, http::Request};
        use tower::ServiceExt;
        router
            .layer(MockConnectInfo(SocketAddr::from(([203, 0, 113, 9], 40000))))
            .oneshot(Request::get(path).header("host", "kage.test").body(Body::empty()).unwrap())
            .await
            .unwrap()
            .status()
    }

    #[tokio::test]
    async fn the_public_port_serves_no_dashboard_feed_or_status() {
        let app = test_app(5);
        for path in ["/", "/api/events?since=0", "/api/status"] {
            assert_eq!(get_on(public_router(app.clone()), path).await, StatusCode::NOT_FOUND, "{path} on the public port");
            assert_eq!(get_on(admin_router(app.clone()), path).await, StatusCode::OK, "{path} on the admin port");
        }
        // the paid resource itself is on both
        assert_eq!(get_on(public_router(app.clone()), "/api/quote").await, StatusCode::PAYMENT_REQUIRED);
        assert_eq!(get_on(admin_router(app), "/api/quote").await, StatusCode::PAYMENT_REQUIRED);
    }

    #[test]
    fn pruning_and_the_open_count_use_the_open_challenges_index() {
        let s = mem_store();
        let conn = s.conn_for_tests();
        let plan = |sql: &str| -> String {
            let mut q = conn.prepare(&format!("EXPLAIN QUERY PLAN {sql}")).unwrap();
            q.query_map([], |r| r.get::<_, String>(3)).unwrap().map(Result::unwrap).collect::<Vec<_>>().join("; ")
        };
        for sql in [
            "DELETE FROM challenges WHERE spent_txid IS NULL AND expires_at < 5",
            "SELECT COUNT(*) FROM challenges WHERE spent_txid IS NULL AND expires_at > 5",
        ] {
            let p = plan(sql);
            assert!(p.contains("challenges_open"), "{sql}: {p}");
            assert!(!p.contains("SCAN challenges"), "{sql} scans the whole table: {p}");
        }
    }

    #[tokio::test]
    async fn expired_challenges_are_pruned_at_most_once_a_minute() {
        let app = test_app(100);
        let stale = PaymentRequired {
            scheme: SCHEME.into(), network: Network::Testnet, pay_to: UA.into(),
            amount_zat: 100_000, memo_nonce: "x402z-stale".into(), expires_at: 1,
        };
        get_quote(app.clone(), [10, 0, 0, 1], None).await; // first 402 prunes and starts the clock
        app.store.insert(&stale).unwrap();
        get_quote(app.clone(), [10, 0, 0, 1], None).await;
        assert!(app.store.get("x402z-stale").unwrap().is_some(), "not pruned again within the minute");
        app.last_prune.store(now() - PRUNE_EVERY_SECS, std::sync::atomic::Ordering::Relaxed);
        get_quote(app.clone(), [10, 0, 0, 1], None).await;
        assert!(app.store.get("x402z-stale").unwrap().is_none(), "pruned once the minute is over");
    }

    #[tokio::test]
    async fn the_admin_console_refuses_a_foreign_host_header() {
        use axum::{body::Body, extract::connect_info::MockConnectInfo, http::Request};
        use tower::ServiceExt;
        let app = test_app(5);
        let status_for = |host: &'static str, path: &'static str| {
            let app = app.clone();
            async move {
                admin_app(app, "127.0.0.1:4029", &["dash.internal:4029".to_string()])
                    .layer(MockConnectInfo(SocketAddr::from(([127, 0, 0, 1], 40000))))
                    .oneshot(Request::get(path).header("host", host).body(Body::empty()).unwrap())
                    .await
                    .unwrap()
                    .status()
            }
        };
        // a DNS-rebinding page: the browser sends the attacker's host name
        for path in ["/api/status", "/api/events?since=0", "/", "/api/quote"] {
            assert_eq!(status_for("attacker.example:4029", path).await, StatusCode::FORBIDDEN, "{path}");
            assert_eq!(status_for("127.0.0.1:9999", path).await, StatusCode::FORBIDDEN, "{path} on another port");
        }
        for host in ["127.0.0.1:4029", "localhost:4029", "LOCALHOST:4029", "[::1]:4029", "dash.internal:4029"] {
            assert_eq!(status_for(host, "/api/status").await, StatusCode::OK, "{host}");
        }
    }

    fn feed(app: &App) -> Vec<Event> {
        app.book.lock().unwrap().events.clone()
    }

    #[tokio::test]
    async fn pending_and_rate_limited_feed_rows_carry_no_full_nonce_and_no_client_address() {
        let app = {
            let mut a = Arc::try_unwrap(app_with_stand_in().await).ok().unwrap();
            a.limiter = Mutex::new(ChallengeLimiter::new(2));
            Arc::new(a)
        };
        let v = body_json(get_quote(app.clone(), [10, 0, 0, 1], None).await).await;
        let nonce = v["accepts"][0]["extra"]["memoNonce"].as_str().unwrap().to_string();
        let r = get_quote_with(app.clone(), [10, 0, 0, 1], &[(PAYMENT_HEADER, &x_payment_header(TX_PENDING, &nonce))]).await;
        assert_eq!(settlement_of(&r).error_reason.as_deref(), Some(reason::PENDING));
        // budget of 2 is spent: the next attempt is a 429 row
        let r = get_quote_with(app.clone(), [10, 0, 0, 1], &[(PAYMENT_HEADER, &x_payment_header(TX_PENDING, &nonce))]).await;
        assert_eq!(r.status(), StatusCode::TOO_MANY_REQUESTS);

        let events = feed(&app);
        let pending: Vec<_> = events.iter().filter(|e| e.kind == "pending").collect();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].txid.as_deref(), Some(TX_PENDING));
        let shown = pending[0].memo_nonce.clone().unwrap();
        assert_ne!(shown, nonce, "an unspent nonce never sits next to its txid in the feed");
        assert!(nonce.starts_with(shown.trim_end_matches('…')) && shown.chars().count() < nonce.len(), "{shown}");
        let all = serde_json::to_string(&events).unwrap();
        assert!(!all.contains("10.0.0.1"), "client address in the feed: {all}");
        assert!(events.iter().any(|e| e.kind == "rejected" && e.detail.starts_with("rate limited")), "{all}");
    }

    /// A stand-in facilitator that counts every `/settle` it is asked and always answers pending.
    async fn counting_pending_facilitator() -> (String, Arc<std::sync::atomic::AtomicUsize>) {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let hits = Arc::new(AtomicUsize::new(0));
        let h = hits.clone();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let handler = move |headers: HeaderMap, Json(_): Json<FacilitatorRequest>| {
            let h = h.clone();
            async move {
                assert!(bearer_ok(&headers), "the server sends X402Z_FACILITATOR_TOKEN");
                h.fetch_add(1, Ordering::SeqCst);
                Json(SettleResponse {
                    settlement: SettlementResponse::failed(reason::NOT_FOUND),
                    status: VerifyStatus::NotFound,
                    receipt: None,
                    claims: None,
                    confirmations: None,
                    min_confirmations: None,
                    error: Some("lightwalletd does not know tx".into()),
                })
            }
        };
        tokio::spawn(async move {
            axum::serve(listener, Router::new().route(SETTLE_PATH, axum::routing::post(handler))).await.unwrap();
        });
        (format!("http://{addr}"), hits)
    }

    #[tokio::test]
    async fn x_payment_attempts_that_stay_pending_share_the_client_budget() {
        use std::sync::atomic::Ordering;
        let (url, hits) = counting_pending_facilitator().await;
        let mut a = Arc::try_unwrap(test_app(2)).ok().unwrap();
        a.cfg.facilitator_url = url;
        let app = Arc::new(a);
        let v = body_json(get_quote(app.clone(), [10, 0, 0, 7], None).await).await;
        let nonce = v["accepts"][0]["extra"]["memoNonce"].as_str().unwrap().to_string();
        let mut codes = Vec::new();
        for i in 0..40u32 {
            let txid = format!("{:064x}", 0xfeed_0000u64 + u64::from(i));
            codes.push(get_quote_with(app.clone(), [10, 0, 0, 7], &[(PAYMENT_HEADER, &x_payment_header(&txid, &nonce))]).await.status());
        }
        assert_eq!(codes[0], StatusCode::PAYMENT_REQUIRED, "the first attempt is checked and stays pending");
        assert!(codes[1..].iter().all(|c| *c == StatusCode::TOO_MANY_REQUESTS), "{codes:?}");
        assert_eq!(hits.load(Ordering::SeqCst), 1, "the facilitator (and lightwalletd behind it) was asked once");
        // another client still has its own budget
        assert_eq!(get_quote(app.clone(), [10, 0, 0, 8], None).await.status(), StatusCode::PAYMENT_REQUIRED);
    }

    #[tokio::test]
    async fn an_impossible_nonce_or_txid_is_a_400_with_no_feed_row_and_no_facilitator_call() {
        use std::sync::atomic::Ordering;
        let (url, hits) = counting_pending_facilitator().await;
        let mut a = Arc::try_unwrap(test_app(100)).ok().unwrap();
        a.cfg.facilitator_url = url;
        let app = Arc::new(a);
        let before = feed(&app).len();
        let huge = "n".repeat(60_000);
        for (txid, nonce) in [(TX_OK, huge.as_str()), (TX_OK, "x402z bad nonce"), ("zz", "x402z-ok"), (&*"a".repeat(65), "x402z-ok")] {
            let r = get_quote_with(app.clone(), [10, 0, 0, 1], &[(PAYMENT_HEADER, &x_payment_header(txid, nonce))]).await;
            assert_eq!(r.status(), StatusCode::BAD_REQUEST);
            assert_eq!(settlement_of(&r).error_reason.as_deref(), Some(reason::INVALID_PAYLOAD));
        }
        assert_eq!(feed(&app).len(), before, "nothing logged");
        assert_eq!(hits.load(Ordering::SeqCst), 0, "nothing asked");
        // the paste-txid path checks the nonce the same way
        let r = post_redeem(app.clone(), serde_json::json!({"memoNonce": huge, "txid": TX_OK})).await;
        assert_eq!(r.status(), StatusCode::BAD_REQUEST);
        assert_eq!(feed(&app).len(), before);
    }

    #[tokio::test]
    async fn a_facilitator_that_refuses_the_token_is_setup_required_and_burns_nothing() {
        let mut a = Arc::try_unwrap(app_with_stand_in().await).ok().unwrap();
        a.cfg.facilitator_token = "not-the-token".into();
        let app = Arc::new(a);
        let v = body_json(get_quote(app.clone(), [10, 0, 0, 1], None).await).await;
        let nonce = v["accepts"][0]["extra"]["memoNonce"].as_str().unwrap().to_string();
        let r = get_quote_with(app.clone(), [10, 0, 0, 1], &[(PAYMENT_HEADER, &x_payment_header(TX_OK, &nonce))]).await;
        assert_eq!(r.status(), StatusCode::BAD_GATEWAY);
        assert!(body_json(r).await["error"].as_str().unwrap().contains("X402Z_FACILITATOR_TOKEN"));
        assert_eq!(open_nonces(&app), vec![nonce]);
    }

    #[test]
    fn the_demo_panel_opens_the_sealed_agent_ledger_only_with_its_key() {
        let path = std::env::temp_dir().join(format!("x402z-server-{}-demo-ledger", std::process::id()));
        let key = [0x33u8; 32];
        let sealed = spendguard::sealed::seal(&key, spendguard::sealed::LEDGER_PURPOSE, [1; 24], br#"{"agents":[{"agent":"a1"}],"audit":[]}"#);
        std::fs::write(&path, &sealed).unwrap();
        let p = path.to_str().unwrap();
        assert_eq!(read_ledger(Some(p), Some(&key))["data"]["agents"][0]["agent"], "a1");
        assert!(read_ledger(Some(p), None)["error"].as_str().unwrap().starts_with("Setup required: X402Z_DEMO_AGENT_LEDGER_KEY"));
        assert!(read_ledger(Some(p), Some(&[0x34; 32]))["error"].as_str().unwrap().contains("wrong key"));
        std::fs::write(&path, br#"{"agents":[],"audit":[]}"#).unwrap(); // a plaintext file is not shown either
        assert!(read_ledger(Some(p), Some(&key))["error"].as_str().unwrap().contains("not a sealed file"));
        assert!(read_ledger(Some("/nonexistent/ledger"), Some(&key))["error"].as_str().unwrap().contains("unreadable"));
        assert!(read_ledger(None, Some(&key)).get("hidden").is_some());
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn feed_rows_are_clipped() {
        let mut b = Book::default();
        b.push("rejected", Some(&"n".repeat(500)), None, Some(&"t".repeat(500)), "x".repeat(10_000));
        let e = &b.events[0];
        assert!(e.detail.chars().count() <= MAX_DETAIL_CHARS + 1);
        assert!(e.memo_nonce.as_ref().unwrap().chars().count() <= MAX_DETAIL_CHARS + 1);
        assert!(e.txid.as_ref().unwrap().chars().count() <= MAX_DETAIL_CHARS + 1);
    }

    #[tokio::test]
    async fn the_402_resource_is_https_behind_a_tls_proxy_or_the_configured_public_base_url() {
        let resource = |r: serde_json::Value| r["accepts"][0]["resource"].as_str().unwrap().to_string();
        let https = [("x-forwarded-proto", "https")];
        // no proxy configured: X-Forwarded-Proto is a client header like any other and is ignored
        let app = test_app(50);
        assert_eq!(resource(body_json(get_quote_with(app.clone(), [10, 0, 0, 6], &https).await).await), "http://kage.test/api/quote");
        // the trusted proxy terminates TLS and says so
        let mut app = test_app(50);
        Arc::get_mut(&mut app).unwrap().cfg.trusted_proxies = vec!["10.0.0.6".parse().unwrap()];
        assert_eq!(resource(body_json(get_quote_with(app.clone(), [10, 0, 0, 6], &https).await).await), "https://kage.test/api/quote");
        // the same header from any other peer is not believed
        assert_eq!(resource(body_json(get_quote_with(app.clone(), [203, 0, 113, 9], &https).await).await), "http://kage.test/api/quote");
        // a proxy that forwards plain HTTP, or sends junk, keeps http
        assert_eq!(resource(body_json(get_quote_with(app.clone(), [10, 0, 0, 6], &[("x-forwarded-proto", "http")]).await).await), "http://kage.test/api/quote");
        assert_eq!(resource(body_json(get_quote_with(app.clone(), [10, 0, 0, 6], &[("x-forwarded-proto", "gopher")]).await).await), "http://kage.test/api/quote");
        // a configured public base URL wins over Host and forwarded headers
        let mut app = test_app(50);
        Arc::get_mut(&mut app).unwrap().cfg.public_base_url = Some("https://kage-api.example".into());
        assert_eq!(resource(body_json(get_quote_with(app.clone(), [203, 0, 113, 9], &[("x-forwarded-proto", "http")]).await).await), "https://kage-api.example/api/quote");
    }

    #[test]
    fn public_base_url_must_be_an_http_origin() {
        assert_eq!(parse_public_base_url("https://kage-api.example/").unwrap(), "https://kage-api.example");
        assert_eq!(parse_public_base_url("HTTP://127.0.0.1:4020").unwrap(), "http://127.0.0.1:4020");
        assert!(parse_public_base_url("kage-api.example").is_err());
        assert!(parse_public_base_url("ftp://kage-api.example").is_err());
        assert!(parse_public_base_url("https://").is_err());
        assert!(parse_public_base_url("https://kage-api.example/api").is_err());
    }

    #[test]
    fn empty_public_origin_reports_the_origin_requirement() {
        for origin in ["https://", "http://", "https:///"] {
            assert!(parse_public_base_url(origin).unwrap_err().contains("must be an origin like https://api.example (no path)"));
        }
        assert_eq!(parse_public_base_url("https://api.example/").unwrap(), "https://api.example");
    }

    #[test]
    fn limiter_key_trusts_forwarded_for_only_from_configured_proxies_and_groups_ipv6_by_64() {
        let proxy: IpAddr = "10.9.9.9".parse().unwrap();
        let mut h = HeaderMap::new();
        h.insert("x-forwarded-for", HeaderValue::from_static("6.6.6.6, 198.51.100.4, 10.9.9.9"));
        // not a trusted proxy: the header is ignored
        assert_eq!(client_key("192.0.2.1".parse().unwrap(), &h, &[proxy]), "192.0.2.1".parse::<IpAddr>().unwrap());
        // trusted proxy: the right-most hop that isn't a proxy (the client can't spoof past it)
        assert_eq!(client_key(proxy, &h, &[proxy]), "198.51.100.4".parse::<IpAddr>().unwrap());
        // no header from a trusted proxy: the proxy itself
        assert_eq!(client_key(proxy, &HeaderMap::new(), &[proxy]), proxy);
        // IPv6: one /64 shares one bucket
        let a = client_key("2001:db8:1:2:aaaa::1".parse().unwrap(), &HeaderMap::new(), &[]);
        let b = client_key("2001:db8:1:2:bbbb::2".parse().unwrap(), &HeaderMap::new(), &[]);
        let c = client_key("2001:db8:1:3::1".parse().unwrap(), &HeaderMap::new(), &[]);
        assert_eq!(a, b);
        assert_ne!(a, c);
        // IPv4-mapped IPv6 is the IPv4 client
        assert_eq!(client_key("::ffff:192.0.2.7".parse().unwrap(), &HeaderMap::new(), &[]), "192.0.2.7".parse::<IpAddr>().unwrap());
    }
}
