//! Does a transaction pay a 402 challenge? Decided by trial decryption with the merchant's
//! unified *incoming* viewing key (UIVK), the least key that can see payments.
//!
//! An output counts toward the challenge only if it
//! 1. decrypts with the merchant's *external* incoming viewing key for its pool (a real incoming
//!    payment; change and outgoing outputs never decrypt with this key),
//! 2. carries a text memo exactly equal to `memoNonce`, and
//! 3. was sent to the receiver of `payTo` for its pool (Sapling receiver for Sapling notes,
//!    the Orchard receiver for Orchard and Ironwood notes, which share it).
//!
//! The values of all such outputs are summed and compared to `amountZat`.
//!
//! A full viewing key (UFVK) is accepted for compatibility, but only its incoming part is used:
//! [`MerchantKey::from_ufvk`] keeps the derived UIVK and drops the rest.

use orchard::note_encryption::{DomainVersion, IronwoodVersion, NoteEncryptionDomain, OrchardVersion};
use sapling::note_encryption::SaplingDomain;
use zcash_keys::{
    address::Address,
    keys::{UnifiedFullViewingKey, UnifiedIncomingViewingKey},
};
use zcash_note_encryption::try_note_decryption;
use zcash_primitives::transaction::{components::sapling::zip212_enforcement, Transaction};
use zcash_protocol::{
    consensus::{BlockHeight, BranchId, Parameters},
    memo::{Memo, MemoBytes},
    ShieldedPool,
};
use x402z_protocol::PaymentRequired;

/// The merchant key the facilitator verifies with: always a unified incoming viewing key.
#[derive(Debug, Clone)]
pub struct MerchantKey {
    uivk: UnifiedIncomingViewingKey,
    /// True when it was derived from a full viewing key (the operator gave more than needed).
    pub from_full_key: bool,
}

impl MerchantKey {
    pub fn from_uivk(uivk: UnifiedIncomingViewingKey) -> Self {
        Self { uivk, from_full_key: false }
    }

    /// Keep only the external incoming part of `ufvk`; the full key is not retained.
    pub fn from_ufvk(ufvk: &UnifiedFullViewingKey) -> Self {
        Self { uivk: ufvk.to_unified_incoming_viewing_key(), from_full_key: true }
    }

    /// Decode a `uivktest1...` string.
    pub fn decode_uivk<P: Parameters>(params: &P, s: &str) -> Result<Self, String> {
        UnifiedIncomingViewingKey::decode(params, s.trim()).map(Self::from_uivk).map_err(|e| e.to_string())
    }

    /// Decode a `uviewtest1...` string and reduce it to its incoming part.
    pub fn decode_ufvk<P: Parameters>(params: &P, s: &str) -> Result<Self, String> {
        UnifiedFullViewingKey::decode(params, s.trim()).map(|k| Self::from_ufvk(&k)).map_err(|e| e.to_string())
    }

    pub fn uivk(&self) -> &UnifiedIncomingViewingKey {
        &self.uivk
    }
}

/// One output that decrypted with the merchant's incoming viewing key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IncomingOutput {
    pub pool: ShieldedPool,
    pub value_zat: u64,
    /// Raw 512-byte memo field.
    pub memo: [u8; 512],
    pub recipient: Recipient,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Recipient {
    Sapling(sapling::PaymentAddress),
    /// Orchard and Ironwood notes (same receiver type).
    Orchard(orchard::Address),
}

/// The memo as text, if it is a text memo.
pub fn memo_text(memo: &[u8; 512]) -> Option<String> {
    match MemoBytes::from_bytes(memo).ok().and_then(|b| Memo::try_from(&b).ok()) {
        Some(Memo::Text(t)) => Some(t.to_string()),
        _ => None,
    }
}

fn orchard_family<V: DomainVersion>(
    pool: ShieldedPool,
    ivk: &orchard::keys::IncomingViewingKey,
    bundle: &orchard::bundle::Bundle<orchard::bundle::Authorized, zcash_protocol::value::ZatBalance>,
    out: &mut Vec<IncomingOutput>,
) {
    let p = orchard::keys::PreparedIncomingViewingKey::new(ivk);
    for a in bundle.actions() {
        let d = NoteEncryptionDomain::<V>::for_action(a);
        if let Some((note, addr, memo)) = try_note_decryption(&d, &p, a) {
            out.push(IncomingOutput { pool, value_zat: note.value().inner(), memo, recipient: Recipient::Orchard(addr) });
        }
    }
}

/// Trial-decrypt every Sapling, Orchard and Ironwood output of `tx` with the incoming viewing
/// key, per pool. `height` is the mined height (or the next block for a mempool tx); it picks
/// the Sapling ZIP 212 rules.
pub fn incoming_outputs<P: Parameters>(
    params: &P,
    uivk: &UnifiedIncomingViewingKey,
    tx: &Transaction,
    height: BlockHeight,
) -> Vec<IncomingOutput> {
    let mut out = Vec::new();
    if let (Some(b), Some(ivk)) = (tx.sapling_bundle(), uivk.sapling()) {
        let d = SaplingDomain::new(zip212_enforcement(params, height));
        let p = ivk.prepare();
        for o in b.shielded_outputs() {
            if let Some((note, addr, memo)) = try_note_decryption(&d, &p, o) {
                out.push(IncomingOutput {
                    pool: ShieldedPool::Sapling,
                    value_zat: note.value().inner(),
                    memo,
                    recipient: Recipient::Sapling(addr),
                });
            }
        }
    }
    if let Some(ivk) = uivk.orchard() {
        if let Some(b) = tx.orchard_bundle() {
            orchard_family::<OrchardVersion>(ShieldedPool::Orchard, ivk, b, &mut out);
        }
        if let Some(b) = tx.ironwood_bundle() {
            orchard_family::<IronwoodVersion>(ShieldedPool::Ironwood, ivk, b, &mut out);
        }
    }
    out
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Payment {
    pub amount_zat: u64,
    /// 0 when the transaction is only in the mempool.
    pub confirmations: u32,
    pub pools: Vec<ShieldedPool>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VerifyError {
    /// `payTo` is not a shielded-only testnet unified address (no shielded receiver, or it has a transparent one).
    BadPayTo,
    /// Bytes from lightwalletd did not parse as a transaction for this branch.
    Unparseable(String),
    /// The parsed transaction's id differs from the one asked for.
    TxidMismatch { asked: String, got: String },
    /// Nothing in the transaction decrypts to the merchant with this memo nonce.
    NoMatchingOutput,
    /// Outputs with the right memo decrypt to the merchant, but at a different receiver than `payTo`.
    WrongRecipient,
    Underpaid { paid: u64, want: u64 },
}

impl std::fmt::Display for VerifyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            VerifyError::BadPayTo => write!(f, "payTo is not a shielded-only testnet unified address (needs a Sapling or Orchard receiver and no transparent receiver)"),
            VerifyError::Unparseable(e) => write!(f, "transaction did not parse: {e}"),
            VerifyError::TxidMismatch { asked, got } => write!(f, "lightwalletd returned tx {got} for {asked}"),
            VerifyError::NoMatchingOutput => {
                write!(f, "no output in this transaction decrypts to the merchant viewing key with this memo nonce")
            }
            VerifyError::WrongRecipient => write!(f, "memo matches but the note was sent to a different address than payTo"),
            VerifyError::Underpaid { paid, want } => write!(f, "underpaid: {paid} zat < {want} zat"),
        }
    }
}

impl std::error::Error for VerifyError {}

/// Confirmations of a tx mined at `mined` when the tip is `tip` (the mining block counts as 1).
pub fn confirmations(mined: Option<BlockHeight>, tip: BlockHeight) -> u32 {
    match mined {
        None => 0,
        Some(h) => (u32::from(tip).saturating_sub(u32::from(h))).saturating_add(1),
    }
}

/// Parse raw transaction bytes. The consensus branch comes from the mined height, or the next
/// block for a mempool transaction.
pub fn parse_tx<P: Parameters>(
    params: &P,
    data: &[u8],
    mined: Option<BlockHeight>,
    tip: BlockHeight,
) -> Result<Transaction, VerifyError> {
    let height = mined.unwrap_or(tip + 1);
    Transaction::read(data, BranchId::for_height(params, height)).map_err(|e| VerifyError::Unparseable(e.to_string()))
}

/// Check that `tx` pays `challenge`, seen through the merchant's incoming viewing key only.
/// Does not check expiry or confirmation policy; the caller owns those (they depend on
/// wall-clock time and configuration).
pub fn verify_payment<P: Parameters>(
    params: &P,
    key: &MerchantKey,
    challenge: &PaymentRequired,
    tx: &Transaction,
    mined: Option<BlockHeight>,
    tip: BlockHeight,
) -> Result<Payment, VerifyError> {
    let Some(Address::Unified(ua)) = Address::decode(params, &challenge.pay_to) else {
        return Err(VerifyError::BadPayTo);
    };
    // Shielded-only: a payTo with a transparent receiver is refused outright, so a payer can
    // never be steered to (or credited for) a transparent output. Transparent outputs in the
    // tx are never counted either: only Sapling/Orchard/Ironwood notes are credited below.
    if ua.sapling().is_none() && ua.orchard().is_none() || ua.transparent().is_some() {
        return Err(VerifyError::BadPayTo);
    }

    let nonce = challenge.memo_nonce.as_str();
    let mut memo_seen = false;
    let mut paid: u64 = 0;
    let mut pools = Vec::new();
    for o in incoming_outputs(params, key.uivk(), tx, mined.unwrap_or(tip + 1)) {
        if memo_text(&o.memo).as_deref() != Some(nonce) {
            continue;
        }
        memo_seen = true;
        let to_pay_to = match &o.recipient {
            Recipient::Sapling(a) => ua.sapling() == Some(a),
            Recipient::Orchard(a) => ua.orchard() == Some(a),
        };
        if to_pay_to {
            paid = paid.saturating_add(o.value_zat);
            if !pools.contains(&o.pool) {
                pools.push(o.pool);
            }
        }
    }

    if pools.is_empty() {
        return Err(if memo_seen { VerifyError::WrongRecipient } else { VerifyError::NoMatchingOutput });
    }
    if paid < challenge.amount_zat {
        return Err(VerifyError::Underpaid { paid, want: challenge.amount_zat });
    }
    Ok(Payment { amount_zat: paid, confirmations: confirmations(mined, tip), pools })
}
