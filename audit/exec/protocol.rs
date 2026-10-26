#[test]
fn audit_address_nonce_and_payload_boundaries(){
    use x402z_protocol::*;
    let base=PaymentRequired{scheme:SCHEME.into(),network:Network::Testnet,pay_to:"utest1sampleaddressforschemaonly000000".into(),amount_zat:100000,memo_nonce:"n".into(),expires_at:4000000000};
    for address in ["u1wrongaddress0000000000000","utest1short",""]{let mut c=base.clone();c.pay_to=address.into();assert_eq!(c.validate_shape(),Err(InvalidChallenge::BadAddress));}println!("AUDIT B-431 PASS");
    for nonce in ["".to_string(),"x".repeat(65),"has space".into(),"has/slash".into(),"💰".into()]{let mut c=base.clone();c.memo_nonce=nonce;assert_eq!(c.validate_shape(),Err(InvalidChallenge::BadNonce));}for nonce in ["a".into(),"X_-0".repeat(16)]{let mut c=base.clone();c.memo_nonce=nonce;assert!(c.validate_shape().is_ok());}println!("AUDIT B-433 PASS");
    let p=PaymentPayload::zcash(&"ab".repeat(32),"n");for (field,value,why) in [("x402Version",serde_json::json!(2),"invalid_x402_version"),("scheme",serde_json::json!("exact"),"invalid_scheme"),("network",serde_json::json!("zcash-mainnet"),"invalid_network")] {let mut v=serde_json::to_value(&p).unwrap();v[field]=value;let bad:PaymentPayload=serde_json::from_value(v).unwrap();assert_eq!(bad.zcash_payload().unwrap_err().reason(),why);}let mut v=serde_json::to_value(&p).unwrap();v["payload"].as_object_mut().unwrap().remove("memoNonce");let bad:PaymentPayload=serde_json::from_value(v).unwrap();assert_eq!(bad.zcash_payload().unwrap_err().reason(),"invalid_payload");println!("AUDIT B-440 PASS");
}
