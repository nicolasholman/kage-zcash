//! Paywalled demo API + dashboard.
//! `GET /api/quote` returns a 402 challenge until it gets a valid facilitator-signed
//! `X-PAYMENT-RECEIPT`. `GET /` serves the dashboard; `/api/events` and `/api/status` feed it.
//! Nothing here touches the chain: payment proof is the facilitator's ed25519 receipt.
use std::{
    collections::{HashMap, HashSet},
    process::exit,
    sync::{Arc, Mutex},
    time::{SystemTime, UNIX_EPOCH},
};

use axum::{
    extract::{Query, State},
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

struct App {
    cfg: Config,
    book: Mutex<Book>,
}

#[derive(Debug, PartialEq, Eq)]
enum ReceiptError {
    Malformed,
    BadSignature,
    UnknownNonce,
    Replayed,
    Expired,
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

async fn quote(State(app): State<Arc<App>>, headers: HeaderMap) -> Response {
    let mut book = app.book.lock().unwrap();
    let Some(receipt) = headers.get(RECEIPT_HEADER).and_then(|v| v.to_str().ok()) else {
        let ch = issue_challenge(&app, &mut book);
        book.push("challenge", Some(&ch.memo_nonce), Some(ch.amount_zat), None, "402 issued".into());
        return payment_required(ch, None);
    };
    let result = decode_receipt(&app.cfg.receipt_key, receipt)
        .and_then(|c| settle(&mut book, &app.cfg.merchant_ua, &c, now()).map(|_| c));
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

async fn status(State(app): State<Arc<App>>) -> Json<serde_json::Value> {
    let open = app.book.lock().unwrap().open.len();
    Json(serde_json::json!({
        "network": "testnet",
        "merchantUa": app.cfg.merchant_ua,
        "priceZat": app.cfg.price_zat,
        "facilitatorUrl": app.cfg.facilitator_url,
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
    let cfg = Config { merchant_ua, price_zat, receipt_key, facilitator_url, agent_ledger: optional("X402Z_AGENT_LEDGER") };
    if cfg.agent_ledger.is_none() {
        eprintln!("warning: X402Z_AGENT_LEDGER not configured; dashboard will show a red banner");
    }
    let app = Arc::new(App { cfg, book: Mutex::new(Book::default()) });
    let router = Router::new()
        .route("/", get(|| async { Html(DASHBOARD) }))
        .route("/api/quote", get(quote))
        .route("/api/events", get(events))
        .route("/api/status", get(status))
        .with_state(app);
    let listener = tokio::net::TcpListener::bind(&bind).await.unwrap_or_else(|e| {
        eprintln!("error: cannot bind {bind}: {e}");
        exit(1);
    });
    eprintln!("x402z-server listening on http://{bind}");
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
}
