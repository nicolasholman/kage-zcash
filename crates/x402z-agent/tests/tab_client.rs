//! The agent's Kage Tab client against a localhost issuer that speaks x402z-server's tab wire
//! format with real RFC 9474 blind RSA (RSA-2048): a 402 carrying `extra.tab`, `POST /tab/issue`
//! (answering 202 once while "the payment confirms", then the blind signatures) and `X-PAYMENT`
//! scheme `zcash-shielded-tab` checked against a spent set. No chain is involved: the txid is
//! handed to the client as an external wallet's would be. The same client ran live against the
//! real server and testnet (README, "Kage Tab").

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

type Kp = KeyPair<Sha384, PSS, Randomized>;

const PAY_TO: &str = "utest1sampleaddressforschemaonly000000";
const NONCE: &str = "x402z-tabfixture";

struct Issuer {
    kp: Kp,
    key_id: String,
    issue_calls: usize,
    issued: HashSet<String>,
    spent: HashSet<Vec<u8>>,
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

fn respond(s: &mut TcpStream, status: &str, extra_headers: &[(&str, String)], body: &str) {
    let mut h = String::new();
    for (k, v) in extra_headers {
        h.push_str(&format!("{k}: {v}\r\n"));
    }
    let _ = write!(s, "HTTP/1.1 {status}\r\ncontent-type: application/json\r\n{h}content-length: {}\r\nconnection: close\r\n\r\n{body}", body.len());
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
                if st.issue_calls == 1 {
                    respond(&mut s, "202 Accepted", &[], r#"{"status":"pending","reason":"settlement_pending","message":"0 of 1 confirmations"}"#);
                } else if req.memo_nonce != NONCE || req.key_id != st.key_id || !st.issued.insert(req.memo_nonce.clone()) {
                    respond(&mut s, "422 Unprocessable Entity", &[], r#"{"status":"rejected","reason":"nonce_already_used"}"#);
                } else {
                    let sigs: Vec<String> = req
                        .blinded
                        .iter()
                        .map(|b| b64(&st.kp.sk.blind_sign(b64_any(b).unwrap()).unwrap()))
                        .collect();
                    let body = json!({ "keyId": st.key_id, "blindSignatures": sigs }).to_string();
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

fn tmp(name: &str) -> PathBuf {
    let p = std::env::temp_dir().join(format!("x402z-agent-tabclient-{}-{name}.json", std::process::id()));
    for q in [p.clone(), p.with_extension("json.lock"), p.with_extension("tab.json"), p.with_extension("tab.json.lock")] {
        let _ = std::fs::remove_file(q);
    }
    p
}

fn config(ledger: &PathBuf, cap: u64) -> AgentConfig {
    AgentConfig {
        ledger_path: Some(ledger.clone()),
        wallet_dir: std::env::temp_dir(),
        agent: "tab-test".into(),
        policy: Policy { daily_cap_zat: cap, allowed_hosts: vec!["127.0.0.1".into()], approval_threshold_zat: None },
        seed: None,
        birthday: None,
        lwd_url: String::new(),
        confirm_timeout_secs: 60,
        poll_secs: 0,
    }
}

fn no_approval(_: &str, _: u64, _: u64) -> bool {
    false
}

#[tokio::test]
async fn one_external_payment_buys_n_tokens_each_spendable_once() {
    let kp = Kp::generate(&mut DefaultRng, 2048).unwrap();
    let state = Arc::new(Mutex::new(Issuer { kp, key_id: "fixturekey".into(), issue_calls: 0, issued: HashSet::new(), spent: HashSet::new() }));
    let url = serve(state.clone());
    let ledger = tmp("flow");
    let cfg = config(&ledger, 1_000_000);

    // 1. spendguard authorises a tab of 3 and hands back the ZIP 321 request for 3 x 100,000 zat
    let p = tab::start_external(&cfg, &url, 3, &no_approval).await.unwrap();
    assert_eq!((p.n, p.amount_zat, p.memo_nonce.as_str()), (3, 300_000, NONCE));
    assert!(p.payment_uri.starts_with(&format!("zcash:{PAY_TO}?amount=0.003&memo=")), "{}", p.payment_uri);

    // 2. an external wallet paid; the client blinds 3 messages, waits out one 202, unblinds 3 tokens
    let txid = "ab".repeat(32);
    let b = tab::finish_external(&cfg, &txid, None).await.unwrap();
    assert_eq!((b.n, b.amount_zat, b.key_id.as_str()), (3, 300_000, "fixturekey"));
    assert_eq!(state.lock().unwrap().issue_calls, 2);
    let path = tab::tab_path(&cfg).unwrap();
    let stored = tab::TabFile::open(&path).unwrap().state;
    assert_eq!(stored.tokens.len(), 3);
    assert!(stored.pending.is_empty());
    // the payment counts against today's cap, once
    let l: serde_json::Value = serde_json::from_slice(&std::fs::read(&ledger).unwrap()).unwrap();
    let paid: Vec<_> = l["audit"].as_array().unwrap().iter().filter(|e| e["txid"] == txid.as_str()).collect();
    assert_eq!(paid.len(), 1);
    assert_eq!(paid[0]["amountZat"], 300_000);
    assert_eq!(l["agents"][0]["spentTodayZat"], 300_000);

    // 3. three requests, one token each, no new payment
    for left in [2, 1, 0] {
        let f = tab::fetch(&cfg, &url).await.unwrap().unwrap();
        assert_eq!(f.status, 200);
        assert!(f.settlement.unwrap().success);
        assert_eq!(f.remaining, left);
    }
    assert!(tab::fetch(&cfg, &url).await.unwrap().is_none(), "no token left");

    // 4. a token replayed from a copy of the file is refused by the server and not put back
    {
        let mut f = tab::TabFile::open(&path).unwrap();
        f.state.tokens.push(stored.tokens[0].clone());
        f.save().unwrap();
    }
    let e = tab::fetch(&cfg, &url).await.unwrap_err();
    assert!(matches!(&e, PayError::Rejected(m) if m.contains("already spent")), "{e:?}");
    assert_eq!(tab::TabFile::open(&path).unwrap().state.tokens.len(), 0);

    // 5. a forged token (signature over another message) is refused as invalid
    {
        let mut forged = stored.tokens[1].clone();
        forged.msg = b64(&[9u8; 32]);
        let mut f = tab::TabFile::open(&path).unwrap();
        f.state.tokens.push(forged);
        f.save().unwrap();
    }
    let e = tab::fetch(&cfg, &url).await.unwrap_err();
    assert!(matches!(&e, PayError::Rejected(m) if m.contains("signature invalid")), "{e:?}");

    // 6. nothing pending any more
    assert!(matches!(tab::finish_external(&cfg, &txid, None).await, Err(PayError::Config(_))));

    // 7. spendguard refuses a tab over the cap before anything is requested from a wallet
    let e = tab::start_external(&config(&ledger, 500_000), &url, 3, &no_approval).await.unwrap_err();
    assert!(matches!(&e, PayError::Refused(m) if m.contains("daily cap exceeded")), "{e:?}");
    for q in [ledger.clone(), ledger.with_extension("json.lock"), path.clone(), path.with_extension("json.lock")] {
        let _ = std::fs::remove_file(q);
    }
}
