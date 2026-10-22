//! What the agent sends before spendguard has said yes, and what it reads. A localhost server
//! counts every request it gets. Before this fix the agent sent its GET to any URL and only then
//! found the host was not allowlisted (the host learned the agent's IP; over MCP that is a
//! server-side request to any URL), an allowlist entry `127.0.0.1:4020` also allowed
//! `127.0.0.1:9999`, a 402 over plain HTTP from a non-loopback host was payable, and an answer
//! body was read whole into memory whatever its size.
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

use spendguard::Policy;
use x402z_agent::{
    ledger::{LedgerFile, LedgerKey},
    pay::{self, AgentConfig, PayError},
    tab,
};

/// Serve forever, answering every request with `answer()`; returns host:port and a hit counter.
fn serve(answer: impl Fn() -> Vec<u8> + Send + 'static) -> (String, Arc<AtomicUsize>) {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = l.local_addr().unwrap();
    let hits = Arc::new(AtomicUsize::new(0));
    let h = hits.clone();
    std::thread::spawn(move || {
        for stream in l.incoming() {
            let Ok(mut s) = stream else { continue };
            h.fetch_add(1, Ordering::SeqCst);
            let mut r = BufReader::new(s.try_clone().unwrap());
            loop {
                let mut line = String::new();
                if r.read_line(&mut line).is_err() || line.trim_end().is_empty() {
                    break;
                }
            }
            let _ = s.write_all(&answer());
        }
    });
    (format!("127.0.0.1:{}", addr.port()), hits)
}

fn tmp(name: &str) -> PathBuf {
    let p = std::env::temp_dir().join(format!("x402z-agent-preflight-{}-{name}.json", std::process::id()));
    for q in [p.clone(), p.with_extension("json.lock"), p.with_extension("tab.json"), p.with_extension("tab.json.lock")] {
        let _ = std::fs::remove_file(q);
    }
    p
}

fn cfg(ledger: &PathBuf, allowed: &[&str]) -> AgentConfig {
    AgentConfig {
        ledger_path: Some(ledger.clone()),
        wallet_dir: std::env::temp_dir(),
        agent: "preflight-test".into(),
        policy: Policy { daily_cap_zat: 1_000_000, allowed_hosts: allowed.iter().map(|h| h.to_string()).collect(), approval_threshold_zat: None },
        seed: None,
        ledger_key: Some(LedgerKey::from_bytes([13; 32])),
        birthday: None,
        lwd_url: String::new(),
        confirm_timeout_secs: 60,
        poll_secs: 0,
    }
}

fn cleanup(ledger: &PathBuf) {
    for q in [ledger.clone(), ledger.with_extension("json.lock"), ledger.with_extension("tab.json"), ledger.with_extension("tab.json.lock")] {
        let _ = std::fs::remove_file(q);
    }
}

#[tokio::test]
async fn a_host_off_the_allowlist_or_on_another_port_never_sees_a_request() {
    let (host, hits) = serve(|| b"HTTP/1.1 402 Payment Required\r\ncontent-length: 2\r\nconnection: close\r\n\r\n{}".to_vec());
    let url = format!("http://{host}/api/quote");
    for (name, allowed) in [("other host", vec!["api.example.org"]), ("other port", vec!["127.0.0.1:1"]), ("https only", vec!["https://127.0.0.1"])] {
        let ledger = tmp(&name.replace(' ', "-"));
        let c = cfg(&ledger, &allowed);
        let e = pay::paid_fetch(&c, &url, &|_, _, _| false).await.unwrap_err();
        assert!(matches!(&e, PayError::Refused(m) if m.contains("not on the allowlist") && m.contains("Nothing was requested")), "{name}: {e:?}");
        let e = pay::redeem_payment(&c, &url, &"ab".repeat(32), "x402z-n").await.unwrap_err();
        assert!(matches!(&e, PayError::Refused(_)), "{name}: redeem_payment {e:?}");
        let e = tab::buy_or_resume(&c, &url, 2, &|_, _, _| false, false).await.unwrap_err();
        assert!(matches!(&e, PayError::Refused(_)), "{name}: tab purchase {e:?}");
        let e = tab::fetch(&c, &url).await.unwrap_err();
        assert!(matches!(&e, PayError::Refused(_)), "{name}: tab fetch {e:?}");
        let l = LedgerFile::load(&ledger, c.key().unwrap()).unwrap();
        assert_eq!(l.data.audit.len(), 3, "{name}: one denial per refused paying call: {:?}", l.data.audit);
        assert!(l.data.audit.iter().all(|e| e.decision == "denied" && e.txid.is_none() && e.memo_nonce.is_none()));
        cleanup(&ledger);
    }
    assert_eq!(hits.load(Ordering::SeqCst), 0, "the server never got a request");

    // the same host allowlisted with its port: the request is made
    let ledger = tmp("allowed");
    let c = cfg(&ledger, &[&host]);
    let e = pay::paid_fetch(&c, &url, &|_, _, _| false).await.unwrap_err();
    assert!(!matches!(e, PayError::Refused(_)), "{e:?}");
    assert_eq!(hits.load(Ordering::SeqCst), 1);
    cleanup(&ledger);
}

#[tokio::test]
async fn a_402_over_plain_http_from_a_non_loopback_host_is_refused_before_any_request() {
    let ledger = tmp("http");
    // TEST-NET-3: never routed; the refusal must come before any connection attempt
    let c = cfg(&ledger, &["203.0.113.7"]);
    let e = pay::paid_fetch(&c, "http://203.0.113.7:4020/api/quote", &|_, _, _| false).await.unwrap_err();
    assert!(matches!(&e, PayError::Refused(m) if m.contains("plain HTTP") && m.contains("X402Z_AGENT_ALLOW_HTTP")), "{e:?}");
    let l = LedgerFile::load(&ledger, c.key().unwrap()).unwrap();
    assert!(l.data.audit[0].reason.as_deref().unwrap().contains("plain HTTP"));
    cleanup(&ledger);
}

#[tokio::test]
async fn an_answer_body_over_the_limit_is_refused_not_read_into_memory() {
    let big = vec![b'x'; pay::DEFAULT_MAX_BODY_BYTES + 1];
    // with a Content-Length
    let b1 = big.clone();
    let (host1, _) = serve(move || {
        let mut r = format!("HTTP/1.1 402 Payment Required\r\ncontent-length: {}\r\nconnection: close\r\n\r\n", b1.len()).into_bytes();
        r.extend_from_slice(&b1);
        r
    });
    // without one (read until the server closes)
    let (host2, _) = serve(move || {
        let mut r = b"HTTP/1.1 402 Payment Required\r\nconnection: close\r\n\r\n".to_vec();
        r.extend_from_slice(&big);
        r
    });
    for host in [host1, host2] {
        let ledger = tmp("big");
        let c = cfg(&ledger, &[&host]);
        let e = pay::paid_fetch(&c, &format!("http://{host}/api/quote"), &|_, _, _| false).await.unwrap_err();
        assert!(matches!(&e, PayError::Runtime(m) if m.contains("X402Z_AGENT_MAX_BODY_BYTES")), "{e:?}");
        cleanup(&ledger);
    }
}
