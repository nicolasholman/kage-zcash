//! Paywalled demo API + dashboard.
//! `GET /api/quote` returns a 402 challenge until it gets a valid facilitator-signed
//! `X-PAYMENT-RECEIPT`. `GET /` serves the dashboard; `/api/events` and `/api/status` feed it.
//! Nothing here touches the chain: payment proof is the facilitator's ed25519 receipt.
//! Issued challenges and redeemed nonces live in SQLite (`X402Z_SERVER_DB`), so a restart
//! neither loses an open challenge nor un-burns a redeemed one.
mod store;

use std::{
    collections::HashMap,
    net::{IpAddr, SocketAddr},
    path::PathBuf,
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
use x402z_protocol::{paid_in_time, Network, PaymentRequired, ReceiptClaims, DEFAULT_EXPIRY_GRACE_SECS, RECEIPT_HEADER, SCHEME};

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
    agent_ledger: Option<String>,
    /// Receipts with fewer confirmations are refused (0 = accept mempool-only payments).
    min_confirmations: u32,
    /// A payment mined up to this long after `expiresAt` still counts.
    expiry_grace_secs: u64,
    redeem_window_secs: u64,
}

/// Live event feed for the dashboard (in memory; the durable state is in [`Store`]).
#[derive(Default)]
struct Book {
    events: Vec<Event>,
    next_id: u64,
}

impl Book {
    fn push(&mut self, kind: &'static str, nonce: Option<&str>, amount: Option<u64>, txid: Option<&str>, detail: String) {
        self.next_id += 1;
        self.events.push(Event {
            id: self.next_id,
            ts: now(),
            kind,
            memo_nonce: nonce.map(str::to_string),
            amount_zat: amount,
            txid: txid.map(str::to_string),
            detail,
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
    book: Mutex<Book>,
    limiter: Mutex<ChallengeLimiter>,
}

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
    let ch = &row.challenge;
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
    if !store.mark_spent(&c.memo_nonce, &c.txid, now_unix).map_err(ReceiptError::Storage)? {
        return Err(ReceiptError::Replayed);
    }
    Ok(())
}

fn issue_challenge(app: &App) -> Result<PaymentRequired, String> {
    let now_unix = now();
    let keep_after = app.cfg.expiry_grace_secs.saturating_add(app.cfg.redeem_window_secs);
    app.store.prune_unspent_expired_before(now_unix.saturating_sub(keep_after))?;
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

fn payment_required(ch: PaymentRequired, error: Option<String>) -> Response {
    let mut resp = (StatusCode::PAYMENT_REQUIRED, Json(ch)).into_response();
    if let Some(e) = error.and_then(|e| HeaderValue::from_str(&e).ok()) {
        resp.headers_mut().insert("x-payment-error", e);
    }
    resp
}

async fn quote(State(app): State<Arc<App>>, ConnectInfo(peer): ConnectInfo<SocketAddr>, headers: HeaderMap) -> Response {
    // Rate-limits every path that mints a new challenge (first request and re-issue after a rejected receipt).
    let limit = |book: &mut Book| -> Result<(), Response> {
        app.limiter.lock().unwrap().try_acquire(peer.ip(), Instant::now()).map_err(|wait| {
            book.push("rejected", None, None, None, format!("rate limited {} (retry in {}s)", peer.ip(), wait.as_secs().max(1)));
            too_many_challenges(wait)
        })
    };
    let mut book = app.book.lock().unwrap();
    let Some(receipt) = headers.get(RECEIPT_HEADER).and_then(|v| v.to_str().ok()) else {
        if let Err(resp) = limit(&mut book) {
            return resp;
        }
        let ch = match issue_challenge(&app) {
            Ok(ch) => ch,
            Err(e) => return storage_error(e),
        };
        book.push("challenge", Some(&ch.memo_nonce), Some(ch.amount_zat), None, "402 issued".into());
        return payment_required(ch, None);
    };
    let result = decode_receipt(&app.cfg.receipt_key, receipt)
        .and_then(|c| {
            // Checked before settle so an unconfirmed payment doesn't burn the nonce and can retry.
            if c.confirmations < app.cfg.min_confirmations {
                return Err(ReceiptError::Unconfirmed { have: c.confirmations, need: app.cfg.min_confirmations });
            }
            settle(&app.store, &app.cfg.merchant_ua, &c, now(), app.cfg.expiry_grace_secs).map(|_| c)
        });
    match result {
        Ok(c) => {
            let conf = if c.confirmations == 0 { "mempool".to_string() } else { format!("{} conf", c.confirmations) };
            book.push("paid", Some(&c.memo_nonce), Some(c.amount_zat), Some(&c.txid), format!("unlocked ({conf})"));
            Json(serde_json::json!({
                "product": "x402z demo quote",
                "servedAt": now(),
                "settlement": {
                    "memoNonce": c.memo_nonce,
                    "amountZat": c.amount_zat,
                    "txid": c.txid,
                    "confirmations": c.confirmations,
                },
                "quote": "Paid content unlocked by a shielded Zcash testnet payment. The merchant saw it only through its viewing key; the chain shows neither amount nor address.",
            }))
            .into_response()
        }
        Err(e) => {
            book.push("rejected", None, None, None, e.reason());
            if let Err(mut resp) = limit(&mut book) {
                if let Ok(v) = HeaderValue::from_str(&e.reason()) {
                    resp.headers_mut().insert("x-payment-error", v);
                }
                return resp;
            }
            let ch = match issue_challenge(&app) {
                Ok(ch) => ch,
                Err(e) => return storage_error(e),
            };
            book.push("challenge", Some(&ch.memo_nonce), Some(ch.amount_zat), None, "402 re-issued after rejection".into());
            payment_required(ch, Some(e.reason()))
        }
    }
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

/// The agent ledger is written by `x402z-agent` (spendguard state + audit trail).
/// Missing or unreadable => the dashboard shows a red banner naming the problem.
fn read_ledger(path: Option<&str>) -> serde_json::Value {
    let Some(path) = path else {
        return serde_json::json!({ "error": "X402Z_AGENT_LEDGER not configured" });
    };
    match std::fs::read(path) {
        Err(e) => serde_json::json!({ "error": format!("X402Z_AGENT_LEDGER unreadable ({path}): {e}") }),
        Ok(raw) => match serde_json::from_slice::<serde_json::Value>(&raw) {
            Err(e) => serde_json::json!({ "error": format!("X402Z_AGENT_LEDGER is not valid JSON ({path}): {e}") }),
            Ok(v) => serde_json::json!({ "data": v }),
        },
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
        "ledger": read_ledger(app.cfg.agent_ledger.as_deref()),
    }))
}

fn parse_key(hex_key: &str) -> Result<VerifyingKey, String> {
    let raw = hex::decode(hex_key).map_err(|e| format!("not hex: {e}"))?;
    let arr: [u8; 32] = raw.try_into().map_err(|_| "must be 32 bytes".to_string())?;
    VerifyingKey::from_bytes(&arr).map_err(|e| e.to_string())
}

fn router(app: Arc<App>) -> Router {
    Router::new()
        .route("/", get(|| async { Html(DASHBOARD) }))
        .route("/api/quote", get(quote))
        .route("/api/events", get(events))
        .route("/api/status", get(status))
        .with_state(app)
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
    let price_zat = match optional("X402Z_PRICE_ZAT").map(|s| s.parse::<u64>()) {
        None => DEFAULT_PRICE_ZAT,
        Some(Ok(p)) if p > 0 => p,
        Some(_) => {
            eprintln!("error: X402Z_PRICE_ZAT must be a positive integer (zatoshi)");
            exit(2);
        }
    };
    let bind = optional("X402Z_BIND").unwrap_or_else(|| "127.0.0.1:4020".into());
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
        agent_ledger: optional("X402Z_AGENT_LEDGER"),
        min_confirmations,
        expiry_grace_secs: nonneg("X402Z_EXPIRY_GRACE_SECS", DEFAULT_EXPIRY_GRACE_SECS),
        redeem_window_secs: nonneg("X402Z_REDEEM_WINDOW_SECS", DEFAULT_REDEEM_WINDOW_SECS),
    };
    let db_path = PathBuf::from(optional("X402Z_SERVER_DB").unwrap_or_else(|| "x402z-server.sqlite".into()));
    let store = Store::open(&db_path).unwrap_or_else(|e| {
        eprintln!("error: {e}");
        exit(2);
    });
    eprintln!("challenge book: {} (X402Z_SERVER_DB)", db_path.display());
    if cfg.agent_ledger.is_none() {
        eprintln!("warning: X402Z_AGENT_LEDGER not configured; dashboard will show a red banner");
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
    let app = Arc::new(App { cfg, store, book: Mutex::new(Book::default()), limiter: Mutex::new(ChallengeLimiter::new(rate)) });
    let router = router(app).into_make_service_with_connect_info::<SocketAddr>();
    let listener = tokio::net::TcpListener::bind(&bind).await.unwrap_or_else(|e| {
        eprintln!("error: cannot bind {bind}: {e}");
        exit(1);
    });
    eprintln!("x402z-server listening on http://{bind} (402 challenges limited to {rate}/min per IP)");
    if let Err(e) = axum::serve(listener, router).await {
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
            agent_ledger: None,
            min_confirmations: 1,
            expiry_grace_secs: GRACE,
            redeem_window_secs: DEFAULT_REDEEM_WINDOW_SECS,
        };
        let store = Store::open(db).unwrap();
        Arc::new(App { cfg, store, book: Mutex::new(Book::default()), limiter: Mutex::new(ChallengeLimiter::new(rate)) })
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
        use axum::{body::Body, extract::connect_info::MockConnectInfo, http::Request};
        use tower::ServiceExt;
        let mut req = Request::get("/api/quote");
        if let Some(r) = receipt {
            req = req.header(RECEIPT_HEADER, r);
        }
        router(app)
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
}
