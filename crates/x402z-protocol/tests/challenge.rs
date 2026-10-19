use x402z_protocol::{InvalidChallenge, Network, PaymentRequired};

// Shape-only sample; the address is not a real key and is never paid.
const SAMPLE: &str = r#"{
  "scheme": "zcash-shielded",
  "network": "testnet",
  "payTo": "utest1sampleaddressforschemaonly000000",
  "amountZat": 100000,
  "memoNonce": "q-7f3a9c2e",
  "expiresAt": 2000000000
}"#;

fn sample() -> PaymentRequired {
    serde_json::from_str(SAMPLE).unwrap()
}

#[test]
fn sample_parses_and_validates() {
    let c = sample();
    assert_eq!(c.network, Network::Testnet);
    assert_eq!(c.amount_zat, 100_000);
    c.validate(1_900_000_000).unwrap();
}

#[test]
fn round_trips_with_camel_case_keys() {
    let v: serde_json::Value = serde_json::to_value(sample()).unwrap();
    for k in ["scheme", "network", "payTo", "amountZat", "memoNonce", "expiresAt"] {
        assert!(v.get(k).is_some(), "missing {k}");
    }
}

#[test]
fn unknown_fields_rejected() {
    let bad = SAMPLE.replace("\"scheme\"", "\"extra\": 1, \"scheme\"");
    assert!(serde_json::from_str::<PaymentRequired>(&bad).is_err());
}

#[test]
fn rejects_expired_mainnet_wrong_scheme_bad_nonce() {
    let c = sample();
    assert!(matches!(c.validate(2_000_000_000), Err(InvalidChallenge::Expired { .. })));

    let mut m = sample();
    m.network = Network::Mainnet;
    assert_eq!(m.validate(0), Err(InvalidChallenge::MainnetNotAllowed));

    let mut s = sample();
    s.scheme = "exact".into();
    assert!(matches!(s.validate(0), Err(InvalidChallenge::WrongScheme(_))));

    let mut n = sample();
    n.memo_nonce = "has space".into();
    assert_eq!(n.validate(0), Err(InvalidChallenge::BadNonce));
    n.memo_nonce = "x".repeat(65);
    assert_eq!(n.validate(0), Err(InvalidChallenge::BadNonce));

    let mut z = sample();
    z.amount_zat = 0;
    assert_eq!(z.validate(0), Err(InvalidChallenge::ZeroAmount));

    let mut a = sample();
    a.pay_to = "zs1notaunifiedaddress0000000".into();
    assert_eq!(a.validate(0), Err(InvalidChallenge::BadAddress));
}
