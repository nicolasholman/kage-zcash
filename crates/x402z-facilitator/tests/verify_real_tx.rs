//! Facilitator verification against real shielded transactions, built in-process:
//! - real ZIP 32 keys (merchant UFVK and unified address derived from a seed),
//! - real note encryption to the merchant's receivers, memo = nonce,
//! - real Halo2 proofs for Ironwood bundles and a real secp256k1 signature on the
//!   transparent input that funds them. The input spends a made-up outpoint, so the chain would
//!   reject the tx; every byte the facilitator reads (bundles, ciphertexts, memo) is genuine.
//! - Sapling outputs use sapling-crypto's mock *proof* (the zk-proof bytes only); note
//!   encryption and trial decryption are real.
//!
//! Transactions are serialized and re-parsed exactly as bytes from lightwalletd would be.
//!
//! Every verification runs twice and must agree: once with the merchant's unified incoming
//! viewing key decoded from its `uivktest1...` string (the facilitator's default), and once
//! with a key reduced from the full viewing key (the deprecated `X402Z_MERCHANT_UFVK` path).

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

const NONCE: &str = "x402z-0123456789abcdef01234567";
const PRICE: u64 = 100_000;

fn merchant(seed_byte: u8) -> UnifiedFullViewingKey {
    UnifiedSpendingKey::from_seed(&NETWORK, &[seed_byte; 32], zip32::AccountId::ZERO)
        .unwrap()
        .to_unified_full_viewing_key()
}

/// First address at or after diversifier `idx` with both an Orchard and a Sapling receiver.
fn ua_at(ufvk: &UnifiedFullViewingKey, idx: u32) -> zcash_keys::address::UnifiedAddress {
    use zcash_keys::keys::ReceiverRequirement::{Omit, Require};
    let req = UnifiedAddressRequest::unsafe_custom(Require, Require, Omit);
    ufvk.find_address(DiversifierIndex::from(idx), req).unwrap().0
}

fn challenge(pay_to: &str, amount: u64) -> PaymentRequired {
    PaymentRequired {
        scheme: SCHEME.into(),
        network: Network::Testnet,
        pay_to: pay_to.into(),
        amount_zat: amount,
        memo_nonce: NONCE.into(),
        expires_at: 4_000_000_000,
    }
}

fn target_height() -> BlockHeight {
    NETWORK.activation_height(NetworkUpgrade::Nu7).unwrap() + 100
}

fn memo(text: &str) -> MemoBytes {
    MemoBytes::from(Memo::from_str(text).unwrap())
}

enum Out<'a> {
    Ironwood(orchard::Address, u64, &'a str),
    Sapling(sapling::PaymentAddress, u64, &'a str),
    Transparent(TransparentAddress, u64),
    /// Ironwood output whose sender keeps an outgoing-viewing-key copy (as a wallet does when
    /// it pays someone, or sends itself change).
    IronwoodOvk(orchard::Address, u64, &'a str, orchard::keys::OutgoingViewingKey),
    SaplingOvk(sapling::PaymentAddress, u64, &'a str, sapling::keys::OutgoingViewingKey),
}

/// Build, prove, sign and serialize a tx funded by one transparent input of exactly
/// outputs + ZIP 317 fee.
fn build_tx(outs: &[Out<'_>]) -> Vec<u8> {
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
        Out::Ironwood(_, v, _) | Out::Sapling(_, v, _) | Out::Transparent(_, v) => *v,
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
fn uivk_key(ufvk: &UnifiedFullViewingKey) -> MerchantKey {
    let s = ufvk.to_unified_incoming_viewing_key().encode(&NETWORK).unwrap();
    assert!(s.starts_with("uivktest1"), "{s}");
    MerchantKey::decode_uivk(&NETWORK, &s).unwrap()
}

/// What the facilitator does with lightwalletd's bytes, through the UIVK path (default) and
/// the UFVK-reduced path; both must agree.
fn check(bytes: &[u8], ufvk: &UnifiedFullViewingKey, ch: &PaymentRequired, mined: Option<u32>, tip: u32) -> Result<verify::Payment, VerifyError> {
    let mined = mined.map(BlockHeight::from_u32);
    let tip = BlockHeight::from_u32(tip);
    let tx: Transaction = verify::parse_tx(&NETWORK, bytes, mined, tip)?;
    let via_uivk = verify::verify_payment(&NETWORK, &uivk_key(ufvk), ch, &tx, mined, tip);
    let reduced = MerchantKey::from_ufvk(ufvk);
    assert!(reduced.from_full_key);
    assert_eq!(via_uivk, verify::verify_payment(&NETWORK, &reduced, ch, &tx, mined, tip), "UIVK and UFVK paths disagree");
    via_uivk
}

#[test]
fn ironwood_and_sapling_payments_verify_with_viewing_key_only() {
    let ufvk = merchant(1);
    let ua = ua_at(&ufvk, 0);
    let ch = challenge(&ua.encode(&NETWORK), PRICE);
    let h = u32::from(target_height());

    // Ironwood (the live Orchard-family pool), exact amount, still in the mempool.
    let iw = build_tx(&[Out::Ironwood(*ua.orchard().unwrap(), PRICE, NONCE)]);
    let p = check(&iw, &ufvk, &ch, None, h - 1).unwrap();
    assert_eq!(p.amount_zat, PRICE);
    assert_eq!(p.confirmations, 0);
    assert_eq!(p.pools, vec![ShieldedPool::Ironwood]);

    // Same bytes once mined at h, tip h+2 => 3 confirmations.
    assert_eq!(check(&iw, &ufvk, &ch, Some(h), h + 2).unwrap().confirmations, 3);

    // Sapling payment to the same UA's Sapling receiver; overpaying is accepted.
    let sp = build_tx(&[Out::Sapling(*ua.sapling().unwrap(), PRICE + 5, NONCE)]);
    let p = check(&sp, &ufvk, &ch, Some(h), h).unwrap();
    assert_eq!((p.amount_zat, p.confirmations, p.pools), (PRICE + 5, 1, vec![ShieldedPool::Sapling]));

    // Two outputs with the nonce in different pools add up.
    let split = build_tx(&[
        Out::Ironwood(*ua.orchard().unwrap(), 60_000, NONCE),
        Out::Sapling(*ua.sapling().unwrap(), 40_000, NONCE),
    ]);
    let p = check(&split, &ufvk, &ch, Some(h), h).unwrap();
    assert_eq!(p.amount_zat, PRICE);
    assert_eq!(p.pools.len(), 2);
}

#[test]
fn wrong_amount_memo_key_and_address_are_rejected() {
    let ufvk = merchant(1);
    let ua = ua_at(&ufvk, 0);
    let pay_to = ua.encode(&NETWORK);
    let h = u32::from(target_height());

    // Underpaid by one zatoshi.
    let under = build_tx(&[Out::Ironwood(*ua.orchard().unwrap(), PRICE - 1, NONCE)]);
    assert_eq!(
        check(&under, &ufvk, &challenge(&pay_to, PRICE), Some(h), h),
        Err(VerifyError::Underpaid { paid: PRICE - 1, want: PRICE })
    );

    // Right amount, memo is a different challenge's nonce.
    let other_nonce = build_tx(&[Out::Ironwood(*ua.orchard().unwrap(), PRICE, "x402z-someotherchallenge")]);
    assert_eq!(check(&other_nonce, &ufvk, &challenge(&pay_to, PRICE), Some(h), h), Err(VerifyError::NoMatchingOutput));

    // Paid to a different merchant: our UFVK cannot decrypt it at all.
    let stranger = ua_at(&merchant(2), 0);
    let elsewhere = build_tx(&[Out::Ironwood(*stranger.orchard().unwrap(), PRICE, NONCE)]);
    assert_eq!(check(&elsewhere, &ufvk, &challenge(&pay_to, PRICE), Some(h), h), Err(VerifyError::NoMatchingOutput));

    // Paid to the same merchant wallet but a different diversified address than payTo.
    let sibling = ua_at(&ufvk, 7);
    let diverted = build_tx(&[Out::Ironwood(*sibling.orchard().unwrap(), PRICE, NONCE)]);
    assert_eq!(check(&diverted, &ufvk, &challenge(&pay_to, PRICE), Some(h), h), Err(VerifyError::WrongRecipient));

    // payTo that is not a unified address.
    assert_eq!(
        check(&under, &ufvk, &challenge("utest1notarealaddress0000000000", PRICE), Some(h), h),
        Err(VerifyError::BadPayTo)
    );

    // Garbage bytes.
    assert!(matches!(check(&[0u8; 40], &ufvk, &challenge(&pay_to, PRICE), Some(h), h), Err(VerifyError::Unparseable(_))));
}

#[test]
fn receipt_round_trip_uses_server_wire_format() {
    let sk = receipt::generate_signing_key();
    let claims = ReceiptClaims {
        memo_nonce: NONCE.into(),
        amount_zat: PRICE,
        pay_to: ua_at(&merchant(1), 0).encode(&NETWORK),
        txid: "ab".repeat(32),
        confirmations: 1,
        issued_at: 1_800_000_000,
        mined_at: Some(1_799_999_900),
    };
    let r = receipt::sign(&sk, &claims);
    assert_eq!(receipt::verify(&sk.verifying_key(), &r).unwrap(), claims);

    // camelCase JSON claims, base64url without padding, 64-byte signature
    use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
    let (c, s) = r.split_once('.').unwrap();
    let json: serde_json::Value = serde_json::from_slice(&URL_SAFE_NO_PAD.decode(c).unwrap()).unwrap();
    for k in ["memoNonce", "amountZat", "payTo", "txid", "confirmations", "issuedAt", "minedAt"] {
        assert!(json.get(k).is_some(), "missing {k}");
    }
    assert_eq!(URL_SAFE_NO_PAD.decode(s).unwrap().len(), 64);

    // Another key's signature, or edited claims, fail.
    let other = receipt::generate_signing_key();
    assert!(receipt::verify(&other.verifying_key(), &r).is_err());
    let mut forged = claims.clone();
    forged.amount_zat = 1;
    let forged = format!("{}.{s}", URL_SAFE_NO_PAD.encode(serde_json::to_vec(&forged).unwrap()));
    assert!(receipt::verify(&sk.verifying_key(), &forged).is_err());

    // Hex key round trip as stored in X402Z_RECEIPT_SIGNING_KEY.
    let parsed = receipt::parse_signing_key(&hex::encode(sk.to_bytes())).unwrap();
    assert_eq!(parsed.verifying_key(), sk.verifying_key());
    assert!(receipt::parse_signing_key("abcd").is_err());
}

#[test]
fn confirmations_and_lwd_height_sentinels() {
    use x402z_facilitator::lwd::TxLocation;
    assert_eq!(verify::confirmations(None, BlockHeight::from_u32(100)), 0);
    assert_eq!(verify::confirmations(Some(BlockHeight::from_u32(100)), BlockHeight::from_u32(100)), 1);
    assert_eq!(verify::confirmations(Some(BlockHeight::from_u32(90)), BlockHeight::from_u32(100)), 11);
    // tip lagging behind the mined height (different backends) still counts the block itself
    assert_eq!(verify::confirmations(Some(BlockHeight::from_u32(101)), BlockHeight::from_u32(100)), 1);
    assert_eq!(TxLocation::from_raw_height(0), TxLocation::Mempool);
    assert_eq!(TxLocation::from_raw_height(u64::MAX), TxLocation::Orphaned);
    assert_eq!(TxLocation::from_raw_height(3_500_000), TxLocation::Mined(BlockHeight::from_u32(3_500_000)));
}

#[test]
fn transparent_receivers_and_outputs_are_never_accepted() {
    let ufvk = merchant(3);
    let h = target_height().into();
    // Merchant UA that also carries a transparent receiver: refused as payTo, even though the
    // tx pays its Orchard receiver in full.
    let shielded = ua_at(&ufvk, 0);
    let taddr = TransparentAddress::PublicKeyHash([0x11; 20]);
    let full = zcash_keys::address::UnifiedAddress::from_receivers(shielded.orchard().copied(), shielded.sapling().copied(), Some(taddr), None, None).unwrap();
    let with_t = full.encode(&NETWORK);
    let shielded_ok = build_tx(&[Out::Ironwood(*full.orchard().unwrap(), PRICE, NONCE)]);
    assert_eq!(check(&shielded_ok, &ufvk, &challenge(&with_t, PRICE), Some(h), h), Err(VerifyError::BadPayTo));

    // Transparent-funded tx paying the merchant's transparent receiver: never credited.
    let pay_to = ua_at(&ufvk, 0).encode(&NETWORK);
    let t_only = build_tx(&[Out::Transparent(taddr, PRICE)]);
    assert_eq!(check(&t_only, &ufvk, &challenge(&pay_to, PRICE), Some(h), h), Err(VerifyError::NoMatchingOutput));
    // Transparent value does not top up a short shielded payment.
    let mixed = build_tx(&[Out::Transparent(taddr, PRICE), Out::Ironwood(*ua_at(&ufvk, 0).orchard().unwrap(), PRICE - 1, NONCE)]);
    assert_eq!(check(&mixed, &ufvk, &challenge(&pay_to, PRICE), Some(h), h), Err(VerifyError::Underpaid { paid: PRICE - 1, want: PRICE }));
}

#[test]
fn uivk_cannot_see_the_merchants_own_change_or_outgoing_outputs() {
    let ufvk = merchant(4);
    let ua = ua_at(&ufvk, 0);
    let ch = challenge(&ua.encode(&NETWORK), PRICE);
    let h = target_height();
    let ofvk = ufvk.orchard().unwrap();
    let sfvk = ufvk.sapling().unwrap();
    let stranger = ua_at(&merchant(5), 0);
    // What the merchant's own wallet emits when it spends: change to its internal (change)
    // addresses, and payments to someone else with an outgoing-viewing-key copy. Every output
    // carries the open challenge's nonce, so only the key decides what is visible.
    let bytes = build_tx(&[
        Out::IronwoodOvk(ofvk.address_at(0u32, Scope::Internal), PRICE, NONCE, ofvk.to_ovk(Scope::Internal)),
        Out::SaplingOvk(sfvk.change_address().1, PRICE, NONCE, sfvk.to_ovk(Scope::Internal)),
        Out::IronwoodOvk(*stranger.orchard().unwrap(), PRICE, NONCE, ofvk.to_ovk(Scope::External)),
        Out::SaplingOvk(*stranger.sapling().unwrap(), PRICE, NONCE, sfvk.to_ovk(Scope::External)),
    ]);
    let tx = verify::parse_tx(&NETWORK, &bytes, Some(h), h).unwrap();

    // The full viewing key sees all four: two change outputs and two outgoing payments.
    let d = decrypt_transaction(&NETWORK, Some(h), Some(h), &tx, &HashMap::from([(0u32, ufvk.clone())]));
    let mut kinds: Vec<TransferType> = d.sapling_outputs().iter().map(|o| o.transfer_type()).collect();
    kinds.extend(d.ironwood_outputs().iter().map(|o| o.transfer_type()));
    assert_eq!(kinds.iter().filter(|k| **k == TransferType::AccountInternal).count(), 2, "{kinds:?}");
    assert_eq!(kinds.iter().filter(|k| **k == TransferType::Outgoing).count(), 2, "{kinds:?}");

    // The incoming key sees none of them, so none can be credited to a challenge.
    assert!(verify::incoming_outputs(&NETWORK, uivk_key(&ufvk).uivk(), &tx, h).is_empty());
    assert_eq!(check(&bytes, &ufvk, &ch, Some(h.into()), h.into()), Err(VerifyError::NoMatchingOutput));

    // Control: a real incoming payment in the same shape is visible to the incoming key.
    let paid = build_tx(&[Out::Ironwood(*ua.orchard().unwrap(), PRICE, NONCE)]);
    let tx = verify::parse_tx(&NETWORK, &paid, Some(h), h).unwrap();
    let seen = verify::incoming_outputs(&NETWORK, uivk_key(&ufvk).uivk(), &tx, h);
    assert_eq!(seen.len(), 1);
    assert_eq!(verify::memo_text(&seen[0].memo).as_deref(), Some(NONCE));
}
