
use std::{collections::HashMap, str::FromStr};

use secp256k1::SecretKey;
use x402z_facilitator::{
    receipt,
    verify::{self, MerchantKey, VerifyError},
    NETWORK,
};
use x402z_protocol::{Network, PaymentRequired, ReceiptClaims, SCHEME};
use zcash_keys::keys::{UnifiedAddressRequest, UnifiedFullViewingKey, UnifiedSpendingKey};
use zcash_primitives::transaction::{
    builder::{BuildConfig, Builder, BundlePadding},
    fees::zip317,
    Transaction,
};
use zcash_protocol::{
    consensus::{BlockHeight, NetworkUpgrade, Parameters},
    memo::{Memo, MemoBytes},
    value::Zatoshis,
    ShieldedPool,
};
use zcash_transparent::{
    address::TransparentAddress,
    builder::TransparentSigningSet,
    bundle::{OutPoint, TxOut},
};
use zcash_client_backend::{decrypt_transaction, TransferType};
use zip32::{DiversifierIndex, Scope};

pub(super) const NONCE: &str = "x402z-0123456789abcdef01234567";
pub(super) const PRICE: u64 = 100_000;

pub(super) fn merchant(seed_byte: u8) -> UnifiedFullViewingKey {
    UnifiedSpendingKey::from_seed(&NETWORK, &[seed_byte; 32], zip32::AccountId::ZERO)
        .unwrap()
        .to_unified_full_viewing_key()
}

/// First address at or after diversifier `idx` with both an Orchard and a Sapling receiver.
pub(super) fn ua_at(ufvk: &UnifiedFullViewingKey, idx: u32) -> zcash_keys::address::UnifiedAddress {
    use zcash_keys::keys::ReceiverRequirement::{Omit, Require};
    let req = UnifiedAddressRequest::unsafe_custom(Require, Require, Omit);
    ufvk.find_address(DiversifierIndex::from(idx), req).unwrap().0
}

pub(super) fn challenge(pay_to: &str, amount: u64) -> PaymentRequired {
    PaymentRequired {
        scheme: SCHEME.into(),
        network: Network::Testnet,
        pay_to: pay_to.into(),
        amount_zat: amount,
        memo_nonce: NONCE.into(),
        expires_at: 4_000_000_000,
    }
}

pub(super) fn target_height() -> BlockHeight {
    NETWORK.activation_height(NetworkUpgrade::Nu7).unwrap() + 100
}

pub(super) fn memo(text: &str) -> MemoBytes {
    MemoBytes::from(Memo::from_str(text).unwrap())
}

pub(super) enum Out<'a> {
    Ironwood(orchard::Address, u64, &'a str),
    NonText(orchard::Address, u64),
    Sapling(sapling::PaymentAddress, u64, &'a str),
    Transparent(TransparentAddress, u64),
    /// Ironwood output whose sender keeps an outgoing-viewing-key copy (as a wallet does when
    /// it pays someone, or sends itself change).
    IronwoodOvk(orchard::Address, u64, &'a str, orchard::keys::OutgoingViewingKey),
    SaplingOvk(sapling::PaymentAddress, u64, &'a str, sapling::keys::OutgoingViewingKey),
}

/// Build, prove, sign and serialize a tx funded by one transparent input of exactly
/// outputs + ZIP 317 fee.
pub(super) fn build_tx(outs: &[Out<'_>]) -> Vec<u8> {
    let sk = SecretKey::from_secret_bytes([0x42; 32]).unwrap();
    let pk = sk.public_key();
    let make = |input_value: u64| {
        let mut b = Builder::new(
            NETWORK,
            target_height(),
            BuildConfig::Standard {
                sapling_anchor: Some(sapling::Anchor::empty_tree()),
                orchard_anchor: None,
                ironwood_anchor: Some(orchard::Anchor::empty_tree()),
                orchard_padding: BundlePadding::DEFAULT,
                ironwood_padding: BundlePadding::DEFAULT,
            },
        );
        let coin = TxOut::new(
            Zatoshis::from_u64(input_value).unwrap(),
            TransparentAddress::from_pubkey(&pk).script().into(),
        );
        b.add_transparent_p2pkh_input(pk, OutPoint::new([7u8; 32], 0), coin).unwrap();
        for o in outs {
            match o {
                Out::NonText(to, v) => { b.add_ironwood_output::<zip317::FeeError>(None, *to, Zatoshis::from_u64(*v).unwrap(), MemoBytes::empty()).unwrap() }
                Out::Ironwood(to, v, m) => {
                    b.add_ironwood_output::<zip317::FeeError>(None, *to, Zatoshis::from_u64(*v).unwrap(), memo(m)).unwrap()
                }
                Out::Sapling(to, v, m) => {
                    b.add_sapling_output::<zip317::FeeError>(None, *to, Zatoshis::from_u64(*v).unwrap(), memo(m)).unwrap()
                }
                Out::Transparent(to, v) => b.add_transparent_output(to, Zatoshis::from_u64(*v).unwrap()).unwrap(),
                Out::IronwoodOvk(to, v, m, ovk) => b
                    .add_ironwood_output::<zip317::FeeError>(Some(ovk.clone()), *to, Zatoshis::from_u64(*v).unwrap(), memo(m))
                    .unwrap(),
                Out::SaplingOvk(to, v, m, ovk) => b
                    .add_sapling_output::<zip317::FeeError>(Some(*ovk), *to, Zatoshis::from_u64(*v).unwrap(), memo(m))
                    .unwrap(),
            }
        }
        b
    };
    let rule = zip317::FeeRule::standard();
    let paid: u64 = outs.iter().map(|o| match o {
        Out::NonText(_, v) | Out::Ironwood(_, v, _) | Out::Sapling(_, v, _) | Out::Transparent(_, v) => *v,
        Out::IronwoodOvk(_, v, _, _) | Out::SaplingOvk(_, v, _, _) => *v,
    }).sum();
    let fee = make(paid).get_fee(&rule).unwrap().into_u64();
    let mut signing = TransparentSigningSet::new();
    signing.add_key(sk);
    let built = make(paid + fee)
        .build(
            &signing,
            &[],
            &[],
            rand10::rand_core::UnwrapErr(rand10::rngs::SysRng),
            &sapling::prover::mock::MockSpendProver,
            &sapling::prover::mock::MockOutputProver,
            &rule,
        )
        .expect("build tx");
    let mut bytes = Vec::new();
    built.transaction().write(&mut bytes).unwrap();
    bytes
}

/// The merchant's UIVK as an operator would configure it: encoded, then decoded.
pub(super) fn uivk_key(ufvk: &UnifiedFullViewingKey) -> MerchantKey {
    let s = ufvk.to_unified_incoming_viewing_key().encode(&NETWORK).unwrap();
    assert!(s.starts_with("uivktest1"), "{s}");
    MerchantKey::decode_uivk(&NETWORK, &s).unwrap()
}

/// What the facilitator does with lightwalletd's bytes, through the UIVK path (default) and
/// the UFVK-reduced path; both must agree.
pub(super) fn check(bytes: &[u8], ufvk: &UnifiedFullViewingKey, ch: &PaymentRequired, mined: Option<u32>, tip: u32) -> Result<verify::Payment, VerifyError> {
    let mined = mined.map(BlockHeight::from_u32);
    let tip = BlockHeight::from_u32(tip);
    let tx: Transaction = verify::parse_tx(&NETWORK, bytes, mined, tip)?;
    let via_uivk = verify::verify_payment(&NETWORK, &uivk_key(ufvk), ch, &tx, mined, tip);
    let reduced = MerchantKey::from_ufvk(ufvk);
    assert!(reduced.from_full_key);
    assert_eq!(via_uivk, verify::verify_payment(&NETWORK, &reduced, ch, &tx, mined, tip), "UIVK and UFVK paths disagree");
    via_uivk
}
