//! The ZIP 321 URIs Kage puts in `extra.paymentUri` parse with librustzcash's own ZIP 321 parser
//! (the one wallets built on zcash_client_backend use) into exactly the challenge's address,
//! amount and memo.
#![allow(deprecated)] // zcash_client_backend::zip321 re-exports the zip321 crate

use std::str::FromStr;

use x402z_facilitator::NETWORK;
use x402z_protocol::{challenge_uri, Network, PaymentRequired, SCHEME};
use zcash_client_backend::zip321::TransactionRequest;
use zcash_keys::keys::{ReceiverRequirement::{Omit, Require}, UnifiedAddressRequest, UnifiedSpendingKey};
use zcash_protocol::memo::{Memo, MemoBytes};

#[test]
fn librustzcash_parses_the_challenge_uri_back_to_the_challenge() {
    let ufvk = UnifiedSpendingKey::from_seed(&NETWORK, &[9u8; 32], zip32::AccountId::ZERO).unwrap().to_unified_full_viewing_key();
    let (ua, _) = ufvk.default_address(UnifiedAddressRequest::unsafe_custom(Require, Require, Omit)).unwrap();
    for (amount, nonce) in [(100_000u64, "x402z-f19a63da20cbc0749ea800b3"), (1, "x402z-a"), (123_456_789, "x402z-0123456789abcdef01234567")] {
        let ch = PaymentRequired {
            scheme: SCHEME.into(),
            network: Network::Testnet,
            pay_to: ua.encode(&NETWORK),
            amount_zat: amount,
            memo_nonce: nonce.into(),
            expires_at: 2_000_000_000,
        };
        let uri = challenge_uri(&ch);
        let req = TransactionRequest::from_uri(&uri).unwrap_or_else(|e| panic!("{uri}: {e:?}"));
        assert_eq!(req.payments().len(), 1);
        let p = &req.payments()[&0];
        assert_eq!(p.recipient_address().encode(), ch.pay_to);
        assert_eq!(p.amount().map(|z| z.into_u64()), Some(amount));
        let want = MemoBytes::from(Memo::from_str(nonce).unwrap());
        assert_eq!(p.memo(), Some(&want));
        // and librustzcash renders the same URI back
        assert_eq!(req.to_uri(), uri);
    }
}
