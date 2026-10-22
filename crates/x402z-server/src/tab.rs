//! Kage Tab: one shielded payment buys N unlinkable tokens.
//!
//! RFC 9474 blind RSA (RSABSSA-SHA384-PSS-Randomized, `blind-rsa-signatures`):
//! 1. The 402 offers `extra.tab` with the issuing key (`keyId`, SPKI) and a unit price.
//! 2. The payer pays `n * unitPriceZat` to the challenge (memo = nonce), blinds n random
//!    messages and calls `POST /tab/issue {memoNonce, txid, keyId, blinded[n]}`. The server has
//!    the facilitator confirm the payment, checks it covers n tokens, burns the nonce (once,
//!    durably) and returns n blind signatures.
//! 3. The payer unblinds them locally. Each later request spends one token with
//!    `X-PAYMENT: base64 {x402Version, scheme: "zcash-shielded-tab", network, payload: {keyId,
//!    msg, randomizer, sig}}`. The server verifies the signature and records the token as spent.
//!
//! The server signed only blinded messages, so it cannot link a token to the payment or to the
//! other tokens from the same tab. Limits (README): the anonymity set is everyone who bought
//! tokens under the same key epoch, and timing (buying then spending right away) can link them.

use std::sync::Arc;

use blind_rsa_signatures::{DefaultRng, KeyPair, MessageRandomizer, PublicKey, Randomized, SecretKey, Sha384, Signature, PSS};
use sha2::{Digest, Sha256};
use x402z_protocol::{b64, b64_any, TabOffer, TabPayload, TAB_ISSUE_PATH, TAB_SCHEME, TAB_SUITE};

use crate::store::Store;

pub type Pk = PublicKey<Sha384, PSS, Randomized>;
pub type Sk = SecretKey<Sha384, PSS, Randomized>;

/// Default `X402Z_TAB_EPOCH_SECS`: a key (and its unspent tokens) lives a week.
pub const DEFAULT_EPOCH_SECS: u64 = 7 * 24 * 60 * 60;
/// Default `X402Z_TAB_MAX_TOKENS`.
pub const DEFAULT_MAX_TOKENS: u32 = 20;
/// Default `X402Z_TAB_GRACE_SECS`: how long after its epoch the previous key still issues a tab
/// already paid under it and still accepts its tokens (two confirmation timeouts).
pub const DEFAULT_GRACE_SECS: u64 = 30 * 60;

pub struct TabKey {
    pub key_id: String,
    pub sk: Sk,
    pub pk: Pk,
    pub spki: Vec<u8>,
    pub created_at: u64,
}

fn key_id(spki: &[u8]) -> String {
    hex::encode(&Sha256::digest(spki)[..8])
}

fn from_der(der: &[u8], created_at: u64) -> Result<TabKey, String> {
    let sk = Sk::from_der(der).map_err(|e| format!("tab key in the database does not parse: {e:?}"))?;
    let pk = sk.public_key().map_err(|e| format!("tab public key: {e:?}"))?;
    let spki = pk.to_spki().map_err(|e| format!("tab public key encoding: {e:?}"))?;
    Ok(TabKey { key_id: key_id(&spki), sk, pk, spki, created_at })
}

/// Generate a fresh RSA-2048 issuing key and make it the only active one (a new epoch).
pub fn rotate(store: &Store, now: u64) -> Result<TabKey, String> {
    let kp = KeyPair::<Sha384, PSS, Randomized>::generate(&mut DefaultRng, 2048)
        .map_err(|e| format!("cannot generate the tab key: {e:?}"))?;
    let der = kp.sk.to_der().map_err(|e| format!("tab key encoding: {e:?}"))?;
    install(store, &der, now)
}

/// Install a given secret key (DER) as the active key.
pub fn install(store: &Store, der: &[u8], now: u64) -> Result<TabKey, String> {
    let k = from_der(der, now)?;
    store.install_tab_key(&k.key_id, der, now)?;
    Ok(k)
}

/// The active key, rotating it when its epoch is over.
pub fn load_or_rotate(store: &Store, now: u64, epoch_secs: u64) -> Result<TabKey, String> {
    match store.active_tab_key()? {
        Some((_, der, created)) if now < created.saturating_add(epoch_secs) => from_der(&der, created),
        _ => rotate(store, now),
    }
}

/// The previous key if it is still inside its grace period: it stopped selling at the earlier of
/// its epoch end and its retirement, and tabs paid under it are honoured `grace_secs` past that.
pub fn previous_in_grace(store: &Store, now: u64, epoch_secs: u64, grace_secs: u64) -> Result<Option<TabKey>, String> {
    if grace_secs == 0 {
        return Ok(None);
    }
    match store.previous_tab_key()? {
        Some((_, der, created, retired)) if now < created.saturating_add(epoch_secs).min(retired).saturating_add(grace_secs) => {
            from_der(&der, created).map(Some)
        }
        _ => Ok(None),
    }
}

pub fn offer(k: &TabKey, unit_price_zat: u64, max_tokens: u32, epoch_secs: u64, grace_secs: u64) -> TabOffer {
    TabOffer {
        scheme: TAB_SCHEME.into(),
        suite: TAB_SUITE.into(),
        key_id: k.key_id.clone(),
        public_key: b64(&k.spki),
        unit_price_zat,
        max_tokens,
        issue_path: TAB_ISSUE_PATH.into(),
        epoch_ends_at: k.created_at.saturating_add(epoch_secs),
        grace_secs,
    }
}

/// A token's identity in the spent set: SHA-256(randomizer || msg).
pub fn token_id(randomizer: &[u8; 32], msg: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(randomizer);
    h.update(msg);
    hex::encode(h.finalize())
}

/// Why a tab request was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TabError {
    Malformed(String),
    /// Token signed under another (retired or unknown) key epoch.
    OtherEpoch { got: String, active: String },
    BadSignature,
    DoubleSpent,
    Storage(String),
}

impl TabError {
    pub fn reason(&self) -> String {
        match self {
            TabError::Malformed(m) => format!("tab token malformed: {m}"),
            TabError::OtherEpoch { got, active } => {
                format!("tab token is from key {got}, not the active key {active} (another epoch)")
            }
            TabError::BadSignature => "tab token signature invalid".into(),
            TabError::DoubleSpent => "tab token already spent".into(),
            TabError::Storage(e) => e.clone(),
        }
    }
}

/// Sign `n` blinded messages (already length-checked by the caller via `parse_blinded`).
pub fn blind_sign_all(k: &TabKey, blinded: &[Vec<u8>]) -> Result<Vec<String>, String> {
    blinded
        .iter()
        .map(|m| k.sk.blind_sign(m).map(|s| b64(&s)).map_err(|e| format!("blind signing failed: {e:?}")))
        .collect()
}

/// Decode and size-check blinded messages for key `k`.
pub fn parse_blinded(k: &TabKey, blinded: &[String]) -> Result<Vec<Vec<u8>>, String> {
    let modulus_bytes = 256; // RSA-2048
    blinded
        .iter()
        .enumerate()
        .map(|(i, b)| match b64_any(b) {
            Some(raw) if raw.len() == modulus_bytes => Ok(raw),
            Some(raw) => Err(format!("blinded[{i}] is {} bytes, expected {modulus_bytes} for key {}", raw.len(), k.key_id)),
            None => Err(format!("blinded[{i}] is not base64")),
        })
        .collect()
}

/// Verify a token and burn it. Returns its token id.
pub fn spend(store: &Store, k: &Arc<TabKey>, p: &TabPayload, now: u64) -> Result<String, TabError> {
    if p.key_id != k.key_id {
        return Err(TabError::OtherEpoch { got: p.key_id.clone(), active: k.key_id.clone() });
    }
    let msg = b64_any(&p.msg).ok_or_else(|| TabError::Malformed("msg is not base64".into()))?;
    let r: [u8; 32] = b64_any(&p.randomizer)
        .and_then(|r| r.try_into().ok())
        .ok_or_else(|| TabError::Malformed("randomizer must be 32 bytes of base64".into()))?;
    let sig = b64_any(&p.sig).ok_or_else(|| TabError::Malformed("sig is not base64".into()))?;
    k.pk.verify(&Signature(sig), Some(MessageRandomizer(r)), &msg).map_err(|_| TabError::BadSignature)?;
    let id = token_id(&r, &msg);
    match store.spend_token(&id, &k.key_id, now) {
        Ok(true) => Ok(id),
        Ok(false) => Err(TabError::DoubleSpent),
        Err(e) => Err(TabError::Storage(e)),
    }
}
