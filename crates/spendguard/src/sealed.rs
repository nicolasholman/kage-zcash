//! Spendguard state at rest: the agent's ledger and its Kage Tab token file are sealed with
//! XChaCha20-Poly1305 under a 32-byte key, so the file shows neither hosts, amounts, txids nor
//! tokens, and a changed byte (or a file sealed for the other purpose) is refused, not read.
//!
//! Format: `MAGIC (12 bytes) || nonce (24 bytes) || ciphertext || tag (16 bytes)`, with
//! `MAGIC || purpose` as associated data. Callers supply a fresh random nonce for every seal.

use chacha20poly1305::{
    aead::{Aead, KeyInit, Payload},
    XChaCha20Poly1305, XNonce,
};

/// Purpose (associated data) of the agent's ledger file.
pub const LEDGER_PURPOSE: &str = "x402z-agent ledger";
/// Purpose (associated data) of the agent's Kage Tab token file.
pub const TAB_PURPOSE: &str = "x402z-agent tab";

/// File prefix of a sealed spendguard file (version 1).
pub const MAGIC: &[u8; 12] = b"KAGE-SEALED1";
pub const NONCE_LEN: usize = 24;
const TAG_LEN: usize = 16;

/// Why a sealed file could not be opened.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SealError {
    /// The file does not start with [`MAGIC`] (plaintext from an older version, or not ours).
    NotSealed,
    /// Too short to hold a nonce and a tag.
    Truncated,
    /// Authentication failed: wrong key, a changed byte, or sealed for another purpose.
    WrongKeyOrTampered,
}

impl std::fmt::Display for SealError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            SealError::NotSealed => "not a sealed file (no KAGE-SEALED1 header)",
            SealError::Truncated => "sealed file is truncated",
            SealError::WrongKeyOrTampered => "cannot be opened with this key: wrong key, or the file was changed",
        })
    }
}

fn aad(purpose: &str) -> Vec<u8> {
    [MAGIC.as_slice(), purpose.as_bytes()].concat()
}

/// Seal `plaintext` for `purpose` (e.g. "ledger", "tab") under `key` with `nonce`.
pub fn seal(key: &[u8; 32], purpose: &str, nonce: [u8; NONCE_LEN], plaintext: &[u8]) -> Vec<u8> {
    let cipher = XChaCha20Poly1305::new(key.into());
    let ct = cipher
        .encrypt(&XNonce::from(nonce), Payload { msg: plaintext, aad: &aad(purpose) })
        .expect("XChaCha20-Poly1305 encryption of an in-memory buffer cannot fail");
    [MAGIC.as_slice(), &nonce, &ct].concat()
}

/// Open a file sealed by [`seal`] for the same `purpose` and `key`.
pub fn open(key: &[u8; 32], purpose: &str, data: &[u8]) -> Result<Vec<u8>, SealError> {
    let rest = data.strip_prefix(MAGIC.as_slice()).ok_or(SealError::NotSealed)?;
    if rest.len() < NONCE_LEN + TAG_LEN {
        return Err(SealError::Truncated);
    }
    let (nonce, ct) = rest.split_at(NONCE_LEN);
    let nonce: [u8; NONCE_LEN] = nonce.try_into().map_err(|_| SealError::Truncated)?;
    XChaCha20Poly1305::new(key.into())
        .decrypt(&XNonce::from(nonce), Payload { msg: ct, aad: &aad(purpose) })
        .map_err(|_| SealError::WrongKeyOrTampered)
}

/// Is `data` a sealed file (whatever its key)?
pub fn is_sealed(data: &[u8]) -> bool {
    data.starts_with(MAGIC)
}

/// A 32-byte key from 64 hex characters.
pub fn parse_key_hex(s: &str) -> Result<[u8; 32], String> {
    let s = s.trim();
    if s.len() != 64 || !s.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err("must be 64 hex characters (32 bytes; e.g. `openssl rand -hex 32`)".into());
    }
    let mut k = [0u8; 32];
    for (i, b) in k.iter_mut().enumerate() {
        *b = u8::from_str_radix(&s[2 * i..2 * i + 2], 16).map_err(|e| e.to_string())?;
    }
    Ok(k)
}

#[cfg(test)]
mod tests {
    use super::*;

    const K: [u8; 32] = [7; 32];

    #[test]
    fn round_trip_and_nothing_readable_on_disk() {
        let plain = br#"{"audit":[{"host":"api.example.org","txid":"d3bf0b49408fed11"}]}"#;
        let s = seal(&K, "ledger", [1; NONCE_LEN], plain);
        assert!(is_sealed(&s));
        assert_eq!(open(&K, "ledger", &s).unwrap(), plain);
        for needle in [&b"api.example.org"[..], b"d3bf0b49", b"audit", b"host"] {
            assert!(!s.windows(needle.len()).any(|w| w == needle), "{:?} visible", std::str::from_utf8(needle));
        }
    }

    #[test]
    fn wrong_key_tampering_truncation_and_purpose_swaps_are_refused() {
        let s = seal(&K, "ledger", [2; NONCE_LEN], b"{}");
        assert_eq!(open(&[8; 32], "ledger", &s), Err(SealError::WrongKeyOrTampered));
        assert_eq!(open(&K, "tab", &s), Err(SealError::WrongKeyOrTampered), "a ledger can't pass for a tab file");
        for i in [MAGIC.len(), MAGIC.len() + NONCE_LEN, s.len() - 1] {
            let mut t = s.clone();
            t[i] ^= 1;
            assert_eq!(open(&K, "ledger", &t), Err(SealError::WrongKeyOrTampered), "byte {i}");
        }
        assert_eq!(open(&K, "ledger", &s[..MAGIC.len() + 10]), Err(SealError::Truncated));
        assert_eq!(open(&K, "ledger", b"{\"audit\":[]}"), Err(SealError::NotSealed));
    }

    #[test]
    fn hex_keys() {
        assert_eq!(parse_key_hex(&"0a".repeat(32)).unwrap(), [10; 32]);
        assert!(parse_key_hex("abc").is_err());
        assert!(parse_key_hex(&"zz".repeat(32)).is_err());
    }
}
