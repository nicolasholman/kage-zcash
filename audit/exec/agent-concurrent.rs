
use std::{
    io::{BufRead, BufReader, Read, Write},
    net::{TcpListener, TcpStream},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
};

use blind_rsa_signatures::{DefaultRng, KeyPair, Randomized, Sha384, PSS};
use serde_json::json;
use spendguard::Policy;
use x402z_agent::{
    ledger::{AuditEntry, LedgerFile, LedgerKey},
    pay::{self, AgentConfig, PaidFetch},
    tab::{self, BuyOutcome, PendingPurchase, TabFile},
};
use x402z_protocol::{b64, encode_header, SettlementResponse, TabOffer};

const PAY_TO: &str = "utest1sampleaddressforschemaonly000000";
const OTHER_TX: &str = "b7b7b7b7b7b7b7b7b7b7b7b7b7b7b7b7b7b7b7b7b7b7b7b7b7b7b7b7b7b7b7b7";

fn read_request(s: &mut TcpStream) -> (String, bool) {
    let mut r = BufReader::new(s.try_clone().unwrap());
    let mut line = String::new();
    r.read_line(&mut line).unwrap();
    let (mut x_payment, mut len) = (false, 0usize);
    loop {
        let mut h = String::new();
        r.read_line(&mut h).unwrap();
        let h = h.trim_end().to_ascii_lowercase();
        if h.is_empty() {
            break;
        }
        x_payment |= h.starts_with("x-payment:");
        if let Some(v) = h.strip_prefix("content-length:") {
            len = v.trim().parse().unwrap_or(0);
        }
    }
    let mut body = vec![0u8; len];
    r.read_exact(&mut body).unwrap();
    (line, x_payment)
}

fn respond(s: &mut TcpStream, status: &str, extra: &str, body: &str) {
    let _ = write!(s, "HTTP/1.1 {status}\r\ncontent-type: application/json\r\n{extra}content-length: {}\r\nconnection: close\r\n\r\n{body}", body.len());
}

fn tmp(name: &str) -> PathBuf {
    let p = std::env::temp_dir().join(format!("x402z-agent-concurrent-{}-{name}.json", std::process::id()));
    for q in [p.clone(), p.with_extension("json.lock"), p.with_extension("tab.json"), p.with_extension("tab.json.lock")] {
        let _ = std::fs::remove_file(q);
    }
    p
}

fn cfg(ledger: &Path, host: &str, key: &LedgerKey) -> AgentConfig {
    AgentConfig {
        ledger_path: Some(ledger.to_path_buf()),
        wallet_dir: std::env::temp_dir(),
        agent: "concurrent-test".into(),
        policy: Policy { daily_cap_zat: 10_000_000, allowed_hosts: vec![host.to_string()], approval_threshold_zat: None },
        seed: None,
        ledger_key: Some(key.clone()),
        birthday: None,
        lwd_url: String::new(),
        confirm_timeout_secs: 60,
        poll_secs: 0,
    }
}

fn the_402(addr: &str, tab: Option<serde_json::Value>) -> String {
    let mut extra = json!({ "memoNonce": "x402z-callA", "expiresAt": 4_000_000_000u64 });
    if let Some(t) = tab {
        extra["tab"] = t;
    }
    json!({
        "x402Version": 1, "error": "X-PAYMENT header is required",
        "accepts": [{
            "scheme": "zcash-shielded", "network": "zcash-testnet", "maxAmountRequired": "100000",
            "resource": format!("http://{addr}/api/quote"), "description": "fixture", "mimeType": "application/json",
            "payTo": PAY_TO, "maxTimeoutSeconds": 900, "asset": "ZEC", "extra": extra
        }]
    })
    .to_string()
}

#[tokio::test]
async fn audit_tab_purchase_race_preserves_one_charge() {
    let ledger = tmp("tab");
    let key = LedgerKey::from_bytes([12; 32]);
    let kp = KeyPair::<Sha384, PSS, Randomized>::generate(&mut DefaultRng, 2048).unwrap();
    let offer = TabOffer {
        scheme: "zcash-shielded-tab".into(),
        suite: "RSABSSA-SHA384-PSS-Randomized".into(),
        key_id: "k1".into(),
        public_key: b64(&kp.pk.to_spki().unwrap()),
        unit_price_zat: 100_000,
        max_tokens: 20,
        issue_path: "/tab/issue".into(),
        epoch_ends_at: 4_000_000_000,
        grace_secs: 0,
    };
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let host = format!("127.0.0.1:{}", l.local_addr().unwrap().port());
    let url = format!("http://{host}/api/quote");
    let tab_file = ledger.with_extension("tab.json");
    let issue_calls = Arc::new(AtomicUsize::new(0));
    {
        let (tab_file, key, host, url, offer, issue_calls, ledger_path) = (tab_file.clone(), key.clone(), host.clone(), url.clone(), offer.clone(), issue_calls.clone(), ledger.clone());
        std::thread::spawn(move || {
            for stream in l.incoming() {
                let Ok(mut s) = stream else { continue };
                let (line, _) = read_request(&mut s);
                if line.starts_with("POST /tab/issue") {
                    issue_calls.fetch_add(1, Ordering::SeqCst);
                    respond(&mut s, "202 Accepted", "", r#"{"status":"pending","reason":"settlement_pending","message":"0 of 1 confirmations"}"#);
                    continue;
                }
                // call B buys a tab for this origin while call A waits for this 402
                let mut f = TabFile::open(&tab_file, &key).unwrap();
                if f.state.pending.is_empty() {
                    f.state.pending.push(PendingPurchase {
                        url: url.clone(),
                        n: 3,
                        amount_zat: 300_000,
                        pay_to: PAY_TO.into(),
                        memo_nonce: "x402z-callB".into(),
                        expires_at: 4_000_000_000,
                        payment_uri: String::new(),
                        offer: offer.clone(),
                        created_at: pay::now(),
                        txid: Some(OTHER_TX.into()),
                    });
                    f.save().unwrap();
                    let _ledger_lock=LedgerFile::lock(&ledger_path).unwrap();
                    let mut ledger=LedgerFile::load(&ledger_path,&key).unwrap();
                    ledger.append(x402z_agent::ledger::AuditEntry{ts:pay::now(),agent:"concurrent-test".into(),host:host.clone(),amount_zat:300000,decision:"allowed".into(),reason:Some("Explicit synthetic concurrent post-broadcast fixture".into()),memo_nonce:Some("x402z-callB".into()),txid:Some(OTHER_TX.into()),url:None,settled:None});
                    ledger.save().unwrap();
                }
                drop(f);
                respond(&mut s, "402 Payment Required", "", &the_402(&host, Some(serde_json::to_value(&offer).unwrap())));
            }
        });
    }
    let c = cfg(&ledger, &host, &key);
    let out = tab::buy_or_resume(&c, &url, 2, &|_, _, _| false, false).await.expect("collects, does not try to pay");
    let BuyOutcome::Pending { purchase, .. } = out else { panic!("expected B's tab, still confirming") };
    assert_eq!((purchase.txid.as_deref(), purchase.memo_nonce.as_str(), purchase.n), (Some(OTHER_TX), "x402z-callB", 3));
    assert_eq!(issue_calls.load(Ordering::SeqCst), 1, "B's tab was asked for once");
    let l = LedgerFile::load(&ledger, &key).unwrap();
    assert_eq!(l.data.audit.len(),1);assert_eq!(l.data.audit[0].txid.as_deref(),Some(OTHER_TX));assert_eq!(l.guard(&c.agent,&c.policy,pay::now()).spent_today(&c.agent,pay::now()),300000);println!("AUDIT B-325 PASS");
    for q in [ledger.clone(), ledger.with_extension("json.lock"), tab_file.clone(), tab_file.with_extension("json.lock")] {
        let _ = std::fs::remove_file(q);
    }
}
