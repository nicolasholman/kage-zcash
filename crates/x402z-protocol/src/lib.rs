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
        let n = &self.memo_nonce;
        if n.is_empty() || n.len() > MAX_NONCE_LEN || !n.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
            return Err(InvalidChallenge::BadNonce);
        }
        Ok(())
    }
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
