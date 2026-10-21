//! Agent ledger at `X402Z_AGENT_LEDGER`: the JSON file the dashboard reads
//! (`{agents:[...], audit:[...]}`, see README). The agent persists every spendguard decision
//! here, and today's spend is rebuilt from audit entries that carry a broadcast txid, so the
//! daily cap holds across runs.
//!
//! Concurrent agent runs serialize on an exclusive lock on `<ledger>.lock` ([`LedgerFile::lock`]):
//! the check against the cap and the record of the broadcast happen under one lock, so two runs
//! can't both see room under the cap and both pay.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use spendguard::{day_of, Policy, SpendGuard, SpendRequest};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Ledger {
    #[serde(default)]
    pub agents: Vec<AgentRow>,
    #[serde(default)]
    pub audit: Vec<AuditEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentRow {
    pub agent: String,
    pub daily_cap_zat: u64,
    pub spent_today_zat: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approval_threshold_zat: Option<u64>,
    pub allowed_hosts: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuditEntry {
    pub ts: u64,
    pub agent: String,
    pub host: String,
    pub amount_zat: u64,
    /// `allowed` | `denied` | `needs_approval` (dashboard tag names).
    pub decision: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memo_nonce: Option<String>,
    /// Set only once a payment was actually broadcast.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub txid: Option<String>,
}

/// Exclusive advisory lock on the ledger, released when dropped (or when the process exits).
pub struct LedgerLock {
    _file: std::fs::File,
}

pub struct LedgerFile {
    path: PathBuf,
    pub data: Ledger,
}

impl LedgerFile {
    /// Block until this process holds the exclusive lock for the ledger at `path`.
    pub fn lock(path: &Path) -> Result<LedgerLock, String> {
        let lock_path = path.with_extension("json.lock");
        let file = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(&lock_path)
            .map_err(|e| format!("cannot open ledger lock {}: {e}", lock_path.display()))?;
        file.lock().map_err(|e| format!("cannot lock {}: {e}", lock_path.display()))?;
        Ok(LedgerLock { _file: file })
    }

    /// A missing file starts an empty ledger; an unreadable or invalid one is an error.
    pub fn load(path: &Path) -> Result<Self, String> {
        let data = match std::fs::read(path) {
            Ok(raw) => serde_json::from_slice(&raw)
                .map_err(|e| format!("X402Z_AGENT_LEDGER is not valid ledger JSON ({}): {e}", path.display()))?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ledger::default(),
            Err(e) => return Err(format!("X402Z_AGENT_LEDGER unreadable ({}): {e}", path.display())),
        };
        Ok(Self { path: path.to_path_buf(), data })
    }

    /// Atomic replace (write temp file, rename), so a crash never leaves half a ledger.
    pub fn save(&self) -> Result<(), String> {
        let tmp = self.path.with_extension("json.tmp");
        let raw = serde_json::to_vec_pretty(&self.data).map_err(|e| e.to_string())?;
        std::fs::write(&tmp, raw).map_err(|e| format!("cannot write {}: {e}", tmp.display()))?;
        std::fs::rename(&tmp, &self.path).map_err(|e| format!("cannot replace {}: {e}", self.path.display()))
    }

    /// Paid (broadcast) amounts for `agent` on the UTC day of `now`.
    pub fn paid_today(&self, agent: &str, now: u64) -> Vec<(String, u64, u64)> {
        self.data
            .audit
            .iter()
            .filter(|e| e.agent == agent && e.txid.is_some() && day_of(e.ts) == day_of(now))
            .map(|e| (e.host.clone(), e.amount_zat, e.ts))
            .collect()
    }

    /// A guard holding `policy` for `agent`, preloaded with today's broadcast payments.
    pub fn guard(&self, agent: &str, policy: &Policy, now: u64) -> SpendGuard {
        let mut g = SpendGuard::new();
        // Replay history under a permissive copy of the policy: past payments happened, and the
        // current allowlist/cap must not erase them from today's total.
        let replay = Policy { daily_cap_zat: u64::MAX, allowed_hosts: vec!["replay.local".into()], approval_threshold_zat: None };
        g.set_policy(agent, replay);
        let mut total: u64 = 0;
        for (_, amt, _) in self.paid_today(agent, now) {
            total = total.saturating_add(amt);
        }
        if total > 0 {
            let _ = g.record_spend(&SpendRequest { agent, host: "replay.local", amount_zat: total, now_unix: now });
        }
        g.set_policy(agent, policy.clone());
        g
    }

    pub fn upsert_agent(&mut self, agent: &str, policy: &Policy, spent_today_zat: u64) {
        let row = AgentRow {
            agent: agent.to_string(),
            daily_cap_zat: policy.daily_cap_zat,
            spent_today_zat,
            approval_threshold_zat: policy.approval_threshold_zat,
            allowed_hosts: policy.allowed_hosts.clone(),
        };
        match self.data.agents.iter_mut().find(|a| a.agent == agent) {
            Some(a) => *a = row,
            None => self.data.agents.push(row),
        }
    }

    pub fn append(&mut self, e: AuditEntry) {
        self.data.audit.push(e);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use spendguard::{Decision, Denial, SECS_PER_DAY};

    const DAY0: u64 = 1_792_000_000 / SECS_PER_DAY * SECS_PER_DAY;

    fn policy() -> Policy {
        Policy { daily_cap_zat: 250_000, allowed_hosts: vec!["127.0.0.1".into()], approval_threshold_zat: None }
    }

    fn entry(ts: u64, amount: u64, txid: Option<&str>) -> AuditEntry {
        AuditEntry {
            ts,
            agent: "agent-1".into(),
            host: "127.0.0.1:4020".into(),
            amount_zat: amount,
            decision: "allowed".into(),
            reason: None,
            memo_nonce: None,
            txid: txid.map(str::to_string),
        }
    }

    fn tmp(name: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!("x402z-ledger-{}-{name}.json", std::process::id()));
        let _ = std::fs::remove_file(&p);
        p
    }

    #[test]
    fn cap_survives_restarts_via_paid_audit_entries() {
        let path = tmp("cap");
        let mut l = LedgerFile::load(&path).unwrap();
        assert!(l.data.audit.is_empty());
        l.append(entry(DAY0 - 10, 200_000, Some("aa"))); // yesterday: does not count
        l.append(entry(DAY0 + 5, 100_000, Some("bb")));
        l.append(entry(DAY0 + 6, 100_000, Some("cc")));
        l.append(entry(DAY0 + 7, 100_000, None)); // allowed but never broadcast: does not count
        l.save().unwrap();

        let l = LedgerFile::load(&path).unwrap();
        let g = l.guard("agent-1", &policy(), DAY0 + 100);
        assert_eq!(g.spent_today("agent-1", DAY0 + 100), 200_000);
        let req = |amt| SpendRequest { agent: "agent-1", host: "127.0.0.1:4020", amount_zat: amt, now_unix: DAY0 + 100 };
        assert_eq!(g.check(&req(50_000)), Decision::Allow);
        assert_eq!(
            g.check(&req(100_000)),
            Decision::Deny(Denial::CapExceeded { cap_zat: 250_000, spent_today_zat: 200_000, requested_zat: 100_000 })
        );
        // next UTC day starts from zero
        assert_eq!(l.guard("agent-1", &policy(), DAY0 + SECS_PER_DAY).spent_today("agent-1", DAY0 + SECS_PER_DAY), 0);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn replay_does_not_bypass_allowlist() {
        let l = LedgerFile::load(&tmp("allow")).unwrap();
        let g = l.guard("agent-1", &policy(), DAY0);
        let r = SpendRequest { agent: "agent-1", host: "replay.local", amount_zat: 1, now_unix: DAY0 };
        assert!(matches!(g.check(&r), Decision::Deny(Denial::HostNotAllowed { .. })));
    }

    #[test]
    fn invalid_ledger_is_an_error_not_a_reset() {
        let path = tmp("bad");
        std::fs::write(&path, b"{not json").unwrap();
        let e = LedgerFile::load(&path).err().unwrap();
        assert!(e.contains("X402Z_AGENT_LEDGER"), "{e}");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn concurrent_runs_cannot_both_pass_the_cap() {
        // Cap 250k with 200k already paid today: room for exactly one more 50k payment.
        let path = tmp("race");
        let mut l = LedgerFile::load(&path).unwrap();
        l.append(entry(DAY0 + 1, 200_000, Some("aa")));
        l.save().unwrap();

        let barrier = std::sync::Arc::new(std::sync::Barrier::new(6));
        let runs: Vec<_> = (0..6)
            .map(|i| {
                let (path, barrier) = (path.clone(), barrier.clone());
                std::thread::spawn(move || {
                    barrier.wait();
                    // What `x402z-agent pay` does: lock, load, check, pay, record, unlock.
                    let _lock = LedgerFile::lock(&path).unwrap();
                    let mut l = LedgerFile::load(&path).unwrap();
                    let now = DAY0 + 10;
                    let g = l.guard("agent-1", &policy(), now);
                    let req = SpendRequest { agent: "agent-1", host: "127.0.0.1:4020", amount_zat: 50_000, now_unix: now };
                    if g.check(&req) != Decision::Allow {
                        return false;
                    }
                    std::thread::sleep(std::time::Duration::from_millis(30)); // building + broadcasting
                    l.append(entry(now, 50_000, Some(&format!("tx{i}"))));
                    l.save().unwrap();
                    true
                })
            })
            .collect();
        let paid = runs.into_iter().map(|h| h.join().unwrap()).filter(|p| *p).count();
        assert_eq!(paid, 1, "exactly one run may pay");
        let l = LedgerFile::load(&path).unwrap();
        assert_eq!(l.guard("agent-1", &policy(), DAY0 + 10).spent_today("agent-1", DAY0 + 10), 250_000);
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(path.with_extension("json.lock"));
    }

    #[test]
    fn upsert_and_dashboard_shape() {
        let mut l = LedgerFile::load(&tmp("shape")).unwrap();
        l.upsert_agent("agent-1", &policy(), 10);
        l.upsert_agent("agent-1", &policy(), 20);
        l.append(entry(DAY0, 5, Some("dd")));
        let v = serde_json::to_value(&l.data).unwrap();
        assert_eq!(v["agents"].as_array().unwrap().len(), 1);
        assert_eq!(v["agents"][0]["spentTodayZat"], 20);
        assert_eq!(v["agents"][0]["dailyCapZat"], 250_000);
        assert_eq!(v["audit"][0]["amountZat"], 5);
        assert_eq!(v["audit"][0]["decision"], "allowed");
    }
}
