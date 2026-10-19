//! Wire types for the 402 challenge returned by a paywalled endpoint.
//! Field names follow CLAUDE.md: `{scheme, network, payTo, amountZat, memoNonce, expiresAt}`.

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
        if self.expires_at <= now_unix {
            return Err(InvalidChallenge::Expired { expires_at: self.expires_at, now: now_unix });
        }
        Ok(())
    }
}
