
use std::{
    collections::HashSet,
    io::{BufRead, BufReader, Read, Write},
    net::{TcpListener, TcpStream},
    path::PathBuf,
    sync::{Arc, Mutex},
};

use blind_rsa_signatures::{DefaultRng, KeyPair, MessageRandomizer, Randomized, Sha384, Signature, PSS};
use serde_json::json;
use spendguard::Policy;
use x402z_agent::{
    pay::{AgentConfig, PayError},
    tab,
};
use x402z_protocol::{b64, b64_any, decode_header, encode_header, PaymentPayload, SettlementResponse, TabIssueRequest};

pub(super) type Kp = KeyPair<Sha384, PSS, Randomized>;

pub(super) const PAY_TO: &str = "utest1sampleaddressforschemaonly000000";
pub(super) const NONCE: &str = "x402z-tabfixture";

pub(super) struct Issuer {
    pub(super) kp: Kp,
    pub(super) key_id: String,
    pub(super) issue_calls: usize,
    pub(super) mode: u8,
    pub(super) issued: HashSet<String>,
    pub(super) spent: HashSet<Vec<u8>>,
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

pub(super) fn respond(s: &mut TcpStream, status: &str, extra_headers: &[(&str, String)], body: &str) {
    let mut h = String::new();
    for (k, v) in extra_headers {
        h.push_str(&format!("{k}: {v}\r\n"));
    }
    let _ = write!(s, "HTTP/1.1 {status}\r\ncontent-type: application/json\r\n{h}content-length: {}\r\nconnection: close\r\n\r\n{body}", body.len());
}

pub(super) fn serve(state: Arc<Mutex<Issuer>>) -> String {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = l.local_addr().unwrap();
    std::thread::spawn(move || {
        for stream in l.incoming() {
            let Ok(mut s) = stream else { continue };
            let (line, headers, body) = read_request(&mut s);
            let mut st = state.lock().unwrap();
            if line.starts_with("POST /tab/issue") {
                let req: TabIssueRequest = serde_json::from_slice(&body).unwrap();
                st.issue_calls += 1;
                if st.issue_calls == 1 || st.mode == 4 {
                    respond(&mut s, "202 Accepted", &[], r#"{"status":"pending","reason":"settlement_pending","message":"0 of 1 confirmations"}"#);
                } else if req.memo_nonce != NONCE || req.key_id != st.key_id || !st.issued.insert(req.memo_nonce.clone()) {
                    respond(&mut s, "422 Unprocessable Entity", &[], r#"{"status":"rejected","reason":"nonce_already_used"}"#);
                } else {
                    let mut sigs: Vec<String> = req
                        .blinded
                        .iter()
                        .map(|b| b64(&st.kp.sk.blind_sign(b64_any(b).unwrap()).unwrap()))
                        .collect();
                    if st.mode==2 { sigs.clear(); } if st.mode==3 { sigs=vec![b64(&[0u8;256]);req.blinded.len()]; }
                    let id=if st.mode==1 { "wrong-key" } else { &st.key_id };
                    let body = json!({ "keyId": id, "blindSignatures": sigs }).to_string();
                    respond(&mut s, "200 OK", &[], &body);
                }
                continue;
            }
            if let Some((_, h)) = headers.iter().find(|(k, _)| k == "x-payment") {
                let t = decode_header::<PaymentPayload>(h).unwrap().tab_payload().unwrap();
                let msg = b64_any(&t.msg).unwrap();
                let r: [u8; 32] = b64_any(&t.randomizer).unwrap().try_into().unwrap();
                let ok = st.kp.pk.verify(&Signature(b64_any(&t.sig).unwrap()), Some(MessageRandomizer(r)), &msg).is_ok();
                let id = [r.as_slice(), msg.as_slice()].concat();
                if ok && st.spent.insert(id) {
                    let hdr = encode_header(&SettlementResponse::ok("tab:fixture"));
                    respond(&mut s, "200 OK", &[("x-payment-response", hdr)], r#"{"quote":"paid by tab"}"#);
                } else {
                    let why = if ok { "nonce_already_used" } else { "invalid_payment" };
                    let hdr = encode_header(&SettlementResponse::failed(why));
                    let err = if ok { "tab token already spent" } else { "tab token signature invalid" };
                    respond(&mut s, "402 Payment Required", &[("x-payment-response", hdr)], &json!({ "error": err }).to_string());
                }
                continue;
            }
            let spki = st.kp.pk.to_spki().unwrap();
            let body = json!({
                "x402Version": 1, "error": "X-PAYMENT header is required",
                "accepts": [{
                    "scheme": "zcash-shielded", "network": "zcash-testnet", "maxAmountRequired": "100000",
                    "resource": format!("http://{addr}/api/quote"), "description": "fixture", "mimeType": "application/json",
                    "payTo": PAY_TO, "maxTimeoutSeconds": 900, "asset": "ZEC",
                    "extra": { "memoNonce": NONCE, "expiresAt": 4_000_000_000u64,
                               "tab": { "scheme": "zcash-shielded-tab", "suite": "RSABSSA-SHA384-PSS-Randomized", "keyId": st.key_id,
                                        "publicKey": b64(&spki), "unitPriceZat": 100000, "maxTokens": 20, "issuePath": "/tab/issue",
                                        "epochEndsAt": 4_000_000_000u64 } }
                }]
            })
            .to_string();
            respond(&mut s, "402 Payment Required", &[], &body);
        }
    });
    format!("http://{addr}/api/quote")
}
