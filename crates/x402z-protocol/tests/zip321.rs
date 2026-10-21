use x402z_protocol::{zec_decimal, zip321_uri};

#[test]
fn amounts_are_decimal_zec_without_trailing_zeros() {
    assert_eq!(zec_decimal(100_000), "0.001");
    assert_eq!(zec_decimal(1), "0.00000001");
    assert_eq!(zec_decimal(100_000_000), "1");
    assert_eq!(zec_decimal(150_000_000), "1.5");
    assert_eq!(zec_decimal(0), "0");
    assert_eq!(zec_decimal(2_100_000_000_000_000), "21000000");
    // ZIP 321 grammar: 1*DIGIT [ "." 1*8DIGIT ]
    for z in [1u64, 10, 12_345_678, 99_999_999, 100_000_001, 123_456_789_012] {
        let s = zec_decimal(z);
        let (w, f) = s.split_once('.').unwrap_or((&s, ""));
        assert!(!w.is_empty() && w.chars().all(|c| c.is_ascii_digit()), "{s}");
        assert!(f.len() <= 8 && f.chars().all(|c| c.is_ascii_digit()) && !f.ends_with('0'), "{s}");
    }
}

#[test]
fn memo_is_base64url_without_padding() {
    // ZIP 321's own example: memo=VGhpcyBpcyBhIHNpbXBsZSBtZW1vLg is "This is a simple memo."
    let uri = zip321_uri("utest1abc", 100_000, "This is a simple memo.");
    assert_eq!(uri, "zcash:utest1abc?amount=0.001&memo=VGhpcyBpcyBhIHNpbXBsZSBtZW1vLg");
    let uri = zip321_uri("utest1abc", 100_000, "x402z-f19a63da20cbc0749ea800b3");
    let memo = uri.split("memo=").nth(1).unwrap();
    assert!(!memo.contains(['=', '+', '/']));
    use base64::Engine;
    let back = base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(memo).unwrap();
    assert_eq!(back, b"x402z-f19a63da20cbc0749ea800b3");
}
