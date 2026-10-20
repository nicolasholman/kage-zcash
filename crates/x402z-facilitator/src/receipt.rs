//! Receipt signing. Wire format (see `x402z_protocol::ReceiptClaims`):
//! `base64url(claims_json) "." base64url(ed25519_sig)`, no padding; the signature covers the
//! exact `claims_json` bytes. `x402z-server` verifies with `X402Z_RECEIPT_PUBKEY`.

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use x402z_protocol::ReceiptClaims;

/// `X402Z_RECEIPT_SIGNING_KEY`: hex of the 32-byte ed25519 secret key (seed).
pub fn parse_signing_key(hex_key: &str) -> Result<SigningKey, String> {
    let raw = hex::decode(hex_key.trim()).map_err(|e| format!("not hex: {e}"))?;
    let arr: [u8; 32] = raw.try_into().map_err(|_| "must be 32 bytes (64 hex chars)".to_string())?;
    Ok(SigningKey::from_bytes(&arr))
}

pub fn generate_signing_key() -> SigningKey {
    SigningKey::generate(&mut rand::rngs::OsRng)
}

pub fn sign(key: &SigningKey, claims: &ReceiptClaims) -> String {
    let raw = serde_json::to_vec(claims).expect("claims serialize");
    let sig = key.sign(&raw);
    format!("{}.{}", URL_SAFE_NO_PAD.encode(&raw), URL_SAFE_NO_PAD.encode(sig.to_bytes()))
}

/// Same check `x402z-server` applies; used by the agent and in tests.
pub fn verify(key: &VerifyingKey, receipt: &str) -> Result<ReceiptClaims, String> {
    let (c, s) = receipt.trim().split_once('.').ok_or("receipt malformed")?;
    let raw = URL_SAFE_NO_PAD.decode(c).map_err(|_| "receipt malformed")?;
    let sig = Signature::from_slice(&URL_SAFE_NO_PAD.decode(s).map_err(|_| "receipt malformed")?)
        .map_err(|_| "receipt malformed")?;
    key.verify(&raw, &sig).map_err(|_| "receipt signature invalid")?;
    serde_json::from_slice(&raw).map_err(|e| format!("receipt claims malformed: {e}"))
}
