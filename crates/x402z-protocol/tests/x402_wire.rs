//! x402 v1 wire format: coinbase/x402 specs/x402-specification-v1.md + transports-v1/http.md.

use x402z_protocol::*;

/// The spec's own v1 402 example (EVM `exact` on base-sepolia), plus a Kage entry in `accepts`.
const SPEC_402: &str = r#"{
  "x402Version": 1,
  "error": "X-PAYMENT header is required",
  "accepts": [
    {
      "scheme": "exact",
      "network": "base-sepolia",
      "maxAmountRequired": "10000",
      "resource": "https://api.example.com/premium-data",
      "description": "Access to premium market data",
      "mimeType": "application/json",
      "payTo": "0x209693Bc6afc0C5328bA36FaF03C514EF312287C",
      "maxTimeoutSeconds": 60,
      "asset": "0x036CbD53842c5426634e7929541eC2318f3dCF7e",
      "extra": { "name": "USDC", "version": "2" }
    },
    {
      "scheme": "zcash-shielded",
      "network": "zcash-testnet",
      "maxAmountRequired": "100000",
      "resource": "http://127.0.0.1:4020/api/quote",
      "description": "Kage demo quote",
      "mimeType": "application/json",
      "payTo": "utest1sampleaddressforschemaonly000000",
      "maxTimeoutSeconds": 900,
      "asset": "ZEC",
      "extra": { "memoNonce": "x402z-7f3a9c2e", "expiresAt": 2000000000 }
    }
  ]
}"#;

#[test]
fn spec_402_example_round_trips_and_the_zcash_option_is_found() {
    let r: PaymentRequiredResponse = serde_json::from_str(SPEC_402).unwrap();
    assert_eq!(r.x402_version, 1);
    assert_eq!(r.accepts.len(), 2);
    // the EVM entry parses (its extra is kept as raw JSON) but is not ours
    assert_eq!(r.accepts[0].to_challenge(), Err(WireError::Scheme("exact".into())));
    let z = r.zcash_option().unwrap();
    let ch = z.to_challenge().unwrap();
    assert_eq!((ch.amount_zat, ch.memo_nonce.as_str(), ch.expires_at), (100_000, "x402z-7f3a9c2e", 2_000_000_000));
    ch.validate(1_900_000_000).unwrap();

    // serialize -> parse is lossless, and the camelCase field names are the spec's
    let back: PaymentRequiredResponse = serde_json::from_value(serde_json::to_value(&r).unwrap()).unwrap();
    assert_eq!(back, r);
    let v = serde_json::to_value(z).unwrap();
    for k in ["scheme", "network", "maxAmountRequired", "resource", "description", "mimeType", "payTo", "maxTimeoutSeconds", "asset", "extra"] {
        assert!(v.get(k).is_some(), "missing {k}");
    }
}

#[test]
fn challenge_to_requirements_and_back() {
    let ch = PaymentRequired {
        scheme: SCHEME.into(),
        network: Network::Testnet,
        pay_to: "utest1sampleaddressforschemaonly000000".into(),
        amount_zat: 123_456,
        memo_nonce: "x402z-abc".into(),
        expires_at: 1_999_999_999,
    };
    let mut req = PaymentRequirements::from_challenge(&ch, "http://h/api/quote", "d", "application/json", 900);
    assert_eq!(req.max_amount_required, "123456");
    assert_eq!((req.network.as_str(), req.asset.as_str()), ("zcash-testnet", "ZEC"));
    assert_eq!(req.to_challenge().unwrap(), ch);

    let mut extra = req.zcash_extra().unwrap();
    extra.payment_uri = Some("zcash:utest1x?amount=0.00123456".into());
    req.set_zcash_extra(&extra);
    assert_eq!(req.zcash_extra().unwrap().payment_uri.as_deref(), Some("zcash:utest1x?amount=0.00123456"));
    assert_eq!(req.to_challenge().unwrap(), ch);

    req.max_amount_required = "1.5".into();
    assert_eq!(req.to_challenge(), Err(WireError::Amount("1.5".into())));
}

#[test]
fn x_payment_header_is_base64_json_and_round_trips() {
    let p = PaymentPayload::zcash(&"ab".repeat(32), "x402z-abc");
    let h = encode_header(&p);
    // standard base64 of {"x402Version":1,"scheme":"zcash-shielded","network":"zcash-testnet","payload":{...}}
    let json: serde_json::Value = serde_json::from_slice(&base64::Engine::decode(&base64::engine::general_purpose::STANDARD, &h).unwrap()).unwrap();
    assert_eq!(json["x402Version"], 1);
    assert_eq!(json["scheme"], "zcash-shielded");
    assert_eq!(json["network"], "zcash-testnet");
    assert_eq!(json["payload"]["txid"], "ab".repeat(32));
    assert_eq!(json["payload"]["memoNonce"], "x402z-abc");

    let back: PaymentPayload = decode_header(&h).unwrap();
    assert_eq!(back, p);
    assert_eq!(back.zcash_payload().unwrap(), ZcashPayload { txid: "ab".repeat(32), memo_nonce: "x402z-abc".into() });
    // URL-safe, unpadded encodings are accepted too
    let url = base64::Engine::encode(&base64::engine::general_purpose::URL_SAFE_NO_PAD, serde_json::to_vec(&p).unwrap());
    assert_eq!(decode_header::<PaymentPayload>(&url).unwrap(), p);
    assert_eq!(decode_header::<PaymentPayload>("%%%"), Err(WireError::Base64));
}

#[test]
fn spec_evm_payment_payload_parses_but_is_refused_as_zcash() {
    // Shape of the spec's v1 `exact` X-PAYMENT payload.
    let evm = r#"{"x402Version":1,"scheme":"exact","network":"base-sepolia","payload":{"signature":"0x2d6a","authorization":{"from":"0x857b","to":"0x2096","value":"10000","validAfter":"1740672089","validBefore":"1740672154","nonce":"0xf374"}}}"#;
    let p: PaymentPayload = serde_json::from_str(evm).unwrap();
    assert_eq!(p.zcash_payload(), Err(WireError::Scheme("exact".into())));
    let mut wrong_net = PaymentPayload::zcash("ab", "n");
    wrong_net.network = "zcash-mainnet".into();
    assert_eq!(wrong_net.zcash_payload().unwrap_err().reason(), reason::INVALID_NETWORK);
    let mut v2 = PaymentPayload::zcash("ab", "n");
    v2.x402_version = 2;
    assert_eq!(v2.zcash_payload(), Err(WireError::Version(2)));
    let mut no_nonce = PaymentPayload::zcash("ab", "n");
    no_nonce.payload = serde_json::json!({"txid": "ab"});
    assert!(matches!(no_nonce.zcash_payload(), Err(WireError::Json(_))));
}

#[test]
fn settlement_response_matches_spec_fields() {
    let ok = SettlementResponse::ok(&"cd".repeat(32));
    let v = serde_json::to_value(&ok).unwrap();
    assert_eq!(v, serde_json::json!({"success": true, "transaction": "cd".repeat(32), "network": "zcash-testnet", "payer": ""}));
    let bad = SettlementResponse::failed(reason::PENDING);
    let v = serde_json::to_value(&bad).unwrap();
    assert_eq!(v["success"], false);
    assert_eq!(v["errorReason"], "settlement_pending");
    assert_eq!(v["transaction"], "");
    assert_eq!(decode_header::<SettlementResponse>(&encode_header(&ok)).unwrap(), ok);

    // facilitator /settle answer flattens the settlement fields at top level
    let s = SettleResponse {
        settlement: ok,
        status: VerifyStatus::Confirmed,
        receipt: Some("r".into()),
        claims: None,
        confirmations: Some(2),
        min_confirmations: Some(1),
        error: None,
    };
    let v = serde_json::to_value(&s).unwrap();
    assert_eq!(v["success"], true);
    assert_eq!(v["status"], "confirmed");
    assert_eq!(serde_json::from_value::<SettleResponse>(v).unwrap(), s);

    let sup = SupportedResponse { kinds: vec![SupportedKind { x402_version: 1, scheme: SCHEME.into(), network: NETWORK_TESTNET.into() }] };
    assert_eq!(serde_json::to_value(&sup).unwrap(), serde_json::json!({"kinds": [{"x402Version": 1, "scheme": "zcash-shielded", "network": "zcash-testnet"}]}));
}

#[test]
fn tab_payload_round_trips_and_is_scheme_checked() {
    let t = TabPayload { key_id: "k1".into(), msg: b64(b"m"), randomizer: b64(&[1u8; 32]), sig: b64(&[2u8; 256]) };
    let h = encode_header(&PaymentPayload::tab(&t));
    let p: PaymentPayload = decode_header(&h).unwrap();
    assert_eq!(p.scheme, TAB_SCHEME);
    assert_eq!(p.tab_payload().unwrap(), t);
    // a tab payload is not a zcash-shielded payment, and vice versa
    assert_eq!(p.zcash_payload(), Err(WireError::Scheme(TAB_SCHEME.into())));
    assert_eq!(PaymentPayload::zcash("ab", "n").tab_payload(), Err(WireError::Scheme(SCHEME.into())));
    assert_eq!(b64_any(&t.randomizer).unwrap(), vec![1u8; 32]);
}
