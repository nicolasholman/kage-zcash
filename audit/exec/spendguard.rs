#[test]
fn audit_hex_key_shape(){
    for input in ["".to_string(),"a".repeat(63),"x".repeat(64),"a".repeat(65)]{assert_eq!(spendguard::sealed::parse_key_hex(&input).unwrap_err(),"must be 64 hex characters (32 bytes; e.g. `openssl rand -hex 32`)");}
    let parsed=spendguard::sealed::parse_key_hex(&"AB".repeat(32)).unwrap();assert_eq!(parsed.len(),32);assert!(parsed.iter().all(|b|*b==171));println!("AUDIT B-464 PASS");
}
