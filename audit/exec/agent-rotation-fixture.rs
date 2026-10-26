
use std::{
    io::{BufRead, BufReader, Read, Write},
    net::{TcpListener, TcpStream},
    path::PathBuf,
    sync::{Arc, Mutex},
};

use blind_rsa_signatures::{DefaultRng, KeyPair, MessageRandomizer, Randomized, Sha384, Signature, PSS};
use serde_json::json;
use spendguard::Policy;
use x402z_agent::{
    pay::{now, AgentConfig, PayError},
    tab,
};
use x402z_protocol::{b64, b64_any, decode_header, encode_header, PaymentPayload, SettlementResponse, TabIssueRequest};

pub(super) type Kp = KeyPair<Sha384, PSS, Randomized>;

pub(super) struct Key {
    pub(super) kp: Kp,
    pub(super) id: String,
    pub(super) epoch_ends_at: u64,
}

pub(super) struct Issuer {
    pub(super) key: Key,
    /// Rotate to this key when the first /tab/issue arrives (and answer that one 409).
    pub(super) rotate_on_issue: Option<Key>,
    pub(super) issue_calls: usize,
    pub(super) moved: bool,
    pub(super) post_paths: Vec<String>,
}

pub(super) fn key(id: &str, epoch_ends_at: u64) -> Key {
    Key { kp: Kp::generate(&mut DefaultRng, 2048).unwrap(), id: id.into(), epoch_ends_at }
}

pub(super) fn read_request(s: &mut TcpStream) -> (String, Vec<(String, String)>, Vec<u8>) {
    let mut r = BufReader::new(s.try_clone().unwrap());
    let mut line = String::new();
    r.read_line(&mut line).unwrap();
    let mut headers = Vec::new();
    loop {
        let mut h = String::new();
        r.read_line(&mut h).unwrap();
        let h = h.trim_end();
        if h.is_empty() {
            break;
        }
        if let Some((k, v)) = h.split_once(':') {
            headers.push((k.trim().to_ascii_lowercase(), v.trim().to_string()));
        }
    }
    let len = headers.iter().find(|(k, _)| k == "content-length").and_then(|(_, v)| v.parse().ok()).unwrap_or(0);
    let mut body = vec![0u8; len];
    r.read_exact(&mut body).unwrap();
    (line, headers, body)
}

pub(super) fn respond(s: &mut TcpStream, status: &str, extra: &str, body: &str) {
    let _ = write!(s, "HTTP/1.1 {status}\r\ncontent-type: application/json\r\n{extra}content-length: {}\r\nconnection: close\r\n\r\n{body}", body.len());
}

pub(super) fn serve(state: Arc<Mutex<Issuer>>) -> String {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = l.local_addr().unwrap();
    std::thread::spawn(move || {
        for stream in l.incoming() {
            let Ok(mut s) = stream else { continue };
            let (line, headers, body) = read_request(&mut s);
            let mut st = state.lock().unwrap();
            if line.starts_with("POST ") { st.post_paths.push(line.clone()); }
            if line.starts_with("POST /tab/issue") {
                let req: TabIssueRequest = serde_json::from_slice(&body).unwrap();
                st.issue_calls += 1;
                if let Some(next) = st.rotate_on_issue.take() {
                    st.key = next;
                }
                if req.key_id != st.key.id {
                    let b = json!({ "status": "rejected", "reason": "tab_key_rotated", "activeKeyId": st.key.id }).to_string();
                    respond(&mut s, "409 Conflict", "", &b);
                    continue;
                }
                let sigs: Vec<String> = req.blinded.iter().map(|b| b64(&st.key.kp.sk.blind_sign(b64_any(b).unwrap()).unwrap())).collect();
                respond(&mut s, "200 OK", "", &json!({ "keyId": st.key.id, "blindSignatures": sigs }).to_string());
                continue;
            }
            if let Some((_, h)) = headers.iter().find(|(k, _)| k == "x-payment") {
                let t = decode_header::<PaymentPayload>(h).unwrap().tab_payload().unwrap();
                let r: [u8; 32] = b64_any(&t.randomizer).unwrap().try_into().unwrap();
                let ok = t.key_id == st.key.id
                    && st.key.kp.pk.verify(&Signature(b64_any(&t.sig).unwrap()), Some(MessageRandomizer(r)), &b64_any(&t.msg).unwrap()).is_ok();
                let (status, settle) = if ok { ("200 OK", SettlementResponse::ok("tab:x")) } else { ("402 Payment Required", SettlementResponse::failed("invalid_payment")) };
                respond(&mut s, status, &format!("x-payment-response: {}\r\n", encode_header(&settle)), r#"{"quote":"tab"}"#);
                continue;
            }
            let body = json!({
                "x402Version": 1, "error": "X-PAYMENT header is required",
                "accepts": [{
                    "scheme": "zcash-shielded", "network": "zcash-testnet", "maxAmountRequired": "100000",
                    "resource": format!("http://{addr}/api/quote"), "description": "fixture", "mimeType": "application/json",
                    "payTo": "utest1sampleaddressforschemaonly000000", "maxTimeoutSeconds": 900, "asset": "ZEC",
                    "extra": { "memoNonce": format!("x402z-keys{}", st.issue_calls), "expiresAt": 4_000_000_000u64,
                               "tab": { "scheme": "zcash-shielded-tab", "suite": "RSABSSA-SHA384-PSS-Randomized", "keyId": st.key.id,
                                        "publicKey": b64(&st.key.kp.pk.to_spki().unwrap()), "unitPriceZat": 100000, "maxTokens": 20,
                                        "issuePath": if st.moved && st.issue_calls>0 {"/tab/changed"}else{"/tab/issue"}, "epochEndsAt": st.key.epoch_ends_at, "graceSecs": 0 } }
                }]
            })
            .to_string();
            respond(&mut s, "402 Payment Required", "", &body);
        }
    });
    format!("http://{addr}/api/quote")
}
