//! Does a transaction pay a 402 challenge? Decided by trial decryption with the merchant UFVK.
//!
//! An output counts toward the challenge only if it
//! 1. decrypts with the UFVK's *external* incoming viewing key (a real incoming payment, not
//!    change or an outgoing output recovered with the OVK),
//! 2. carries a text memo exactly equal to `memoNonce`, and
//! 3. was sent to the receiver of `payTo` for its pool (Sapling receiver for Sapling notes,
//!    the Orchard receiver for Orchard and Ironwood notes, which share it).
//!
//! The values of all such outputs are summed and compared to `amountZat`.

use std::collections::HashMap;

use zcash_client_backend::{decrypt_transaction, TransferType};
use zcash_keys::{address::Address, keys::UnifiedFullViewingKey};
use zcash_primitives::transaction::Transaction;
use zcash_protocol::{
    consensus::{BlockHeight, BranchId, Parameters},
    memo::Memo,
    ShieldedPool,
};
use x402z_protocol::PaymentRequired;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Payment {
    pub amount_zat: u64,
    /// 0 when the transaction is only in the mempool.
    pub confirmations: u32,
    pub pools: Vec<ShieldedPool>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VerifyError {
    /// `payTo` is not a testnet unified address with a shielded receiver.
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
            VerifyError::BadPayTo => write!(f, "payTo is not a testnet unified address with a shielded receiver"),
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

fn memo_matches(memo: &zcash_protocol::memo::MemoBytes, nonce: &str) -> bool {
    matches!(Memo::try_from(memo), Ok(Memo::Text(t)) if &*t == nonce)
}

/// Check that `tx` pays `challenge`. Does not check expiry or confirmation policy; the caller
/// owns those (they depend on wall-clock time and configuration).
pub fn verify_payment<P: Parameters>(
    params: &P,
    ufvk: &UnifiedFullViewingKey,
    challenge: &PaymentRequired,
    tx: &Transaction,
    mined: Option<BlockHeight>,
    tip: BlockHeight,
) -> Result<Payment, VerifyError> {
    let Some(Address::Unified(ua)) = Address::decode(params, &challenge.pay_to) else {
        return Err(VerifyError::BadPayTo);
    };
    if ua.sapling().is_none() && ua.orchard().is_none() {
        return Err(VerifyError::BadPayTo);
    }

    let ufvks = HashMap::from([(0u32, ufvk.clone())]);
    let d = decrypt_transaction(params, mined, Some(tip), tx, &ufvks);

    let nonce = challenge.memo_nonce.as_str();
    let mut memo_seen = false;
    let mut paid: u64 = 0;
    let mut pools = Vec::new();
    let mut credit = |value: u64, pool: ShieldedPool| {
        paid = paid.saturating_add(value);
        if !pools.contains(&pool) {
            pools.push(pool);
        }
    };

    for o in d.sapling_outputs() {
        if o.transfer_type() != TransferType::Incoming || !memo_matches(o.memo(), nonce) {
            continue;
        }
        memo_seen = true;
        if ua.sapling() == Some(&o.note().recipient()) {
            credit(o.note().value().inner(), o.value_pool());
        }
    }
    for o in d.orchard_outputs().iter().chain(d.ironwood_outputs()) {
        if o.transfer_type() != TransferType::Incoming || !memo_matches(o.memo(), nonce) {
            continue;
        }
        memo_seen = true;
        let (note, _) = o.note();
        if ua.orchard() == Some(&note.recipient()) {
            credit(note.value().inner(), o.value_pool());
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
