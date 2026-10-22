//! Kage Tab key rules on the agent side, against a localhost issuer with real RFC 9474 blind RSA
//! (RSA-2048) in x402z-server's tab wire format:
//! - a merchant that offers a second key for the same origin while the first one's epoch runs is
//!   refused before spendguard (it could link every token to its buyer);
//! - an offer that stops issuing before a payment can confirm is refused;
//! - a 409 `tab_key_rotated` at issue (key rotated between payment and issue) is answered by
//!   blinding again for the new key and retrying the same payment.
//! No chain is involved: the txid is handed in as an external wallet's would be.

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

type Kp = KeyPair<Sha384, PSS, Randomized>;

struct Key {
    kp: Kp,
    id: String,
    epoch_ends_at: u64,
}

struct Issuer {
    key: Key,
    /// Rotate to this key when the first /tab/issue arrives (and answer that one 409).
    rotate_on_issue: Option<Key>,
    issue_calls: usize,
}

fn key(id: &str, epoch_ends_at: u64) -> Key {
    Key { kp: Kp::generate(&mut DefaultRng, 2048).unwrap(), id: id.into(), epoch_ends_at }
}

fn read_request(s: &mut TcpStream) -> (String, Vec<(String, String)>, Vec<u8>) {
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

fn respond(s: &mut TcpStream, status: &str, extra: &str, body: &str) {
    let _ = write!(s, "HTTP/1.1 {status}\r\ncontent-type: application/json\r\n{extra}content-length: {}\r\nconnection: close\r\n\r\n{body}", body.len());
}

fn serve(state: Arc<Mutex<Issuer>>) -> String {
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
                                        "issuePath": "/tab/issue", "epochEndsAt": st.key.epoch_ends_at, "graceSecs": 0 } }
                }]
            })
            .to_string();
            respond(&mut s, "402 Payment Required", "", &body);
        }
    });
    format!("http://{addr}/api/quote")
}

fn tmp(name: &str) -> PathBuf {
    let p = std::env::temp_dir().join(format!("x402z-agent-tabkeys-{}-{name}.json", std::process::id()));
    for q in [p.clone(), p.with_extension("json.lock"), p.with_extension("tab.json"), p.with_extension("tab.json.lock")] {
        let _ = std::fs::remove_file(q);
    }
    p
}

fn config(ledger: &PathBuf, confirm_timeout_secs: u64) -> AgentConfig {
    AgentConfig {
        ledger_path: Some(ledger.clone()),
        wallet_dir: std::env::temp_dir(),
        agent: "tab-keys".into(),
        policy: Policy { daily_cap_zat: 10_000_000, allowed_hosts: vec!["127.0.0.1".into()], approval_threshold_zat: None },
        seed: None,
        birthday: None,
        lwd_url: String::new(),
        confirm_timeout_secs,
        poll_secs: 0,
    }
}

fn audit_len(ledger: &PathBuf) -> usize {
    x402z_agent::ledger::LedgerFile::load(ledger).unwrap().data.audit.len()
}

fn no_approval(_: &str, _: u64, _: u64) -> bool {
    false
}

#[tokio::test]
async fn a_second_key_for_the_same_origin_inside_one_epoch_is_refused_before_spendguard() {
    let state = Arc::new(Mutex::new(Issuer { key: key("k1", 4_000_000_000), rotate_on_issue: None, issue_calls: 0 }));
    let url = serve(state.clone());
    let ledger = tmp("pin");
    let cfg = config(&ledger, 60);
    tab::start_external(&cfg, &url, 2, &no_approval).await.unwrap();
    let decisions = audit_len(&ledger);

    // the merchant now hands this buyer a different key, same epoch
    state.lock().unwrap().key = key("k2", 4_000_000_000);
    let e = tab::start_external(&cfg, &url, 2, &no_approval).await.unwrap_err();
    assert!(matches!(&e, PayError::Runtime(m) if m.contains("k2") && m.contains("k1") && m.contains("still in its epoch")), "{e:?}");
    assert_eq!(audit_len(&ledger), decisions, "refused before spendguard: no decision recorded");
    // the agent-wallet path refuses it the same way (it would stop at the missing seed otherwise)
    let e = tab::buy(&cfg, &url, 2, &no_approval).await.unwrap_err();
    assert!(matches!(&e, PayError::Runtime(m) if m.contains("still in its epoch")), "{e:?}");
    assert_eq!(audit_len(&ledger), decisions);
}

#[tokio::test]
async fn an_offer_that_stops_issuing_before_a_payment_can_confirm_is_refused() {
    let state = Arc::new(Mutex::new(Issuer { key: key("k1", now() + 30), rotate_on_issue: None, issue_calls: 0 }));
    let url = serve(state);
    let ledger = tmp("life");
    let e = tab::start_external(&config(&ledger, 900), &url, 2, &no_approval).await.unwrap_err();
    assert!(matches!(&e, PayError::Runtime(m) if m.contains("stops issuing")), "{e:?}");
    assert!(!ledger.exists() || audit_len(&ledger) == 0);
}

#[tokio::test]
async fn a_rotated_key_at_issue_is_answered_by_blinding_again_for_the_new_key() {
    // k1's epoch ends in 2 s; the payment confirms after it, when the server has rotated to k2.
    let state = Arc::new(Mutex::new(Issuer { key: key("k1", now() + 2), rotate_on_issue: None, issue_calls: 0 }));
    let url = serve(state.clone());
    let ledger = tmp("rotate");
    let cfg = config(&ledger, 0);
    let p = tab::start_external(&cfg, &url, 3, &no_approval).await.unwrap();
    assert_eq!(p.offer.key_id, "k1");
    std::thread::sleep(std::time::Duration::from_secs(3));
    state.lock().unwrap().rotate_on_issue = Some(key("k2", 4_000_000_000));

    let b = tab::finish_external(&cfg, &"ab".repeat(32), None).await.unwrap();
    assert_eq!((b.n, b.key_id.as_str()), (3, "k1"), "the purchase is the one paid under k1");
    assert_eq!(state.lock().unwrap().issue_calls, 2, "one 409, then the same payment with new blinded messages");
    // the stored tokens are k2 tokens and the k2 server takes them
    let stored = tab::TabFile::open(&tab::tab_path(&cfg).unwrap()).unwrap().state;
    assert!(stored.tokens.len() == 3 && stored.tokens.iter().all(|t| t.key_id == "k2"), "{:?}", stored.tokens);
    for left in [2, 1, 0] {
        assert_eq!(tab::fetch(&cfg, &url).await.unwrap().unwrap().remaining, left);
    }
}
