//! B-300: real encrypted LedgerFile and SpendGuard, with simulated paid rows only.
//! No wallet, network client, transaction construction, or broadcast is invoked.
use std::{path::PathBuf, sync::{Arc, Barrier}, time::{SystemTime, UNIX_EPOCH}};
use spendguard::{Decision, Denial, Policy, SpendRequest};
use x402z_agent::ledger::{AuditEntry, LedgerFile, LedgerKey};

fn main() {
    let path = PathBuf::from(std::env::args_os().nth(1).expect("temporary ledger path"));
    assert!(!path.exists(), "probe must start with a fresh ledger");
    let price = 100_000;
    let agent = "b300-probe";
    let host = "127.0.0.1:4020";
    let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs();
    let policy = Policy { daily_cap_zat: price, allowed_hosts: vec![host.into()], approval_threshold_zat: None };
    // Ephemeral ledger-only encryption key; never logged or used as a wallet key.
    let key = LedgerKey::from_bytes(rand::random());
    {
        let _lock = LedgerFile::lock(&path).unwrap();
        let mut ledger = LedgerFile::load(&path, &key).unwrap();
        assert!(ledger.data.audit.is_empty());
        let spent = ledger.guard(agent, &policy, now).spent_today(agent, now);
        assert_eq!(spent, 0);
        ledger.upsert_agent(agent, &policy, spent);
        ledger.save().unwrap();
        println!("initial spent_today_zat={spent}; price_zat={price}; daily_cap_zat={price}");
    }
    let barrier = Arc::new(Barrier::new(2));
    let threads: Vec<_> = (0..2).map(|i| {
        let (path, key, policy, barrier) = (path.clone(), key.clone(), policy.clone(), barrier.clone());
        std::thread::spawn(move || {
            barrier.wait();
            let _lock = LedgerFile::lock(&path).unwrap();
            let mut ledger = LedgerFile::load(&path, &key).unwrap();
            let mut guard = ledger.guard(agent, &policy, now);
            let request = SpendRequest { agent, host, amount_zat: price, now_unix: now };
            let allowed = match guard.check(&request) {
                Decision::Allow => {
                    // Widen the race while holding the actual file lock. This is only a
                    // synthetic paid audit entry; there is no payment implementation here.
                    std::thread::sleep(std::time::Duration::from_millis(50));
                    assert_eq!(guard.record_spend(&request).unwrap(), price);
                    true
                }
                Decision::Deny(Denial::CapExceeded { cap_zat, spent_today_zat, requested_zat }) => {
                    assert_eq!((cap_zat, spent_today_zat, requested_zat), (price, price, price));
                    false
                }
                other => panic!("unexpected guard decision: {other:?}"),
            };
            ledger.append(AuditEntry {
                ts: now, agent: agent.into(), host: host.into(), amount_zat: price,
                decision: if allowed { "allowed" } else { "denied" }.into(),
                reason: Some(if allowed { "probe simulated paid row; no broadcast" } else { "daily cap exceeded" }.into()),
                memo_nonce: None, txid: allowed.then(|| format!("{i:064x}")),
                url: None, settled: allowed.then_some(true),
            });
            ledger.upsert_agent(agent, &policy, guard.spent_today(agent, now));
            ledger.save().unwrap();
            allowed
        })
    }).collect();
    let allowed = threads.into_iter().map(|t| t.join().unwrap()).filter(|v| *v).count();
    assert_eq!(allowed, 1);
    let _lock = LedgerFile::lock(&path).unwrap();
    let ledger = LedgerFile::load(&path, &key).unwrap();
    assert_eq!(ledger.data.audit.len(), 2);
    let paid = ledger.data.audit.iter().filter(|e| e.decision == "allowed" && e.txid.is_some() && e.settled == Some(true)).count();
    let denied = ledger.data.audit.iter().filter(|e| e.decision == "denied" && e.txid.is_none()).count();
    assert_eq!((paid, denied), (1, 1));
    assert_eq!(ledger.data.audit.iter().filter(|e| e.txid.is_some()).count(), 1);
    let spent = ledger.guard(agent, &policy, now).spent_today(agent, now);
    assert_eq!(spent, price);
    assert_eq!(ledger.data.agents[0].spent_today_zat, price);
    assert_eq!(ledger.paid_today(agent, now).iter().map(|(_, amount, _)| amount).sum::<u64>(), price);
    println!("PASS B-300: attempts=2 allowed_paid_rows={paid} denied_rows={denied} txid_bearing_rows=1 spent_today_zat={spent} price_zat={price}");
    println!("Ledger-only simulation; synthetic txid; no wallet, seed, lightwalletd, or broadcast.");
}
