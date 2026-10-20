//! Paywalled demo API + dashboard.
//! `GET /api/quote` returns a 402 challenge until it gets a valid facilitator-signed
//! `X-PAYMENT-RECEIPT`. `GET /` serves the dashboard; `/api/events` and `/api/status` feed it.
//! Nothing here touches the chain: payment proof is the facilitator's ed25519 receipt.
use std::{
    collections::{HashMap, HashSet},
    net::{IpAddr, SocketAddr},
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
use x402z_protocol::{Network, PaymentRequired, ReceiptClaims, RECEIPT_HEADER, SCHEME};

const DASHBOARD: &str = include_str!("dashboard.html");
/// ~12 testnet blocks at 75 s: room for 1 confirmation plus a slow wallet.
const CHALLENGE_TTL_SECS: u64 = 15 * 60;
const DEFAULT_PRICE_ZAT: u64 = 100_000; // 0.001 TAZ
const MAX_EVENTS: usize = 200;
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
}

#[derive(Default)]
struct Book {
    open: HashMap<String, PaymentRequired>,
    spent_nonces: HashSet<String>,
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
    Expired,
    Unconfirmed { have: u32, need: u32 },
    WrongAddress,
    Underpaid { paid: u64, want: u64 },
}

impl ReceiptError {
    fn reason(&self) -> String {
        match self {
            Self::Malformed => "receipt malformed".into(),
            Self::BadSignature => "receipt signature invalid".into(),
            Self::UnknownNonce => "memo nonce was never issued here".into(),
            Self::Replayed => "receipt already used".into(),
            Self::Expired => "challenge expired".into(),
            Self::Unconfirmed { have, need } => format!("payment has {have} confirmation(s), need {need}; retry once it is mined"),
            Self::WrongAddress => "paid to a different address".into(),
            Self::Underpaid { paid, want } => format!("underpaid: {paid} zat < {want} zat"),
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

/// Match a signed receipt to an open challenge and burn its nonce.
fn settle(book: &mut Book, merchant_ua: &str, c: &ReceiptClaims, now_unix: u64) -> Result<(), ReceiptError> {
    if book.spent_nonces.contains(&c.memo_nonce) {
        return Err(ReceiptError::Replayed);
    }
    let ch = book.open.get(&c.memo_nonce).ok_or(ReceiptError::UnknownNonce)?;
    if ch.expires_at <= now_unix {
        return Err(ReceiptError::Expired);
    }
    if c.pay_to != merchant_ua || ch.pay_to != merchant_ua {
        return Err(ReceiptError::WrongAddress);
    }
    if c.amount_zat < ch.amount_zat {
        return Err(ReceiptError::Underpaid { paid: c.amount_zat, want: ch.amount_zat });
    }
    book.open.remove(&c.memo_nonce);
    book.spent_nonces.insert(c.memo_nonce.clone());
    Ok(())
}

fn issue_challenge(app: &App, book: &mut Book) -> PaymentRequired {
    let now_unix = now();
    book.open.retain(|_, ch| ch.expires_at > now_unix);
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
    book.open.insert(ch.memo_nonce.clone(), ch.clone());
    ch
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
        let ch = issue_challenge(&app, &mut book);
        book.push("challenge", Some(&ch.memo_nonce), Some(ch.amount_zat), None, "402 issued".into());
        return payment_required(ch, None);
    };
    let result = decode_receipt(&app.cfg.receipt_key, receipt)
        .and_then(|c| {
            // Checked before settle so an unconfirmed payment doesn't burn the nonce and can retry.
            if c.confirmations < app.cfg.min_confirmations {
                return Err(ReceiptError::Unconfirmed { have: c.confirmations, need: app.cfg.min_confirmations });
            }
            settle(&mut book, &app.cfg.merchant_ua, &c, now()).map(|_| c)
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
            let ch = issue_challenge(&app, &mut book);
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
    let open = app.book.lock().unwrap().open.len();
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
    let cfg = Config { merchant_ua, price_zat, receipt_key, facilitator_url, agent_ledger: optional("X402Z_AGENT_LEDGER"), min_confirmations };
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
    let app = Arc::new(App { cfg, book: Mutex::new(Book::default()), limiter: Mutex::new(ChallengeLimiter::new(rate)) });
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
        ReceiptClaims { memo_nonce: nonce.into(), amount_zat: amount, pay_to: UA.into(), txid: "ab".repeat(32), confirmations: 1, issued_at: 1 }
    }

    fn encode(sk: &SigningKey, c: &ReceiptClaims) -> String {
        let raw = serde_json::to_vec(c).unwrap();
        format!("{}.{}", URL_SAFE_NO_PAD.encode(&raw), URL_SAFE_NO_PAD.encode(sk.sign(&raw).to_bytes()))
    }

    fn book_with(nonce: &str, amount: u64, expires_at: u64) -> Book {
        let mut b = Book::default();
        b.open.insert(nonce.into(), PaymentRequired {
            scheme: SCHEME.into(), network: Network::Testnet, pay_to: UA.into(),
            amount_zat: amount, memo_nonce: nonce.into(), expires_at,
        });
        b
    }

    #[test]
    fn valid_receipt_settles_once() {
        let sk = key();
        let c = decode_receipt(&sk.verifying_key(), &encode(&sk, &claims("n1", 100))).unwrap();
        let mut b = book_with("n1", 100, 50);
        assert_eq!(settle(&mut b, UA, &c, 10), Ok(()));
        assert_eq!(settle(&mut b, UA, &c, 10), Err(ReceiptError::Replayed));
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
        let mut b = book_with("n1", 100, 50);
        assert_eq!(settle(&mut b, UA, &claims("n1", 99), 10), Err(ReceiptError::Underpaid { paid: 99, want: 100 }));
        assert_eq!(settle(&mut b, UA, &claims("n1", 100), 50), Err(ReceiptError::Expired));
        assert_eq!(settle(&mut b, UA, &claims("nope", 100), 10), Err(ReceiptError::UnknownNonce));
        let mut wrong = claims("n1", 100);
        wrong.pay_to = "utest1someoneelse000000000000000".into();
        assert_eq!(settle(&mut b, UA, &wrong, 10), Err(ReceiptError::WrongAddress));
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

    fn test_app(rate: u32) -> Arc<App> {
        let cfg = Config {
            merchant_ua: UA.into(),
            price_zat: 100_000,
            receipt_key: key().verifying_key(),
            facilitator_url: "http://127.0.0.1:9".into(),
            agent_ledger: None,
            min_confirmations: 1,
        };
        Arc::new(App { cfg, book: Mutex::new(Book::default()), limiter: Mutex::new(ChallengeLimiter::new(rate)) })
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
        assert_eq!(app.book.lock().unwrap().open.len(), 2);
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
        assert_eq!(app.book.lock().unwrap().open.len(), 1);
    }

    #[tokio::test]
    async fn valid_receipt_is_not_rate_limited() {
        let app = test_app(1);
        let r = get_quote(app.clone(), [10, 0, 0, 1], None).await;
        assert_eq!(r.status(), StatusCode::PAYMENT_REQUIRED);
        let nonce = app.book.lock().unwrap().open.keys().next().unwrap().clone();
        let receipt = encode(&key(), &claims(&nonce, 100_000));
        // budget is spent, but redeeming a receipt mints no challenge
        assert_eq!(get_quote(app.clone(), [10, 0, 0, 1], Some(&receipt)).await.status(), StatusCode::OK);
    }
}
