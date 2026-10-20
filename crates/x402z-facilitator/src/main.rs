//! Facilitator: merchant UFVK only (no spend key). Fetches a payer's transaction from
//! lightwalletd, trial-decrypts it with the UFVK, checks it against the 402 challenge and
//! signs an ed25519 receipt for `X-PAYMENT-RECEIPT`.
//!
//! `GET /health`  live lightwalletd round-trip; 200 with the chain tip, 503 if it fails.
//! `POST /verify` `{challenge, txid}` -> `VerifyResponse` (see x402z_protocol).
//!
//! `x402z-facilitator keygen` prints a fresh receipt key pair for `.env`.
use std::{
    collections::HashMap,
    process::exit,
    sync::{Arc, Mutex},
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
    verify::{self, VerifyError},
    NETWORK,
};
use x402z_protocol::{ReceiptClaims, VerifyRequest, VerifyResponse, VerifyStatus, HEALTH_PATH, VERIFY_PATH};
use zcash_keys::keys::UnifiedFullViewingKey;

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
    ufvk: UnifiedFullViewingKey,
    signing_key: SigningKey,
    /// When set, challenges for any other `payTo` are refused outright.
    merchant_ua: Option<String>,
    min_confirmations: u32,
    /// memo nonce -> txid it was receipted for, so one nonce can't be receipted for two txs.
    issued: Mutex<HashMap<String, String>>,
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
    if let Err(e) = ch.validate(now()) {
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
    if let Some(prev) = app.issued.lock().unwrap().get(&ch.memo_nonce) {
        if prev != &txid_hex {
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
        verify::verify_payment(&NETWORK, &app.ufvk, ch, &tx, mined, tip)
    });
    let payment = match result {
        Ok(p) => p,
        Err(e) => {
            eprintln!("verify {} tx {txid_hex}: rejected: {e}", ch.memo_nonce);
            return fail(StatusCode::UNPROCESSABLE_ENTITY, VerifyStatus::Rejected, e.to_string());
        }
    };
    let tip_u64 = u64::from(u32::from(tip));
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
    };
    let signed = receipt::sign(&app.signing_key, &claims);
    app.issued.lock().unwrap().insert(ch.memo_nonce.clone(), txid_hex.clone());
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
        Some(other) => {
            eprintln!("error: unknown command '{other}' (usage: x402z-facilitator [serve|keygen])");
            exit(2);
        }
    }

    let lwd_url = optional("ZCASH_LWD_URL").unwrap_or_else(|| {
        eprintln!("ZCASH_LWD_URL not set; using {}", lwd::DEFAULT_LWD_URL);
        lwd::DEFAULT_LWD_URL.to_string()
    });
    let ufvk = UnifiedFullViewingKey::decode(&NETWORK, &require("X402Z_MERCHANT_UFVK")).unwrap_or_else(|e| {
        eprintln!("error: X402Z_MERCHANT_UFVK is not a testnet unified full viewing key (uviewtest1...): {e}");
        exit(2);
    });
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
        ufvk,
        signing_key,
        merchant_ua,
        min_confirmations,
        issued: Mutex::new(HashMap::new()),
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
