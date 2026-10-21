//! Real testnet wallet: a `zcash_client_sqlite` database in `X402Z_AGENT_WALLET_DIR`, synced
//! over lightwalletd, spending with the ZIP 32 account-0 key derived from `X402Z_AGENT_SEED`.

use std::{path::Path, str::FromStr};

use bip0039::{English, Mnemonic};
use rand::{rand_core::UnwrapErr, rngs::SysRng};
use secrecy::{ExposeSecret, SecretVec};
use zcash_client_backend::{
    data_api::{
        wallet::{
            create_proposed_transactions, input_selection::GreedyInputSelectorError,
            propose_standard_transfer_to_address, ConfirmationsPolicy, SpendingKeys,
        },
        Account as _, AccountBirthday, WalletRead, WalletWrite,
    },
    fees::StandardFeeRule,
    proto::service::{BlockId, RawTransaction},
    wallet::OvkPolicy,
};
use zcash_client_sqlite::{util::SystemClock, wallet::init::init_wallet_db, AccountUuid, WalletDb};
use zcash_keys::{
    address::Address,
    keys::{UnifiedAddressRequest, UnifiedFullViewingKey, UnifiedSpendingKey},
};
use zcash_protocol::{
    consensus::BlockHeight,
    memo::{Memo, MemoBytes},
    value::Zatoshis,
    ShieldedPool, TxId,
};
use x402z_facilitator::{lwd, NETWORK};

use crate::cache::MemBlockCache;

pub type Db = WalletDb<rusqlite::Connection, zcash_protocol::consensus::Network, SystemClock, UnwrapErr<SysRng>>;

/// Minimum value the wallet must hold beyond the payment: the ZIP 317 fee floor for a
/// one-recipient shielded tx (2 logical actions x 5000 zat). The real fee comes from the proposal.
pub const MIN_FEE_ZAT: u64 = 10_000;

pub fn seed_from_phrase(phrase: &str) -> Result<SecretVec<u8>, String> {
    let m = Mnemonic::<English>::from_phrase(phrase.trim()).map_err(|e| format!("not a valid BIP 39 English mnemonic: {e}"))?;
    Ok(SecretVec::new(m.to_seed("").to_vec()))
}

pub fn new_phrase() -> String {
    Mnemonic::<English>::generate(bip0039::Count::Words24).phrase().to_string()
}

pub fn open(dir: &Path, seed: Option<&SecretVec<u8>>) -> Result<Db, String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("cannot create wallet dir {}: {e}", dir.display()))?;
    let path = dir.join("wallet.sqlite");
    let mut db = WalletDb::for_path(&path, NETWORK, SystemClock, UnwrapErr(SysRng))
        .map_err(|e| format!("cannot open wallet db {}: {e}", path.display()))?;
    init_wallet_db(&mut db, seed.map(|s| SecretVec::new(s.expose_secret().clone())))
        .map_err(|e| format!("wallet db migration failed ({}): {e}", path.display()))?;
    Ok(db)
}

pub fn account(db: &Db) -> Result<Option<AccountUuid>, String> {
    Ok(db.get_account_ids().map_err(|e| e.to_string())?.into_iter().next())
}

pub fn ufvk(db: &Db, id: AccountUuid) -> Result<UnifiedFullViewingKey, String> {
    let acct = db.get_account(id).map_err(|e| e.to_string())?.ok_or("account vanished")?;
    acct.ufvk().cloned().ok_or_else(|| "wallet account has no UFVK".to_string())
}

pub fn encode_ufvk(ufvk: &UnifiedFullViewingKey) -> Result<String, String> {
    ufvk.encode(&NETWORK).map_err(|e| format!("cannot encode viewing key: {e:?}"))
}

/// The account's unified incoming viewing key (what a merchant's facilitator needs).
pub fn encode_uivk(ufvk: &UnifiedFullViewingKey) -> Result<String, String> {
    ufvk.to_unified_incoming_viewing_key().encode(&NETWORK).map_err(|e| format!("cannot encode incoming viewing key: {e:?}"))
}

/// Shielded-only UA with both an Orchard (also used by Ironwood) and a Sapling receiver, so
/// any shielded testnet wallet can pay it; no transparent receiver.
pub fn address(ufvk: &UnifiedFullViewingKey) -> Result<String, String> {
    use zcash_keys::keys::ReceiverRequirement::{Omit, Require};
    let (ua, _) = ufvk
        .default_address(UnifiedAddressRequest::unsafe_custom(Require, Require, Omit))
        .map_err(|e| format!("cannot derive address: {e:?}"))?;
    Ok(ua.encode(&NETWORK))
}

/// Account-0 spending key for `seed`, checked against the wallet's stored viewing key.
pub fn spending_key(db: &Db, id: AccountUuid, seed: &SecretVec<u8>) -> Result<UnifiedSpendingKey, String> {
    let usk = UnifiedSpendingKey::from_seed(&NETWORK, seed.expose_secret(), zip32::AccountId::ZERO)
        .map_err(|e| format!("key derivation failed: {e:?}"))?;
    if encode_ufvk(&usk.to_unified_full_viewing_key())? != encode_ufvk(&ufvk(db, id)?)? {
        return Err("X402Z_AGENT_SEED does not match the wallet in X402Z_AGENT_WALLET_DIR".into());
    }
    Ok(usk)
}

/// Create account 0 with a birthday at `birthday` (tree state taken from the block before it).
pub async fn create_account(
    db: &mut Db,
    client: &mut lwd::Client,
    seed: &SecretVec<u8>,
    birthday: BlockHeight,
) -> Result<AccountUuid, String> {
    let prior = u64::from(u32::from(birthday).saturating_sub(1));
    let ts = client
        .get_tree_state(BlockId { height: prior, hash: vec![] })
        .await
        .map_err(|e| format!("GetTreeState({prior}) failed: {}", e.message()))?
        .into_inner();
    let bday = AccountBirthday::from_treestate(ts, None).map_err(|e| format!("bad tree state from lightwalletd: {e}"))?;
    let (id, _usk) = db
        .create_account("x402z-agent", seed, &bday, None)
        .map_err(|e| format!("create_account failed: {e}"))?;
    Ok(id)
}

pub async fn sync(db: &mut Db, client: &mut lwd::Client) -> Result<(), String> {
    let cache = MemBlockCache::default();
    zcash_client_backend::sync::run(client, &NETWORK, &cache, db, 1000)
        .await
        .map_err(|e| format!("wallet sync failed: {e}"))
}

pub struct Balance {
    pub spendable_zat: u64,
    pub total_zat: u64,
    pub pending_zat: u64,
    pub chain_tip: u32,
    pub scanned_to: u32,
}

pub fn balance(db: &Db, id: AccountUuid) -> Result<Balance, String> {
    let s = db
        .get_wallet_summary(ConfirmationsPolicy::MIN)
        .map_err(|e| e.to_string())?
        .ok_or("wallet not synced yet")?;
    let b = s.account_balances().get(&id).ok_or("no balance for account")?;
    Ok(Balance {
        spendable_zat: b.spendable_value().into_u64(),
        total_zat: b.total().into_u64(),
        pending_zat: (b.value_pending_spendability() + b.change_pending_confirmation())
            .map(|z| z.into_u64())
            .unwrap_or(0),
        chain_tip: u32::from(s.chain_tip_height()),
        scanned_to: u32::from(s.fully_scanned_height()),
    })
}

fn sapling_prover() -> Result<zcash_proofs::prover::LocalTxProver, String> {
    if let Some(p) = zcash_proofs::prover::LocalTxProver::with_default_location() {
        return Ok(p);
    }
    eprintln!("Sapling proving parameters not found; downloading them (~50 MB, one time)...");
    let paths = zcash_proofs::download_sapling_parameters(Some(600))
        .map_err(|e| format!("could not download Sapling parameters: {e}"))?;
    Ok(zcash_proofs::prover::LocalTxProver::new(&paths.spend, &paths.output))
}

/// Build, prove, sign and broadcast a shielded payment of `amount_zat` to `to` with a text memo.
/// Returns the txid that pays `to` (the last transaction of the proposal).
pub async fn pay(
    db: &mut Db,
    client: &mut lwd::Client,
    id: AccountUuid,
    usk: UnifiedSpendingKey,
    to: &str,
    amount_zat: u64,
    memo: &str,
) -> Result<TxId, String> {
    let to = Address::decode(&NETWORK, to).ok_or_else(|| format!("cannot decode payTo address {to}"))?;
    let memo = MemoBytes::from(Memo::from_str(memo).map_err(|e| format!("memo: {e:?}"))?);
    let amount = Zatoshis::from_u64(amount_zat).map_err(|_| "amount out of range".to_string())?;
    let proposal = propose_standard_transfer_to_address::<_, _, zcash_client_sqlite::wallet::commitment_tree::Error>(
        db,
        &NETWORK,
        StandardFeeRule::Zip317,
        id,
        ConfirmationsPolicy::MIN,
        &to,
        amount,
        Some(memo),
        None,
        ShieldedPool::Ironwood,
        None,
        None,
    )
    .map_err(|e| format!("could not build a payment proposal: {e}"))?;
    let prover = sapling_prover()?;
    let txids = create_proposed_transactions::<_, _, GreedyInputSelectorError, StandardFeeRule, zcash_client_sqlite::error::SqliteClientError, _>(
        db,
        &NETWORK,
        &SystemClock,
        &mut UnwrapErr(SysRng),
        &prover,
        &prover,
        &SpendingKeys::from_unified_spending_key(usk),
        OvkPolicy::Sender,
        &proposal,
        None,
    )
    .map_err(|e| format!("could not create the transaction: {e}"))?;

    for txid in txids.iter() {
        let tx = db
            .get_transaction(*txid)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| format!("created tx {txid} missing from wallet db"))?;
        let mut data = Vec::new();
        tx.write(&mut data).map_err(|e| e.to_string())?;
        let resp = client
            .send_transaction(RawTransaction { data, height: 0 })
            .await
            .map_err(|e| format!("SendTransaction RPC failed: {}", e.message()))?
            .into_inner();
        if resp.error_code != 0 {
            return Err(format!("lightwalletd rejected tx {txid}: {} ({})", resp.error_message, resp.error_code));
        }
        eprintln!("broadcast tx {txid}");
    }
    Ok(*txids.last())
}
