//! A settled payment whose 200 was lost. The server (a localhost stand-in in the x402 wire
//! format) answers the agent's X-PAYMENT for (txid, memoNonce) with `402 nonce_already_used`, as
//! x402z-server does once its resend window is over. Before this fix the agent recorded that as a
//! refusal (`settled: false`), so the ledger said "not paid" and the next paid_fetch paid again.
//! No wallet, chain or network beyond 127.0.0.1 is involved.

use std::{
    io::{BufRead, BufReader, Write},
    net::TcpListener,
    path::PathBuf,
};

use serde_json::json;
use spendguard::Policy;
use x402z_agent::{
    ledger::{AuditEntry, LedgerFile, LedgerKey},
    pay::{self, AgentConfig, PayError},
};
use x402z_protocol::{encode_header, SettlementResponse};

fn serve(answer: impl Fn(bool) -> String + Send + 'static) -> String {
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
            let _ = s.write_all(answer(x_payment).as_bytes());
        }
    });
    format!("127.0.0.1:{}", addr.port())
}

fn tmp(name: &str) -> PathBuf {
    let p = std::env::temp_dir().join(format!("x402z-agent-lost-{}-{name}.json", std::process::id()));
    for q in [p.clone(), p.with_extension("json.lock")] {
        let _ = std::fs::remove_file(q);
    }
    p
}

#[tokio::test]
async fn nonce_already_used_for_the_agents_own_payment_keeps_it_settled_and_says_do_not_pay_again() {
    let host = serve(|x_payment| {
        assert!(x_payment, "the agent only presents the payment here");
        let body = json!({ "x402Version": 1, "error": "receipt already used", "accepts": [] }).to_string();
        format!(
            "HTTP/1.1 402 Payment Required\r\ncontent-type: application/json\r\nx-payment-response: {}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
            encode_header(&SettlementResponse::failed("nonce_already_used")),
            body.len()
        )
    });
    let url = format!("http://{host}/api/quote");
    let ledger = tmp("ledger");
    let key = LedgerKey::from_bytes([5; 32]);
    let txid = "f0".repeat(32);
    for (settled, name) in [(Some(true), "after a 200"), (None, "before any answer")] {
        let mut l = LedgerFile::load(&ledger, &key).unwrap();
        l.data.audit.clear();
        l.append(AuditEntry {
            ts: pay::now(),
            agent: "lost-test".into(),
            host: host.clone(),
            amount_zat: 100_000,
            decision: "allowed".into(),
            reason: Some("paid".into()),
            memo_nonce: Some("x402z-lost".into()),
            txid: Some(txid.clone()),
            url: Some(url.clone()),
            settled,
        });
        l.save().unwrap();
        let cfg = AgentConfig {
            ledger_path: Some(ledger.clone()),
            wallet_dir: std::env::temp_dir(),
            agent: "lost-test".into(),
            policy: Policy { daily_cap_zat: 1_000_000, allowed_hosts: vec![host.clone()], approval_threshold_zat: None },
            seed: None,
            ledger_key: Some(key.clone()),
            birthday: None,
            lwd_url: String::new(),
            confirm_timeout_secs: 60,
            poll_secs: 0,
        };
        let e = pay::redeem_payment(&cfg, &url, &txid, "x402z-lost").await.unwrap_err();
        assert!(matches!(&e, PayError::AlreadyRedeemed(m) if m.contains("Do NOT pay again")), "{name}: {e:?}");
        assert_eq!((e.exit_code(), e.kind()), (6, "already_redeemed"));
        let l = LedgerFile::load(&ledger, &key).unwrap();
        let entry = l.data.audit.iter().find(|e| e.txid.as_deref() == Some(txid.as_str())).unwrap();
        assert_eq!(entry.settled, Some(true), "{name}: the payment stays settled, never 'refused'");
        assert!(entry.reason.as_deref().unwrap().contains("already redeemed"), "{name}: {entry:?}");
        assert_eq!(l.guard("lost-test", &cfg.policy, pay::now()).spent_today("lost-test", pay::now()), 100_000, "still charged once");
    }
    for q in [ledger.clone(), ledger.with_extension("json.lock")] {
        let _ = std::fs::remove_file(q);
    }
}
