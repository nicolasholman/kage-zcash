//! The agent never follows a redirect on a paid request. Two localhost HTTP servers: A (the only
//! allowlisted host) answers 302 to B, and B answers an x402 402 with its own payTo and a tab
//! offer. Before this fix the agent followed the redirect, spendguard approved B's price under A's
//! allowlist entry, and only the missing wallet stopped the payment ("allowed" in the ledger).
//! No wallet, chain or network beyond 127.0.0.1 is involved.

use std::{
    io::{BufRead, BufReader, Write},
    net::TcpListener,
    path::PathBuf,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
};

use serde_json::json;
use spendguard::Policy;
use x402z_agent::{
    pay::{self, AgentConfig, PayError},
    tab,
};

/// Serve forever; `answer(request_line, has_x_payment)` -> raw HTTP response.
fn serve(answer: impl Fn(&str, bool) -> String + Send + 'static) -> String {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = l.local_addr().unwrap();
    std::thread::spawn(move || {
        for stream in l.incoming() {
            let Ok(mut s) = stream else { continue };
            let mut r = BufReader::new(s.try_clone().unwrap());
            let mut line = String::new();
            let _ = r.read_line(&mut line);
            let mut x_payment = false;
            loop {
                let mut h = String::new();
                if r.read_line(&mut h).is_err() || h.trim_end().is_empty() {
                    break;
                }
                x_payment |= h.to_ascii_lowercase().starts_with("x-payment:");
            }
            let _ = s.write_all(answer(&line, x_payment).as_bytes());
        }
    });
    format!("127.0.0.1:{}", addr.port())
}

fn tmp(name: &str) -> PathBuf {
    let p = std::env::temp_dir().join(format!("x402z-agent-redirect-{}-{name}.json", std::process::id()));
    for q in [p.clone(), p.with_extension("json.lock"), p.with_extension("tab.json"), p.with_extension("tab.json.lock")] {
        let _ = std::fs::remove_file(q);
    }
    p
}

#[tokio::test]
async fn a_redirect_from_an_allowlisted_host_is_refused_before_spendguard_and_never_reaches_the_target() {
    // B: the redirect target, with its own payTo. Counts every request it gets.
    let hits_b = Arc::new(AtomicUsize::new(0));
    let xpay_b = Arc::new(AtomicUsize::new(0));
    let (h, x) = (hits_b.clone(), xpay_b.clone());
    let b = serve(move |_, has_xpay| {
        h.fetch_add(1, Ordering::SeqCst);
        if has_xpay {
            x.fetch_add(1, Ordering::SeqCst);
        }
        let body = json!({
            "x402Version": 1, "error": "X-PAYMENT header is required",
            "accepts": [{
                "scheme": "zcash-shielded", "network": "zcash-testnet", "maxAmountRequired": "100000",
                "resource": "http://b/api/quote", "description": "redirect target", "mimeType": "application/json",
                "payTo": "utest1redirecttargetaddressnotallowlisted00", "maxTimeoutSeconds": 900, "asset": "ZEC",
                "extra": { "memoNonce": "x402z-redirected", "expiresAt": 4_000_000_000u64,
                           "tab": { "scheme": "zcash-shielded-tab", "suite": "RSABSSA-SHA384-PSS-Randomized", "keyId": "b",
                                    "publicKey": "AA==", "unitPriceZat": 100000, "maxTokens": 20, "issuePath": "/tab/issue",
                                    "epochEndsAt": 4_000_000_000u64 } }
            }]
        })
        .to_string();
        format!("HTTP/1.1 402 Payment Required\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}", body.len())
    });
    // A: allowlisted, redirects everything to B.
    let target = format!("http://{b}/api/quote");
    let a = serve(move |_, _| format!("HTTP/1.1 302 Found\r\nlocation: {target}\r\ncontent-length: 0\r\nconnection: close\r\n\r\n"));

    let ledger = tmp("ledger");
    let cfg = AgentConfig {
        ledger_path: Some(ledger.clone()),
        wallet_dir: std::env::temp_dir(),
        agent: "redirect-test".into(),
        policy: Policy { daily_cap_zat: 1_000_000, allowed_hosts: vec![a.clone()], approval_threshold_zat: None },
        seed: None,
        birthday: None,
        lwd_url: String::new(),
        confirm_timeout_secs: 60,
        poll_secs: 0,
    };
    let url = format!("http://{a}/api/quote");
    let e = pay::paid_fetch(&cfg, &url, &|_, _, _| false).await.unwrap_err();
    assert!(matches!(&e, PayError::Runtime(m) if m.contains("302") && m.contains("redirect") && m.contains(&b)), "{e:?}");

    // the Kage Tab purchase path refuses the same redirect
    let e = tab::start_external(&cfg, &url, 2, &|_, _, _| false).await.unwrap_err();
    assert!(matches!(&e, PayError::Runtime(m) if m.contains("redirect")), "{e:?}");

    assert_eq!(hits_b.load(Ordering::SeqCst), 0, "the redirect target never saw a request");
    assert_eq!(xpay_b.load(Ordering::SeqCst), 0, "and never an X-PAYMENT");
    // spendguard never ran: no decision at all was recorded for host A
    assert!(!ledger.exists() || std::fs::read(&ledger).unwrap().is_empty(), "nothing recorded as allowed");
    for q in [ledger.clone(), ledger.with_extension("json.lock"), ledger.with_extension("tab.json"), ledger.with_extension("tab.json.lock")] {
        let _ = std::fs::remove_file(q);
    }
}
