//! Wire types for the 402 challenge returned by a paywalled endpoint.
//! Field names: `{scheme, network, payTo, amountZat, memoNonce, expiresAt}`.

use serde::{Deserialize, Serialize};

pub const SCHEME: &str = "zcash-shielded";
pub const RECEIPT_HEADER: &str = "X-PAYMENT-RECEIPT";
/// Zcash memos are 512 bytes; the nonce must fit with room to spare.
pub const MAX_NONCE_LEN: usize = 64;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PaymentRequired {
    pub scheme: String,
    pub network: Network,
    /// Merchant unified address.
    pub pay_to: String,
    pub amount_zat: u64,
    pub memo_nonce: String,
    /// Unix seconds.
    pub expires_at: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Network {
    Testnet,
    Mainnet,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InvalidChallenge {
    WrongScheme(String),
    MainnetNotAllowed,
    BadAddress,
    ZeroAmount,
    BadNonce,
    Expired { expires_at: u64, now: u64 },
}

impl PaymentRequired {
    /// Validate a challenge before the agent considers paying it.
    /// Address check is a prefix sanity check only (testnet UAs start `utest1`);
    /// full UA decoding happens in the agent with `zcash_keys`.
    pub fn validate(&self, now_unix: u64) -> Result<(), InvalidChallenge> {
        self.validate_shape()?;
        if self.expires_at <= now_unix {
            return Err(InvalidChallenge::Expired { expires_at: self.expires_at, now: now_unix });
        }
        Ok(())
    }

    /// Everything [`validate`](Self::validate) checks except expiry. The facilitator uses this:
    /// whether a payment was in time is decided by the time of the block that mined it, not by
    /// when the payer asks for a receipt.
    pub fn validate_shape(&self) -> Result<(), InvalidChallenge> {
        if self.scheme != SCHEME {
            return Err(InvalidChallenge::WrongScheme(self.scheme.clone()));
        }
        if self.network != Network::Testnet {
            return Err(InvalidChallenge::MainnetNotAllowed);
        }
        if !self.pay_to.starts_with("utest1") || self.pay_to.len() < 20 {
            return Err(InvalidChallenge::BadAddress);
        }
        if self.amount_zat == 0 {
            return Err(InvalidChallenge::ZeroAmount);
        }
        if !valid_nonce(&self.memo_nonce) {
            return Err(InvalidChallenge::BadNonce);
        }
        Ok(())
    }
}

/// A memo nonce as challenges carry it: 1 to [`MAX_NONCE_LEN`] characters of `[A-Za-z0-9_-]`.
/// Servers check this before a client-supplied nonce reaches a database, a facilitator or a log.
pub fn valid_nonce(n: &str) -> bool {
    !n.is_empty() && n.len() <= MAX_NONCE_LEN && n.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// A display-order transaction id: exactly 64 hex characters.
pub fn valid_txid(t: &str) -> bool {
    t.len() == 64 && t.chars().all(|c| c.is_ascii_hexdigit())
}

/// Was a payment made in time? `paid_at` is the mined block's time (or, for a mempool-only
/// payment, the time it was seen). A payment counts if it landed no later than
/// `expires_at + grace_secs`: the grace absorbs block-time jitter and slow blocks, so a payer
/// who broadcast before expiry does not lose the money because the block came late.
pub fn paid_in_time(expires_at: u64, paid_at: u64, grace_secs: u64) -> bool {
    paid_at <= expires_at.saturating_add(grace_secs)
}

/// Default for `X402Z_EXPIRY_GRACE_SECS` (facilitator and server): 20 minutes, about 16
/// testnet blocks.
pub const DEFAULT_EXPIRY_GRACE_SECS: u64 = 20 * 60;

/// Claims the facilitator signs once it has seen the payment with the merchant UFVK.
/// Wire format of `X-PAYMENT-RECEIPT`: `base64url(claims_json) "." base64url(ed25519_sig)`
/// (no padding); the signature covers the exact decoded `claims_json` bytes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReceiptClaims {
    pub memo_nonce: String,
    pub amount_zat: u64,
    pub pay_to: String,
    pub txid: String,
    /// 0 = seen in the mempool only.
    pub confirmations: u32,
    pub issued_at: u64,
    /// Unix time of the block that mined the payment (block header time). Absent for a
    /// mempool-only payment and in receipts from older facilitators.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mined_at: Option<u64>,
}

/// Facilitator endpoint the payer posts its txid to, and the liveness probe.
pub const VERIFY_PATH: &str = "/verify";
pub const HEALTH_PATH: &str = "/health";

/// `POST /verify` body: the 402 challenge that was paid and the (display-order hex) txid.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct VerifyRequest {
    pub challenge: PaymentRequired,
    pub txid: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VerifyStatus {
    /// 200: receipt issued.
    Confirmed,
    /// 202: payment found and valid but below the facilitator's confirmation policy; retry.
    Pending,
    /// 404: lightwalletd does not know the txid (yet); retry shortly after broadcast.
    NotFound,
    /// 422 / 400: the transaction does not pay this challenge. Do not retry.
    Rejected,
    /// 502: lightwalletd failed or is unreachable.
    Error,
}

/// `POST /verify` response body (every status).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VerifyResponse {
    pub status: VerifyStatus,
    /// `X-PAYMENT-RECEIPT` value, present when `status == confirmed`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub receipt: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub claims: Option<ReceiptClaims>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confirmations: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_confirmations: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chain_tip: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

// ---------------------------------------------------------------------------------------------
// x402 v1 wire format.
//
// Follows coinbase/x402 `specs/x402-specification-v1.md` and `specs/transports-v1/http.md`
// (read 2026-10-21): a 402 answers with a `PaymentRequiredResponse` body
// `{x402Version, accepts: [PaymentRequirements], error}`; the client retries with an
// `X-PAYMENT` header holding base64 JSON `PaymentPayload {x402Version, scheme, network,
// payload}`; the server answers with an `X-PAYMENT-RESPONSE` header holding base64 JSON
// `SettlementResponse {success, errorReason?, transaction, network, payer}`. Facilitators
// expose `POST /verify`, `POST /settle` (`{x402Version, paymentPayload, paymentRequirements}`)
// and `GET /supported` (`{kinds: [{x402Version, scheme, network}]}`).
//
// Zcash specifics (scheme `zcash-shielded`, network `zcash-testnet`, asset `ZEC`, amounts in
// zatoshi): the payload is `{txid, memoNonce}`. A shielded transaction tells the server nothing
// it can look up, so the payer names the challenge it paid (`memoNonce`); the facilitator then
// checks that same nonce inside the encrypted memo, so it can't be pointed at another challenge.
// `payer` in a settlement response is always "" because nobody learns the payer's address.
// ---------------------------------------------------------------------------------------------

use base64::{
    engine::general_purpose::{STANDARD, URL_SAFE, URL_SAFE_NO_PAD},
    Engine,
};

pub const X402_VERSION: u32 = 1;
pub const PAYMENT_HEADER: &str = "X-PAYMENT";
pub const PAYMENT_RESPONSE_HEADER: &str = "X-PAYMENT-RESPONSE";
/// x402 network id for Zcash testnet.
pub const NETWORK_TESTNET: &str = "zcash-testnet";
pub const ASSET_ZEC: &str = "ZEC";
pub const SETTLE_PATH: &str = "/settle";
pub const SUPPORTED_PATH: &str = "/supported";

/// One entry of `accepts` (x402 `PaymentRequirements`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PaymentRequirements {
    pub scheme: String,
    pub network: String,
    /// Atomic units (zatoshi) as a decimal string, as the spec requires.
    pub max_amount_required: String,
    pub resource: String,
    pub description: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mime_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_schema: Option<serde_json::Value>,
    pub pay_to: String,
    pub max_timeout_seconds: u64,
    pub asset: String,
    /// Scheme-specific; for `zcash-shielded` a [`ZcashExtra`]. Kept as raw JSON so a 402 that
    /// also offers other schemes (EVM `exact` with `{name, version}`) still parses.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extra: Option<serde_json::Value>,
}

/// `extra` for `zcash-shielded`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ZcashExtra {
    /// Text memo the payment must carry.
    pub memo_nonce: String,
    /// Unix seconds; the payment must be mined by then (plus the server's grace).
    pub expires_at: u64,
    /// ZIP 321 payment URI for any Zcash wallet (`zcash:<ua>?amount=…&memo=…`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub payment_uri: Option<String>,
    /// Scheme-specific additions (for example the Kage Tab offer).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tab: Option<serde_json::Value>,
}

/// The 402 response body.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PaymentRequiredResponse {
    pub x402_version: u32,
    pub accepts: Vec<PaymentRequirements>,
    pub error: String,
}

/// `X-PAYMENT` (base64 JSON).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PaymentPayload {
    pub x402_version: u32,
    pub scheme: String,
    pub network: String,
    pub payload: serde_json::Value,
}

/// `payload` of a `zcash-shielded` [`PaymentPayload`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ZcashPayload {
    /// Display-order (explorer) hex txid.
    pub txid: String,
    /// `extra.memoNonce` of the challenge this transaction pays.
    pub memo_nonce: String,
}

/// `X-PAYMENT-RESPONSE` (base64 JSON), and the facilitator's `/settle` answer.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SettlementResponse {
    pub success: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_reason: Option<String>,
    /// Txid; "" if settlement failed.
    pub transaction: String,
    pub network: String,
    /// Always "": a shielded payer is unknown to everyone, including the merchant.
    pub payer: String,
}

/// Facilitator `/verify` and `/settle` request body.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FacilitatorRequest {
    pub x402_version: u32,
    pub payment_payload: PaymentPayload,
    pub payment_requirements: PaymentRequirements,
}

/// Facilitator `/settle` answer: the x402 settlement fields plus Kage's signed receipt
/// (which the resource server checks against the facilitator's ed25519 key) and status detail.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SettleResponse {
    #[serde(flatten)]
    pub settlement: SettlementResponse,
    pub status: VerifyStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub receipt: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub claims: Option<ReceiptClaims>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confirmations: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_confirmations: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Facilitator `/verify` answer (x402 shape, read-only: no receipt is issued).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct X402VerifyResponse {
    pub is_valid: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub invalid_reason: Option<String>,
    pub payer: String,
    pub status: VerifyStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SupportedKind {
    pub x402_version: u32,
    pub scheme: String,
    pub network: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SupportedResponse {
    pub kinds: Vec<SupportedKind>,
}

/// Machine-readable reasons used in `errorReason` / `invalidReason`.
pub mod reason {
    pub const INVALID_PAYLOAD: &str = "invalid_payload";
    pub const INVALID_SCHEME: &str = "invalid_scheme";
    pub const INVALID_NETWORK: &str = "invalid_network";
    pub const INVALID_X402_VERSION: &str = "invalid_x402_version";
    pub const UNKNOWN_CHALLENGE: &str = "unknown_challenge";
    pub const NOT_FOUND: &str = "payment_not_found";
    pub const PENDING: &str = "settlement_pending";
    pub const INVALID_PAYMENT: &str = "invalid_payment";
    pub const EXPIRED: &str = "payment_expired";
    pub const NONCE_USED: &str = "nonce_already_used";
    pub const FACILITATOR_ERROR: &str = "facilitator_error";
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WireError {
    Base64,
    Json(String),
    Version(u32),
    Scheme(String),
    Network(String),
    Amount(String),
    MissingExtra,
}

impl std::fmt::Display for WireError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            WireError::Base64 => write!(f, "not base64"),
            WireError::Json(e) => write!(f, "not the expected JSON: {e}"),
            WireError::Version(v) => write!(f, "x402Version {v} is not supported (expected {X402_VERSION})"),
            WireError::Scheme(s) => write!(f, "scheme '{s}' is not supported (expected '{SCHEME}')"),
            WireError::Network(n) => write!(f, "network '{n}' is not supported (expected '{NETWORK_TESTNET}')"),
            WireError::Amount(a) => write!(f, "maxAmountRequired '{a}' is not a zatoshi integer"),
            WireError::MissingExtra => write!(f, "extra.memoNonce / extra.expiresAt missing"),
        }
    }
}

impl WireError {
    pub fn reason(&self) -> &'static str {
        match self {
            WireError::Version(_) => reason::INVALID_X402_VERSION,
            WireError::Scheme(_) => reason::INVALID_SCHEME,
            WireError::Network(_) => reason::INVALID_NETWORK,
            _ => reason::INVALID_PAYLOAD,
        }
    }
}

/// Base64 (standard alphabet, padded) of the JSON, as x402 headers carry it.
pub fn encode_header<T: Serialize>(v: &T) -> String {
    STANDARD.encode(serde_json::to_vec(v).expect("serializable"))
}

/// Decode an x402 header. Accepts standard and URL-safe base64, padded or not.
pub fn decode_header<T: serde::de::DeserializeOwned>(h: &str) -> Result<T, WireError> {
    let h = h.trim();
    let raw = STANDARD
        .decode(h)
        .or_else(|_| URL_SAFE.decode(h))
        .or_else(|_| URL_SAFE_NO_PAD.decode(h))
        .or_else(|_| base64::engine::general_purpose::STANDARD_NO_PAD.decode(h))
        .map_err(|_| WireError::Base64)?;
    serde_json::from_slice(&raw).map_err(|e| WireError::Json(e.to_string()))
}

impl PaymentRequirements {
    /// The x402 form of an internal challenge.
    pub fn from_challenge(ch: &PaymentRequired, resource: &str, description: &str, mime_type: &str, max_timeout_seconds: u64) -> Self {
        Self {
            scheme: ch.scheme.clone(),
            network: match ch.network {
                Network::Testnet => NETWORK_TESTNET.into(),
                Network::Mainnet => "zcash-mainnet".into(),
            },
            max_amount_required: ch.amount_zat.to_string(),
            resource: resource.into(),
            description: description.into(),
            mime_type: Some(mime_type.into()),
            output_schema: None,
            pay_to: ch.pay_to.clone(),
            max_timeout_seconds,
            asset: ASSET_ZEC.into(),
            extra: Some(
                serde_json::to_value(ZcashExtra {
                    memo_nonce: ch.memo_nonce.clone(),
                    expires_at: ch.expires_at,
                    payment_uri: None,
                    tab: None,
                })
                .expect("serializable"),
            ),
        }
    }

    pub fn zcash_extra(&self) -> Result<ZcashExtra, WireError> {
        let v = self.extra.clone().ok_or(WireError::MissingExtra)?;
        serde_json::from_value(v).map_err(|_| WireError::MissingExtra)
    }

    pub fn set_zcash_extra(&mut self, extra: &ZcashExtra) {
        self.extra = Some(serde_json::to_value(extra).expect("serializable"));
    }

    /// Back to the internal challenge (scheme, network, amount and extra checked).
    pub fn to_challenge(&self) -> Result<PaymentRequired, WireError> {
        if self.scheme != SCHEME {
            return Err(WireError::Scheme(self.scheme.clone()));
        }
        if self.network != NETWORK_TESTNET {
            return Err(WireError::Network(self.network.clone()));
        }
        let amount_zat = self
            .max_amount_required
            .parse::<u64>()
            .map_err(|_| WireError::Amount(self.max_amount_required.clone()))?;
        let extra = self.zcash_extra()?;
        Ok(PaymentRequired {
            scheme: self.scheme.clone(),
            network: Network::Testnet,
            pay_to: self.pay_to.clone(),
            amount_zat,
            memo_nonce: extra.memo_nonce,
            expires_at: extra.expires_at,
        })
    }
}

impl PaymentPayload {
    pub fn zcash(txid: &str, memo_nonce: &str) -> Self {
        Self {
            x402_version: X402_VERSION,
            scheme: SCHEME.into(),
            network: NETWORK_TESTNET.into(),
            payload: serde_json::to_value(ZcashPayload { txid: txid.into(), memo_nonce: memo_nonce.into() })
                .expect("serializable"),
        }
    }

    /// Check version/scheme/network and extract the Zcash payload.
    pub fn zcash_payload(&self) -> Result<ZcashPayload, WireError> {
        if self.x402_version != X402_VERSION {
            return Err(WireError::Version(self.x402_version));
        }
        if self.scheme != SCHEME {
            return Err(WireError::Scheme(self.scheme.clone()));
        }
        if self.network != NETWORK_TESTNET {
            return Err(WireError::Network(self.network.clone()));
        }
        serde_json::from_value(self.payload.clone()).map_err(|e| WireError::Json(e.to_string()))
    }
}

impl PaymentRequiredResponse {
    /// The first `accepts` entry this crate can pay (`zcash-shielded` on `zcash-testnet`).
    pub fn zcash_option(&self) -> Option<&PaymentRequirements> {
        self.accepts.iter().find(|a| a.scheme == SCHEME && a.network == NETWORK_TESTNET)
    }
}

impl SettlementResponse {
    pub fn ok(txid: &str) -> Self {
        Self { success: true, error_reason: None, transaction: txid.into(), network: NETWORK_TESTNET.into(), payer: String::new() }
    }

    pub fn failed(reason: &str) -> Self {
        Self {
            success: false,
            error_reason: Some(reason.into()),
            transaction: String::new(),
            network: NETWORK_TESTNET.into(),
            payer: String::new(),
        }
    }
}

// ---------------------------------------------------------------------------------------------
// ZIP 321 payment URIs (https://zips.z.cash/zip-0321), so any Zcash wallet can pay a challenge.
// ---------------------------------------------------------------------------------------------

/// Zatoshi as a ZIP 321 `amount`: decimal ZEC, at most 8 fractional digits, no trailing zeros
/// and no trailing ".".
pub fn zec_decimal(zat: u64) -> String {
    let (whole, frac) = (zat / 100_000_000, zat % 100_000_000);
    if frac == 0 {
        return whole.to_string();
    }
    let f = format!("{frac:08}");
    format!("{whole}.{}", f.trim_end_matches('0'))
}

/// `zcash:<address>?amount=<ZEC>&memo=<base64url(memo)>`. ZIP 321: the memo parameter is the
/// memo field contents "encoded as base64url without `=` padding"; for a text memo that is its
/// UTF-8 bytes.
pub fn zip321_uri(pay_to: &str, amount_zat: u64, memo_text: &str) -> String {
    format!("zcash:{pay_to}?amount={}&memo={}", zec_decimal(amount_zat), URL_SAFE_NO_PAD.encode(memo_text.as_bytes()))
}

/// The ZIP 321 URI for a challenge.
pub fn challenge_uri(ch: &PaymentRequired) -> String {
    zip321_uri(&ch.pay_to, ch.amount_zat, &ch.memo_nonce)
}

// ---------------------------------------------------------------------------------------------
// Kage Tab: one shielded payment buys N unlinkable RFC 9474 blind-RSA tokens.
// ---------------------------------------------------------------------------------------------

/// `X-PAYMENT` scheme for spending one tab token.
pub const TAB_SCHEME: &str = "zcash-shielded-tab";
/// RFC 9474 variant used for tab tokens.
pub const TAB_SUITE: &str = "RSABSSA-SHA384-PSS-Randomized";
pub const TAB_ISSUE_PATH: &str = "/tab/issue";

/// `extra.tab` in a 402: pay `n * unitPriceZat` (n <= maxTokens) to the challenge, then
/// `POST /tab/issue` with n blinded messages for the key `keyId`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TabOffer {
    pub scheme: String,
    pub suite: String,
    pub key_id: String,
    /// Base64 (standard) DER SubjectPublicKeyInfo of the RSA-2048 issuing key.
    pub public_key: String,
    pub unit_price_zat: u64,
    pub max_tokens: u32,
    pub issue_path: String,
    /// Unix seconds after which this key's unspent tokens stop being accepted.
    pub epoch_ends_at: u64,
}

/// `POST /tab/issue` body. Blinded messages are base64 (standard or URL-safe).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TabIssueRequest {
    pub memo_nonce: String,
    pub txid: String,
    pub key_id: String,
    pub blinded: Vec<String>,
}

/// `POST /tab/issue` answer: one blind signature per blinded message, same order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TabIssueResponse {
    pub key_id: String,
    pub blind_signatures: Vec<String>,
}

/// `payload` of an `X-PAYMENT` with scheme `zcash-shielded-tab`. All byte fields base64.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TabPayload {
    pub key_id: String,
    pub msg: String,
    /// The 32-byte RFC 9474 message randomizer from blinding.
    pub randomizer: String,
    pub sig: String,
}

impl PaymentPayload {
    pub fn tab(p: &TabPayload) -> Self {
        Self {
            x402_version: X402_VERSION,
            scheme: TAB_SCHEME.into(),
            network: NETWORK_TESTNET.into(),
            payload: serde_json::to_value(p).expect("serializable"),
        }
    }

    pub fn tab_payload(&self) -> Result<TabPayload, WireError> {
        if self.x402_version != X402_VERSION {
            return Err(WireError::Version(self.x402_version));
        }
        if self.scheme != TAB_SCHEME {
            return Err(WireError::Scheme(self.scheme.clone()));
        }
        if self.network != NETWORK_TESTNET {
            return Err(WireError::Network(self.network.clone()));
        }
        serde_json::from_value(self.payload.clone()).map_err(|e| WireError::Json(e.to_string()))
    }
}

/// Decode base64 in any of the common alphabets/paddings.
pub fn b64_any(s: &str) -> Option<Vec<u8>> {
    let s = s.trim();
    STANDARD.decode(s).or_else(|_| URL_SAFE.decode(s)).or_else(|_| URL_SAFE_NO_PAD.decode(s)).or_else(|_| base64::engine::general_purpose::STANDARD_NO_PAD.decode(s)).ok()
}

pub fn b64(bytes: &[u8]) -> String {
    STANDARD.encode(bytes)
}
