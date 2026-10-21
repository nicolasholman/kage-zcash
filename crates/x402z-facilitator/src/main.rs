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
//! `GET /health`  live lightwalletd round-trip; 200 with the chain tip, 503 if it fails.
//! `POST /verify` `{challenge, txid}` -> `VerifyResponse` (see x402z_protocol).
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
    http::StatusCode,
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
    paid_in_time, ReceiptClaims, VerifyRequest, VerifyResponse, VerifyStatus, DEFAULT_EXPIRY_GRACE_SECS, HEALTH_PATH,
    VERIFY_PATH,
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
    /// memo nonce -> txid it was receipted for, so one nonce can't be receipted for two txs.
    issued: IssuedStore,
}

fn reply(code: StatusCode, r: VerifyResponse) -> Response {
    (code, Json(r)).into_response()
}

fn fail(code: StatusCode, status: VerifyStatus, error: String) -> Response {
    reply(
        code,
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

async fn verify_handler(State(app): State<Arc<App>>, body: Bytes) -> Response {
    let req: VerifyRequest = match serde_json::from_slice(&body) {
        Ok(r) => r,
        Err(e) => return fail(StatusCode::BAD_REQUEST, VerifyStatus::Rejected, format!("bad request body: {e}")),
    };
    let ch = &req.challenge;
    // Expiry is checked against the mined block's time below, not against "now".
    if let Err(e) = ch.validate_shape() {
        return fail(StatusCode::BAD_REQUEST, VerifyStatus::Rejected, format!("invalid challenge: {e:?}"));
    }
    if let Some(ua) = &app.merchant_ua {
        if &ch.pay_to != ua {
            return fail(StatusCode::UNPROCESSABLE_ENTITY, VerifyStatus::Rejected, "payTo is not this merchant's address".into());
        }
    }
    let Some(txid) = lwd::parse_txid(&req.txid) else {
        return fail(StatusCode::BAD_REQUEST, VerifyStatus::Rejected, "txid must be 64 hex chars".into());
    };
    let txid_hex = txid.to_string();
    let prev = match app.issued.txid_for(&ch.memo_nonce) {
        Ok(p) => p,
        Err(e) => return fail(StatusCode::SERVICE_UNAVAILABLE, VerifyStatus::Error, e),
    };
    if let Some(prev) = prev {
        if prev != txid_hex {
            return fail(
                StatusCode::UNPROCESSABLE_ENTITY,
                VerifyStatus::Rejected,
                format!("memo nonce already receipted for tx {prev}"),
            );
        }
    }

    let mut client = app.lwd.clone();
    let tip = match lwd::chain_tip(&mut client).await {
        Ok(t) => t,
        Err(e) => return fail(StatusCode::BAD_GATEWAY, VerifyStatus::Error, format!("{e} ({})", app.lwd_url)),
    };
    let (data, location) = match lwd::get_transaction(&mut client, &txid).await {
        Ok(FetchedTx::Found { data, location }) => (data, location),
        Ok(FetchedTx::NotFound) => {
            return fail(
                StatusCode::NOT_FOUND,
                VerifyStatus::NotFound,
                format!("lightwalletd does not know tx {txid_hex} yet (tip {tip})"),
            )
        }
        Err(e) => return fail(StatusCode::BAD_GATEWAY, VerifyStatus::Error, format!("{e} ({})", app.lwd_url)),
    };
    let mined = match location {
        TxLocation::Mined(h) => Some(h),
        TxLocation::Mempool => None,
        TxLocation::Orphaned => {
            return fail(StatusCode::ACCEPTED, VerifyStatus::Pending, format!("tx {txid_hex} is only mined on a fork; wait"))
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
            return fail(StatusCode::UNPROCESSABLE_ENTITY, VerifyStatus::Rejected, e.to_string());
        }
    };
    let tip_u64 = u64::from(u32::from(tip));
    let mined_at = match mined {
        Some(h) => match lwd::block_time(&mut client, h).await {
            Ok(t) => Some(t),
            Err(e) => return fail(StatusCode::BAD_GATEWAY, VerifyStatus::Error, format!("block {h} time: {e} ({})", app.lwd_url)),
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
        return reply(
            StatusCode::ACCEPTED,
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
    match app.issued.bind(&ch.memo_nonce, &txid_hex, now()) {
        Ok(bound) if bound == txid_hex => {}
        Ok(bound) => {
            return fail(
                StatusCode::UNPROCESSABLE_ENTITY,
                VerifyStatus::Rejected,
                format!("memo nonce already receipted for tx {bound}"),
            )
        }
        Err(e) => return fail(StatusCode::SERVICE_UNAVAILABLE, VerifyStatus::Error, e),
    }
    let signed = receipt::sign(&app.signing_key, &claims);
    eprintln!(
        "verify {} tx {txid_hex}: CONFIRMED {} zat via {:?}, {} conf (tip {tip}); receipt issued",
        ch.memo_nonce, payment.amount_zat, payment.pools, payment.confirmations
    );
    reply(
        StatusCode::OK,
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

async fn health(State(app): State<Arc<App>>) -> Response {
    let mut client = app.lwd.clone();
    match lwd::chain_tip(&mut client).await {
        Ok(tip) => Json(serde_json::json!({
            "ok": true,
            "network": "testnet",
            "lightwalletd": app.lwd_url,
            "chainTip": u32::from(tip),
            "minConfirmations": app.min_confirmations,
            "viewingKey": if app.key.from_full_key { "ufvk (incoming part only; prefer X402Z_MERCHANT_UIVK)" } else { "uivk" },
            "receiptPubkey": hex::encode(app.signing_key.verifying_key().as_bytes()),
        }))
        .into_response(),
        Err(e) => (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({ "ok": false, "lightwalletd": app.lwd_url, "error": e.to_string() })),
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
    });
    let router = Router::new()
        .route(HEALTH_PATH, get(health))
        .route(VERIFY_PATH, post(verify_handler))
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
