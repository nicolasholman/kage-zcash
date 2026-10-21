//! Live against Zcash testnet (testnet.zec.rocks, override with ZCASH_LWD_URL): the four real
//! Kage payments, fetched from lightwalletd and verified exactly as the facilitator does, with
//! the demo merchant's published incoming viewing key. No mocks; needs network access.

use x402z_facilitator::{
    lwd::{self, FetchedTx, TxLocation},
    verify::{self, MerchantKey, VerifyError},
    NETWORK,
};
use x402z_protocol::{Network, PaymentRequired, SCHEME};
use zcash_keys::keys::{ReceiverRequirement::{Omit, Require}, UnifiedAddressRequest, UnifiedSpendingKey};
use zcash_protocol::{consensus::BlockHeight, ShieldedPool};

/// Published in the README on purpose: incoming viewing key of the demo-only testnet merchant.
const MERCHANT_UIVK: &str = "uivktest1ma9hgu9cmmmkv5f5npk7aktj8tradzqyxzwerez5trhku0xxgx7vsh0azzurz44pfdhmyrzxnur3n282f5t0glhje7kay4fw79njmg64g2wupcznmpfpy0gh6wngu0qw07ry404hnh6rq9vuj264gcjfxucylma4c8cuygxezqx2xwph5x66nckx4095pgvjzuq5rudsmr9dztnyv9f2w74unppttta6cu7fr3tx494zqstn2hv";

/// (txid, mined height, memo nonce, block time where pinned)
const PAYMENTS: [(&str, u32, &str, Option<u64>); 4] = [
    ("030c3cbf78ce8460553bba4e82c0ba813f039bd3ecf4d7f2e81c59837dc28351", 4_521_112, "x402z-f19a63da20cbc0749ea800b3", None),
    ("cd64980a5247881a727aa916476a3ba757e59794263c2ff1bcde467654a0916d", 4_521_440, "x402z-af06b1d50af81616075f0cb3", Some(1_792_482_929)),
    ("ccb4214c6a1fdf183c086ae5b4f559d181ead4c1c61dc7e711fc4a08b95b0ed3", 4_524_586, "x402z-b7a15cc628bb7a06bf9a26cd", None),
    ("25a8b324bebee30220382113c6fb9cc3be1144b036117c34778bde3b1d8b689d", 4_524_602, "x402z-696c58afd509cc0f8753f3b0", Some(1_792_562_282)),
];

fn merchant() -> (MerchantKey, String) {
    let key = MerchantKey::decode_uivk(&NETWORK, MERCHANT_UIVK).unwrap();
    let (ua, _) = key.uivk().default_address(UnifiedAddressRequest::unsafe_custom(Require, Require, Omit)).unwrap();
    (key, ua.encode(&NETWORK))
}

fn challenge(pay_to: &str, nonce: &str, amount: u64) -> PaymentRequired {
    PaymentRequired { scheme: SCHEME.into(), network: Network::Testnet, pay_to: pay_to.into(), amount_zat: amount, memo_nonce: nonce.into(), expires_at: 4_000_000_000 }
}

async fn client() -> (lwd::Client, BlockHeight) {
    let url = std::env::var("ZCASH_LWD_URL").ok().filter(|u| !u.is_empty()).unwrap_or_else(|| lwd::DEFAULT_LWD_URL.into());
    let mut c = lwd::connect(&url).await.unwrap_or_else(|e| panic!("live test needs testnet lightwalletd at {url}: {e}"));
    assert_eq!(lwd::info(&mut c).await.unwrap().chain_name, "test");
    let tip = lwd::chain_tip(&mut c).await.unwrap();
    (c, tip)
}

async fn fetch(c: &mut lwd::Client, txid: &str, tip: BlockHeight) -> (zcash_primitives::transaction::Transaction, BlockHeight) {
    let id = lwd::parse_txid(txid).unwrap();
    let FetchedTx::Found { data, location: TxLocation::Mined(h) } = lwd::get_transaction(c, &id).await.unwrap() else {
        panic!("{txid} not found as a mined transaction");
    };
    let tx = verify::parse_tx(&NETWORK, &data, Some(h), tip).unwrap();
    assert_eq!(tx.txid(), id, "lightwalletd returned a different tx");
    (tx, h)
}

#[tokio::test]
async fn the_four_kage_payments_verify_with_only_the_published_uivk() {
    let (key, ua) = merchant();
    let (mut c, tip) = client().await;
    for (txid, height, nonce, time) in PAYMENTS {
        let (tx, h) = fetch(&mut c, txid, tip).await;
        assert_eq!(u32::from(h), height, "{txid}");
        let p = verify::verify_payment(&NETWORK, &key, &challenge(&ua, nonce, 100_000), &tx, Some(h), tip).unwrap();
        assert_eq!(p.amount_zat, 100_000, "{txid}");
        assert_eq!(p.pools, vec![ShieldedPool::Ironwood], "{txid}");
        assert!(p.confirmations >= 1);
        let bt = lwd::block_time(&mut c, h).await.unwrap();
        if let Some(t) = time {
            assert_eq!(bt, t, "minedAt of {txid}");
        }
        assert!((1_792_000_000..1_800_000_000).contains(&bt), "block time {bt}");
    }
}

#[tokio::test]
async fn real_payments_are_refused_for_the_wrong_nonce_amount_or_key() {
    let (key, ua) = merchant();
    let (mut c, tip) = client().await;
    let (txid, _, nonce, _) = PAYMENTS[3];
    let (tx, h) = fetch(&mut c, txid, tip).await;
    let check = |k: &MerchantKey, ch: &PaymentRequired| verify::verify_payment(&NETWORK, k, ch, &tx, Some(h), tip);

    // another challenge's nonce
    assert_eq!(check(&key, &challenge(&ua, PAYMENTS[2].2, 100_000)), Err(VerifyError::NoMatchingOutput));
    // asks for one zatoshi more than was paid
    assert_eq!(check(&key, &challenge(&ua, nonce, 100_001)), Err(VerifyError::Underpaid { paid: 100_000, want: 100_001 }));
    // someone else's incoming key sees nothing in this tx
    let stranger = UnifiedSpendingKey::from_seed(&NETWORK, &[3u8; 32], zip32::AccountId::ZERO).unwrap().to_unified_full_viewing_key();
    let other = MerchantKey::from_ufvk(&stranger);
    let (other_ua, _) = other.uivk().default_address(UnifiedAddressRequest::unsafe_custom(Require, Require, Omit)).unwrap();
    assert_eq!(check(&other, &challenge(&other_ua.encode(&NETWORK), nonce, 100_000)), Err(VerifyError::NoMatchingOutput));
    // the merchant's key, but a payTo it does not own: the note went elsewhere
    assert_eq!(check(&key, &challenge(&other_ua.encode(&NETWORK), nonce, 100_000)), Err(VerifyError::WrongRecipient));
}

#[tokio::test]
async fn unknown_txid_is_not_found() {
    let (mut c, _) = client().await;
    let id = lwd::parse_txid(&"00".repeat(32)).unwrap();
    assert!(matches!(lwd::get_transaction(&mut c, &id).await.unwrap(), FetchedTx::NotFound));
}
