//! Facilitator: merchant incoming viewing key only (no spend key, and by default no full
//! viewing key). Fetches a payer's transaction from lightwalletd, trial-decrypts it with the
//! UIVK, checks it against the 402 challenge and signs an ed25519 receipt for
//! `X-PAYMENT-RECEIPT`.
//!
//! Key: `X402Z_MERCHANT_UIVK` (`uivktest1...`, recommended). `X402Z_MERCHANT_UFVK` still works
//! but logs a warning: a full viewing key also reveals the merchant's outgoing payments and
//! change, which the facilitator never needs. Only its incoming part is kept in memory.
//!
//! Expiry is judged by the header time of the block that mined the payment (`minedAt` in the
//! receipt), with `X402Z_EXPIRY_GRACE_SECS` of slack, so a payment broadcast before expiry is
//! not lost because its block came late or the payer asked for the receipt later.
//! Nonce -> txid bindings persist in SQLite (`X402Z_FACILITATOR_DB`).
//!
//! `/verify` and `/settle` need `Authorization: Bearer $X402Z_FACILITATOR_TOKEN` (the resource
//! server sends it); without the token the facilitator does not start. `/health` and `/supported`
//! are open.
//!
//! `GET /health`     live lightwalletd round-trip; 200 with the chain tip, 503 if it fails.
//! `GET /supported`  x402: `{kinds: [{x402Version: 1, scheme: "zcash-shielded", network: "zcash-testnet"}]}`.
//! `POST /verify`    x402 `{x402Version, paymentPayload, paymentRequirements}` -> read-only
//!                   `{isValid, invalidReason, payer}`; legacy `{challenge, txid}` -> `VerifyResponse`
//!                   with a receipt.
//! `POST /settle`    x402: same body; binds the nonce, returns `{success, transaction, network,
//!                   payer, errorReason?}` plus `receipt`/`claims` for the resource server.
//!
//! `x402z-facilitator keygen` prints a fresh receipt key pair for `.env`.
//! `x402z-facilitator derive-uivk` prints the UIVK for the UFVK in `X402Z_MERCHANT_UFVK`.
use std::{
    path::PathBuf,
    process::exit,
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

use axum::{
    body::Bytes,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use ed25519_dalek::SigningKey;
use x402z_facilitator::{
    lwd::{self, FetchedTx, TxLocation},
    receipt,
    store::IssuedStore,
    verify::{self, MerchantKey, VerifyError},
    NETWORK,
};
use x402z_protocol::{
    paid_in_time, reason, FacilitatorRequest, PaymentRequired, ReceiptClaims, SettleResponse, SettlementResponse,
    SupportedKind, SupportedResponse, VerifyRequest, VerifyResponse, VerifyStatus, X402VerifyResponse,
    DEFAULT_EXPIRY_GRACE_SECS, HEALTH_PATH, NETWORK_TESTNET, SCHEME, SETTLE_PATH, SUPPORTED_PATH, VERIFY_PATH,
    X402_VERSION,
};

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

struct App {
    lwd_url: String,
    lwd: lwd::Client,
    key: MerchantKey,
    signing_key: SigningKey,
    /// When set, challenges for any other `payTo` are refused outright.
    merchant_ua: Option<String>,
    min_confirmations: u32,
    expiry_grace_secs: u64,
    /// (memo nonce, payTo, amount) -> txid it was receipted for, so one requirement can't be
    /// receipted for two txs.
    issued: IssuedStore,
    /// Shared secret the resource server presents on `/verify` and `/settle`.
    token: String,
    /// `/health`'s last chain-tip probe and when it ran ([`HEALTH_CACHE`]): the endpoint has no
    /// auth, so it must not cost a lightwalletd round trip per request.
    health: tokio::sync::Mutex<Option<(std::time::Instant, Result<u32, String>)>>,
    /// Chain-tip probes `/health` actually made.
    health_probes: std::sync::atomic::AtomicUsize,
}

/// How long `/health` reuses its last chain-tip probe (success or failure).
const HEALTH_CACHE: std::time::Duration = std::time::Duration::from_secs(5);

/// Shortest `X402Z_FACILITATOR_TOKEN` accepted (`openssl rand -hex 32` gives 64).
const MIN_TOKEN_LEN: usize = 32;

/// `Authorization: Bearer <token>`, compared in constant time.
fn authorized(app: &App, headers: &HeaderMap) -> bool {
    let Some(got) = headers.get("authorization").and_then(|v| v.to_str().ok()).and_then(|v| v.strip_prefix("Bearer ")) else {
        return false;
    };
    let (a, b) = (got.trim().as_bytes(), app.token.as_bytes());
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

fn unauthorized() -> Response {
    let mut resp = (
        StatusCode::UNAUTHORIZED,
        Json(serde_json::json!({ "error": "facilitator requires Authorization: Bearer <X402Z_FACILITATOR_TOKEN>" })),
    )
        .into_response();
    resp.headers_mut().insert("www-authenticate", axum::http::HeaderValue::from_static("Bearer"));
    resp
}

type Outcome = (StatusCode, Option<&'static str>, VerifyResponse);

fn fail(code: StatusCode, status: VerifyStatus, why: &'static str, error: String) -> Outcome {
    (
        code,
        Some(why),
        VerifyResponse {
            status,
            receipt: None,
            claims: None,
            confirmations: None,
            min_confirmations: None,
            chain_tip: None,
            error: Some(error),
        },
    )
}

/// Does `txid_in` pay `ch`? With `issue_receipt`, a confirmed payment binds the nonce to the
/// txid and gets a signed receipt (`/settle`, legacy `/verify`); without, it is a read-only
/// check (x402 `/verify`).
async fn process(app: &App, ch: &PaymentRequired, txid_in: &str, issue_receipt: bool) -> Outcome {
    // Expiry is checked against the mined block's time below, not against "now".
    if let Err(e) = ch.validate_shape() {
        return fail(StatusCode::BAD_REQUEST, VerifyStatus::Rejected, reason::INVALID_PAYLOAD, format!("invalid challenge: {e:?}"));
    }
    if let Some(ua) = &app.merchant_ua {
        if &ch.pay_to != ua {
            return fail(StatusCode::UNPROCESSABLE_ENTITY, VerifyStatus::Rejected, reason::INVALID_PAYMENT, "payTo is not this merchant's address".into());
        }
    }
    let Some(txid) = lwd::parse_txid(txid_in) else {
        return fail(StatusCode::BAD_REQUEST, VerifyStatus::Rejected, reason::INVALID_PAYLOAD, "txid must be 64 hex chars".into());
    };
    let txid_hex = txid.to_string();
    let prev = match app.issued.txid_for(&ch.memo_nonce, &ch.pay_to, ch.amount_zat) {
        Ok(p) => p,
        Err(e) => return fail(StatusCode::SERVICE_UNAVAILABLE, VerifyStatus::Error, reason::FACILITATOR_ERROR, e),
    };
    if let Some(prev) = prev {
        if prev != txid_hex {
            return fail(
                StatusCode::UNPROCESSABLE_ENTITY,
                VerifyStatus::Rejected,
                reason::NONCE_USED,
                format!("memo nonce already receipted for tx {prev}"),
            );
        }
    }

    let mut client = app.lwd.clone();
    let tip = match lwd::chain_tip(&mut client).await {
        Ok(t) => t,
        Err(e) => return fail(StatusCode::BAD_GATEWAY, VerifyStatus::Error, reason::FACILITATOR_ERROR, format!("{e} ({})", app.lwd_url)),
    };
    let (data, location) = match lwd::get_transaction(&mut client, &txid).await {
        Ok(FetchedTx::Found { data, location }) => (data, location),
        Ok(FetchedTx::NotFound) => {
            return fail(
                StatusCode::NOT_FOUND,
                VerifyStatus::NotFound,
                reason::NOT_FOUND,
                format!("lightwalletd does not know tx {txid_hex} yet (tip {tip})"),
            )
        }
        Err(e) => return fail(StatusCode::BAD_GATEWAY, VerifyStatus::Error, reason::FACILITATOR_ERROR, format!("{e} ({})", app.lwd_url)),
    };
    let mined = match location {
        TxLocation::Mined(h) => Some(h),
        TxLocation::Mempool => None,
        TxLocation::Orphaned => {
            return fail(StatusCode::ACCEPTED, VerifyStatus::Pending, reason::PENDING, format!("tx {txid_hex} is only mined on a fork; wait"))
        }
    };

    let result = verify::parse_tx(&NETWORK, &data, mined, tip).and_then(|tx| {
        if tx.txid() != txid {
            return Err(VerifyError::TxidMismatch { asked: txid_hex.clone(), got: tx.txid().to_string() });
        }
        verify::verify_payment(&NETWORK, &app.key, ch, &tx, mined, tip)
    });
    let payment = match result {
        Ok(p) => p,
        Err(e) => {
            eprintln!("verify {} tx {txid_hex}: rejected: {e}", ch.memo_nonce);
            return fail(StatusCode::UNPROCESSABLE_ENTITY, VerifyStatus::Rejected, reason::INVALID_PAYMENT, e.to_string());
        }
    };
    let tip_u64 = u64::from(u32::from(tip));
    let mined_at = match mined {
        Some(h) => match lwd::block_time(&mut client, h).await {
            Ok(t) => Some(t),
            Err(e) => {
                return fail(StatusCode::BAD_GATEWAY, VerifyStatus::Error, reason::FACILITATOR_ERROR, format!("block {h} time: {e} ({})", app.lwd_url))
            }
        },
        None => None,
    };
    let paid_at = mined_at.unwrap_or_else(now);
    if !paid_in_time(ch.expires_at, paid_at, app.expiry_grace_secs) {
        let when = if mined_at.is_some() { "was mined" } else { "is still unmined" };
        eprintln!("verify {} tx {txid_hex}: rejected: paid after expiry ({when} at {paid_at}, expired {})", ch.memo_nonce, ch.expires_at);
        return fail(
            StatusCode::UNPROCESSABLE_ENTITY,
            VerifyStatus::Rejected,
            reason::EXPIRED,
            format!(
                "payment {when} at {paid_at}, after the challenge expired at {} plus {} s grace",
                ch.expires_at, app.expiry_grace_secs
            ),
        );
    }
    if payment.confirmations < app.min_confirmations {
        eprintln!(
            "verify {} tx {txid_hex}: pending ({} of {} confirmations, tip {tip})",
            ch.memo_nonce, payment.confirmations, app.min_confirmations
        );
        return (
            StatusCode::ACCEPTED,
            Some(reason::PENDING),
            VerifyResponse {
                status: VerifyStatus::Pending,
                receipt: None,
                claims: None,
                confirmations: Some(payment.confirmations),
                min_confirmations: Some(app.min_confirmations),
                chain_tip: Some(tip_u64),
                error: Some(format!(
                    "payment found ({} zat) with {} confirmation(s); need {}",
                    payment.amount_zat, payment.confirmations, app.min_confirmations
                )),
            },
        );
    }

    let claims = ReceiptClaims {
        memo_nonce: ch.memo_nonce.clone(),
        amount_zat: payment.amount_zat,
        pay_to: ch.pay_to.clone(),
        txid: txid_hex.clone(),
        confirmations: payment.confirmations,
        issued_at: now(),
        mined_at,
    };
    if !issue_receipt {
        eprintln!("verify {} tx {txid_hex}: valid ({} zat, {} conf); read-only check, no receipt", ch.memo_nonce, payment.amount_zat, payment.confirmations);
        return (
            StatusCode::OK,
            None,
            VerifyResponse {
                status: VerifyStatus::Confirmed,
                receipt: None,
                claims: Some(claims),
                confirmations: Some(payment.confirmations),
                min_confirmations: Some(app.min_confirmations),
                chain_tip: Some(tip_u64),
                error: None,
            },
        );
    }
    match app.issued.bind(&ch.memo_nonce, &ch.pay_to, ch.amount_zat, &txid_hex, now()) {
        Ok(bound) if bound == txid_hex => {}
        Ok(bound) => {
            return fail(
                StatusCode::UNPROCESSABLE_ENTITY,
                VerifyStatus::Rejected,
                reason::NONCE_USED,
                format!("memo nonce already receipted for tx {bound}"),
            )
        }
        Err(e) => return fail(StatusCode::SERVICE_UNAVAILABLE, VerifyStatus::Error, reason::FACILITATOR_ERROR, e),
    }
    let signed = receipt::sign(&app.signing_key, &claims);
    eprintln!(
        "verify {} tx {txid_hex}: CONFIRMED {} zat via {:?}, {} conf (tip {tip}); receipt issued",
        ch.memo_nonce, payment.amount_zat, payment.pools, payment.confirmations
    );
    (
        StatusCode::OK,
        None,
        VerifyResponse {
            status: VerifyStatus::Confirmed,
            receipt: Some(signed),
            claims: Some(claims),
            confirmations: Some(payment.confirmations),
            min_confirmations: Some(app.min_confirmations),
            chain_tip: Some(tip_u64),
            error: None,
        },
    )
}

/// x402 request body -> (challenge, txid). The payload's memoNonce must name the same
/// challenge as the requirements.
fn x402_request(body: &[u8]) -> Result<(PaymentRequired, String), (&'static str, String)> {
    let req: FacilitatorRequest =
        serde_json::from_slice(body).map_err(|e| (reason::INVALID_PAYLOAD, format!("bad request body: {e}")))?;
    if req.x402_version != X402_VERSION {
        return Err((reason::INVALID_X402_VERSION, format!("x402Version {} not supported", req.x402_version)));
    }
    let pay = req.payment_payload.zcash_payload().map_err(|e| (e.reason(), format!("paymentPayload: {e}")))?;
    let ch = req.payment_requirements.to_challenge().map_err(|e| (e.reason(), format!("paymentRequirements: {e}")))?;
    if pay.memo_nonce != ch.memo_nonce {
        return Err((reason::INVALID_PAYLOAD, "paymentPayload.memoNonce does not match paymentRequirements.extra.memoNonce".into()));
    }
    Ok((ch, pay.txid))
}

fn is_x402_body(body: &[u8]) -> bool {
    serde_json::from_slice::<serde_json::Value>(body).map(|v| v.get("paymentPayload").is_some()).unwrap_or(false)
}

/// `POST /verify`. x402 body (`{x402Version, paymentPayload, paymentRequirements}`) -> read-only
/// `{isValid, invalidReason, payer}`. Legacy body (`{challenge, txid}`) -> `VerifyResponse` with a
/// receipt, as before.
async fn verify_handler(State(app): State<Arc<App>>, headers: HeaderMap, body: Bytes) -> Response {
    if !authorized(&app, &headers) {
        return unauthorized();
    }
    if is_x402_body(&body) {
        let (code, why, r) = match x402_request(&body) {
            Ok((ch, txid)) => process(&app, &ch, &txid, false).await,
            Err((why, e)) => fail(StatusCode::BAD_REQUEST, VerifyStatus::Rejected, why, e),
        };
        let valid = r.status == VerifyStatus::Confirmed;
        let out = X402VerifyResponse {
            is_valid: valid,
            invalid_reason: if valid { None } else { why.map(str::to_string) },
            payer: String::new(),
            status: r.status,
            error: r.error,
        };
        return (code, Json(out)).into_response();
    }
    let req: VerifyRequest = match serde_json::from_slice(&body) {
        Ok(r) => r,
        Err(e) => {
            let (code, _, r) = fail(StatusCode::BAD_REQUEST, VerifyStatus::Rejected, reason::INVALID_PAYLOAD, format!("bad request body: {e}"));
            return (code, Json(r)).into_response();
        }
    };
    let (code, _, r) = process(&app, &req.challenge, &req.txid, true).await;
    (code, Json(r)).into_response()
}

/// `POST /settle` (x402): binds the nonce to the txid and returns the settlement plus a signed
/// receipt for the resource server.
async fn settle_handler(State(app): State<Arc<App>>, headers: HeaderMap, body: Bytes) -> Response {
    if !authorized(&app, &headers) {
        return unauthorized();
    }
    let (code, why, r) = match x402_request(&body) {
        Ok((ch, txid)) => process(&app, &ch, &txid, true).await,
        Err((why, e)) => fail(StatusCode::BAD_REQUEST, VerifyStatus::Rejected, why, e),
    };
    let settlement = match (&r.status, &r.claims) {
        (VerifyStatus::Confirmed, Some(c)) => SettlementResponse::ok(&c.txid),
        _ => SettlementResponse::failed(why.unwrap_or(reason::FACILITATOR_ERROR)),
    };
    let out = SettleResponse {
        settlement,
        status: r.status,
        receipt: r.receipt,
        claims: r.claims,
        confirmations: r.confirmations,
        min_confirmations: r.min_confirmations,
        error: r.error,
    };
    (code, Json(out)).into_response()
}

/// `GET /supported` (x402).
async fn supported() -> Json<SupportedResponse> {
    Json(SupportedResponse {
        kinds: vec![SupportedKind { x402_version: X402_VERSION, scheme: SCHEME.into(), network: NETWORK_TESTNET.into() }],
    })
}

async fn health(State(app): State<Arc<App>>) -> Response {
    // Concurrent requests wait on one probe; within HEALTH_CACHE they reuse its answer.
    let probe = {
        let mut cached = app.health.lock().await;
        match cached.as_ref() {
            Some((at, r)) if at.elapsed() < HEALTH_CACHE => r.clone(),
            _ => {
                app.health_probes.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                let mut client = app.lwd.clone();
                let r = lwd::chain_tip(&mut client).await.map(u32::from).map_err(|e| e.to_string());
                *cached = Some((std::time::Instant::now(), r.clone()));
                r
            }
        }
    };
    match probe {
        Ok(tip) => Json(serde_json::json!({
            "ok": true,
            "network": "testnet",
            "lightwalletd": app.lwd_url,
            "chainTip": tip,
            "minConfirmations": app.min_confirmations,
            "viewingKey": if app.key.from_full_key { "ufvk (incoming part only; prefer X402Z_MERCHANT_UIVK)" } else { "uivk" },
            "receiptPubkey": hex::encode(app.signing_key.verifying_key().as_bytes()),
        }))
        .into_response(),
        Err(e) => (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({ "ok": false, "lightwalletd": app.lwd_url, "error": e })),
        )
            .into_response(),
    }
}

/// The merchant key from the environment: UIVK preferred, UFVK accepted with a warning.
fn merchant_key() -> MerchantKey {
    match (optional("X402Z_MERCHANT_UIVK"), optional("X402Z_MERCHANT_UFVK")) {
        (Some(_), Some(_)) => {
            eprintln!("error: set only one of X402Z_MERCHANT_UIVK and X402Z_MERCHANT_UFVK (the UIVK is enough)");
            exit(2);
        }
        (Some(s), None) => MerchantKey::decode_uivk(&NETWORK, &s).unwrap_or_else(|e| {
            eprintln!("error: X402Z_MERCHANT_UIVK is not a testnet unified incoming viewing key (uivktest1...): {e}");
            exit(2);
        }),
        (None, Some(s)) => {
            let k = MerchantKey::decode_ufvk(&NETWORK, &s).unwrap_or_else(|e| {
                eprintln!("error: X402Z_MERCHANT_UFVK is not a testnet unified full viewing key (uviewtest1...): {e}");
                exit(2);
            });
            eprintln!(
                "warning: X402Z_MERCHANT_UFVK is a full viewing key; it also reveals the merchant's outgoing payments \
                 and change. The facilitator only needs incoming payments: set X402Z_MERCHANT_UIVK instead \
                 (`x402z-facilitator derive-uivk` prints it). Using only the UFVK's incoming part."
            );
            k
        }
        (None, None) => {
            eprintln!("error: X402Z_MERCHANT_UIVK not configured (see .env.example)");
            exit(2);
        }
    }
}

fn derive_uivk() {
    let s = require("X402Z_MERCHANT_UFVK");
    let k = MerchantKey::decode_ufvk(&NETWORK, &s).unwrap_or_else(|e| {
        eprintln!("error: X402Z_MERCHANT_UFVK is not a testnet unified full viewing key: {e}");
        exit(2);
    });
    match k.uivk().encode(&NETWORK) {
        Ok(u) => println!("X402Z_MERCHANT_UIVK={u}"),
        Err(e) => {
            eprintln!("error: cannot encode the UIVK: {e:?}");
            exit(1);
        }
    }
}

fn keygen() {
    let sk = receipt::generate_signing_key();
    println!("X402Z_RECEIPT_SIGNING_KEY={}", hex::encode(sk.to_bytes()));
    println!("X402Z_RECEIPT_PUBKEY={}", hex::encode(sk.verifying_key().as_bytes()));
}

#[tokio::main]
async fn main() {
    match std::env::args().nth(1).as_deref() {
        None | Some("serve") => {}
        Some("keygen") => return keygen(),
        Some("derive-uivk") => return derive_uivk(),
        Some(other) => {
            eprintln!("error: unknown command '{other}' (usage: x402z-facilitator [serve|keygen|derive-uivk])");
            exit(2);
        }
    }

    let lwd_url = optional("ZCASH_LWD_URL").unwrap_or_else(|| {
        eprintln!("ZCASH_LWD_URL not set; using {}", lwd::DEFAULT_LWD_URL);
        lwd::DEFAULT_LWD_URL.to_string()
    });
    let key = merchant_key();
    let signing_key = receipt::parse_signing_key(&require("X402Z_RECEIPT_SIGNING_KEY")).unwrap_or_else(|e| {
        eprintln!("error: X402Z_RECEIPT_SIGNING_KEY invalid: {e} (generate one with `x402z-facilitator keygen`)");
        exit(2);
    });
    let min_confirmations = match optional("X402Z_MIN_CONFIRMATIONS").map(|s| s.parse::<u32>()) {
        None => 1,
        Some(Ok(n)) => n,
        Some(Err(_)) => {
            eprintln!("error: X402Z_MIN_CONFIRMATIONS must be a non-negative integer");
            exit(2);
        }
    };
    let merchant_ua = optional("X402Z_MERCHANT_UA");
    let expiry_grace_secs = match optional("X402Z_EXPIRY_GRACE_SECS").map(|s| s.parse::<u64>()) {
        None => DEFAULT_EXPIRY_GRACE_SECS,
        Some(Ok(n)) => n,
        Some(Err(_)) => {
            eprintln!("error: X402Z_EXPIRY_GRACE_SECS must be a non-negative integer (seconds)");
            exit(2);
        }
    };
    let db_path = PathBuf::from(optional("X402Z_FACILITATOR_DB").unwrap_or_else(|| "x402z-facilitator.sqlite".into()));
    let issued = IssuedStore::open(&db_path).unwrap_or_else(|e| {
        eprintln!("error: {e}");
        exit(2);
    });
    let bind = optional("X402Z_FACILITATOR_BIND").unwrap_or_else(|| "127.0.0.1:4021".into());
    let token = match optional("X402Z_FACILITATOR_TOKEN") {
        Some(t) if t.len() >= MIN_TOKEN_LEN => t,
        Some(_) => {
            eprintln!("error: X402Z_FACILITATOR_TOKEN must be at least {MIN_TOKEN_LEN} characters (e.g. `openssl rand -hex 32`)");
            exit(2);
        }
        None => {
            eprintln!(
                "error: Setup required: X402Z_FACILITATOR_TOKEN not configured. /verify and /settle accept only the resource \
                 server holding this shared secret; generate one with `openssl rand -hex 32` and give the same value to x402z-server."
            );
            exit(2);
        }
    };

    let mut client = lwd::connect(&lwd_url).await.unwrap_or_else(|e| {
        eprintln!("error: {e}");
        exit(1);
    });
    let info = lwd::info(&mut client).await.unwrap_or_else(|e| {
        eprintln!("error: {e} ({lwd_url})");
        exit(1);
    });
    if info.chain_name != "test" {
        eprintln!("error: lightwalletd at {lwd_url} serves chain '{}', expected testnet ('test')", info.chain_name);
        exit(1);
    }
    let tip = lwd::chain_tip(&mut client).await.unwrap_or_else(|e| {
        eprintln!("error: {e} ({lwd_url})");
        exit(1);
    });
    eprintln!(
        "lightwalletd {lwd_url} ({} {}) chain=test tip={tip}",
        info.vendor, info.version
    );
    eprintln!("receipt pubkey (X402Z_RECEIPT_PUBKEY) = {}", hex::encode(signing_key.verifying_key().as_bytes()));

    let app = Arc::new(App {
        lwd_url,
        lwd: client,
        key,
        signing_key,
        merchant_ua,
        min_confirmations,
        expiry_grace_secs,
        issued,
        token,
        health: tokio::sync::Mutex::new(None),
        health_probes: std::sync::atomic::AtomicUsize::new(0),
    });
    let router = Router::new()
        .route(HEALTH_PATH, get(health))
        .route(VERIFY_PATH, post(verify_handler))
        .route(SETTLE_PATH, post(settle_handler))
        .route(SUPPORTED_PATH, get(supported))
        .with_state(app);
    let listener = tokio::net::TcpListener::bind(&bind).await.unwrap_or_else(|e| {
        eprintln!("error: cannot bind {bind}: {e}");
        exit(1);
    });
    eprintln!("x402z-facilitator listening on http://{bind} (min confirmations {min_confirmations})");
    if let Err(e) = axum::serve(listener, router).await {
        eprintln!("error: facilitator stopped: {e}");
        exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use x402z_protocol::{Network, PaymentPayload, PaymentRequirements};

    fn ch() -> PaymentRequired {
        PaymentRequired {
            scheme: SCHEME.into(),
            network: Network::Testnet,
            pay_to: "utest1sampleaddressforschemaonly000000".into(),
            amount_zat: 100_000,
            memo_nonce: "x402z-abc".into(),
            expires_at: 2_000_000_000,
        }
    }

    fn body(payload: PaymentPayload, reqs: PaymentRequirements) -> Vec<u8> {
        serde_json::to_vec(&FacilitatorRequest { x402_version: 1, payment_payload: payload, payment_requirements: reqs }).unwrap()
    }

    #[test]
    fn x402_request_extracts_challenge_and_txid() {
        let reqs = PaymentRequirements::from_challenge(&ch(), "http://h/r", "d", "application/json", 900);
        let b = body(PaymentPayload::zcash(&"ab".repeat(32), "x402z-abc"), reqs.clone());
        assert!(is_x402_body(&b));
        let (c, txid) = x402_request(&b).unwrap();
        assert_eq!((c, txid), (ch(), "ab".repeat(32)));

        // nonce in the payload must match the requirements
        let b = body(PaymentPayload::zcash(&"ab".repeat(32), "x402z-other"), reqs.clone());
        assert_eq!(x402_request(&b).unwrap_err().0, reason::INVALID_PAYLOAD);
        // wrong network
        let mut p = PaymentPayload::zcash("ab", "x402z-abc");
        p.network = "base-sepolia".into();
        assert_eq!(x402_request(&body(p, reqs.clone())).unwrap_err().0, reason::INVALID_NETWORK);
        // wrong version
        let mut raw: serde_json::Value = serde_json::from_slice(&body(PaymentPayload::zcash("ab", "x402z-abc"), reqs)).unwrap();
        raw["x402Version"] = 2.into();
        assert_eq!(x402_request(&serde_json::to_vec(&raw).unwrap()).unwrap_err().0, reason::INVALID_X402_VERSION);
        // legacy body is not mistaken for x402
        let legacy = serde_json::to_vec(&VerifyRequest { challenge: ch(), txid: "ab".repeat(32) }).unwrap();
        assert!(!is_x402_body(&legacy));
    }

    /// The demo merchant's UIVK, published in the README on purpose.
    const DEMO_UIVK: &str = "uivktest1ma9hgu9cmmmkv5f5npk7aktj8tradzqyxzwerez5trhku0xxgx7vsh0azzurz44pfdhmyrzxnur3n282f5t0glhje7kay4fw79njmg64g2wupcznmpfpy0gh6wngu0qw07ry404hnh6rq9vuj264gcjfxucylma4c8cuygxezqx2xwph5x66nckx4095pgvjzuq5rudsmr9dztnyv9f2w74unppttta6cu7fr3tx494zqstn2hv";
    const TOKEN: &str = "0123456789abcdef0123456789abcdef-test";

    /// An app whose lightwalletd channel connects lazily to a closed port: these tests stop
    /// before any chain call.
    fn app() -> Arc<App> {
        let channel = tonic::transport::Endpoint::from_static("http://127.0.0.1:9").connect_lazy();
        Arc::new(App {
            lwd_url: "http://127.0.0.1:9".into(),
            lwd: zcash_client_backend::proto::service::compact_tx_streamer_client::CompactTxStreamerClient::new(channel),
            key: MerchantKey::decode_uivk(&NETWORK, DEMO_UIVK).unwrap(),
            signing_key: SigningKey::from_bytes(&[7u8; 32]),
            merchant_ua: None,
            min_confirmations: 1,
            expiry_grace_secs: DEFAULT_EXPIRY_GRACE_SECS,
            issued: IssuedStore::open(std::path::Path::new(":memory:")).unwrap(),
            token: TOKEN.into(),
            health: tokio::sync::Mutex::new(None),
            health_probes: std::sync::atomic::AtomicUsize::new(0),
        })
    }

    #[tokio::test]
    async fn health_probes_lightwalletd_once_for_a_burst_of_requests_and_keeps_the_503() {
        let a = app(); // lightwalletd on a closed port: every probe fails
        let calls: Vec<_> = (0..200).map(|_| tokio::spawn(health(State(a.clone())))).collect();
        for c in calls {
            assert_eq!(c.await.unwrap().status(), StatusCode::SERVICE_UNAVAILABLE);
        }
        assert_eq!(a.health_probes.load(std::sync::atomic::Ordering::Relaxed), 1, "one lightwalletd round trip for 200 requests");
        // once the cached answer is older than HEALTH_CACHE, the next request probes again
        a.health.lock().await.as_mut().unwrap().0 -= HEALTH_CACHE;
        assert_eq!(health(State(a.clone())).await.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(a.health_probes.load(std::sync::atomic::Ordering::Relaxed), 2);
    }

    fn with_auth(v: &str) -> HeaderMap {
        let mut h = HeaderMap::new();
        h.insert("authorization", axum::http::HeaderValue::from_str(v).unwrap());
        h
    }

    #[tokio::test]
    async fn verify_and_settle_refuse_callers_without_the_token() {
        let reqs = PaymentRequirements::from_challenge(&ch(), "http://h/r", "d", "application/json", 900);
        let b = Bytes::from(body(PaymentPayload::zcash(&"ab".repeat(32), "x402z-abc"), reqs));
        for headers in [HeaderMap::new(), with_auth("Bearer wrong-token-wrong-token-wrong-token"), with_auth(TOKEN), with_auth(&format!("Basic {TOKEN}"))] {
            let r = settle_handler(State(app()), headers.clone(), b.clone()).await;
            assert_eq!(r.status(), StatusCode::UNAUTHORIZED);
            assert_eq!(r.headers().get("www-authenticate").unwrap(), "Bearer");
            assert_eq!(verify_handler(State(app()), headers, b.clone()).await.status(), StatusCode::UNAUTHORIZED);
        }
        // with the token the request gets past auth (and stops at the body check, before any chain call)
        let r = settle_handler(State(app()), with_auth(&format!("Bearer {TOKEN}")), Bytes::from_static(b"{}")).await;
        assert_eq!(r.status(), StatusCode::BAD_REQUEST);
        let r = verify_handler(State(app()), with_auth(&format!("Bearer {TOKEN}")), Bytes::from_static(b"{}")).await;
        assert_eq!(r.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn supported_lists_zcash_shielded_testnet() {
        let Json(s) = supported().await;
        assert_eq!(
            serde_json::to_value(s).unwrap(),
            serde_json::json!({"kinds": [{"x402Version": 1, "scheme": "zcash-shielded", "network": "zcash-testnet"}]})
        );
    }
}
