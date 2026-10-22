//! Agent ledger at `X402Z_AGENT_LEDGER`: the JSON file the dashboard reads
//! (`{agents:[...], audit:[...]}`, see README). The agent persists every spendguard decision
//! here, and today's spend is rebuilt from audit entries that carry a broadcast txid, so the
//! daily cap holds across runs.
//!
//! Concurrent agent runs serialize on an exclusive lock on `<ledger>.lock` ([`LedgerFile::lock`]):
//! the check against the cap and the record of the broadcast happen under one lock, so two runs
//! can't both see room under the cap and both pay.
//!
//! At rest the file is sealed ([`spendguard::sealed`], XChaCha20-Poly1305) under a [`LedgerKey`]:
//! `X402Z_AGENT_LEDGER_KEY` (64 hex), or else one derived from `X402Z_AGENT_SEED`. Without
//! either the agent stops with "Setup required". A wrong key or a changed byte is an error, never
//! an empty ledger. The Kage Tab token file uses the same key.

use std::path::{Path, PathBuf};

use secrecy::{ExposeSecret, SecretVec};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use spendguard::{day_of, sealed, Policy, SpendGuard, SpendRequest};

/// Key that seals the ledger and the tab file at rest.
#[derive(Clone)]
pub struct LedgerKey([u8; 32]);

impl std::fmt::Debug for LedgerKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("LedgerKey(..)")
    }
}

impl LedgerKey {
    pub fn from_bytes(k: [u8; 32]) -> Self {
        Self(k)
    }

    /// `X402Z_AGENT_LEDGER_KEY`: 64 hex characters.
    pub fn from_hex(s: &str) -> Result<Self, String> {
        sealed::parse_key_hex(s).map(Self).map_err(|e| format!("X402Z_AGENT_LEDGER_KEY {e}"))
    }

    /// Derived from the wallet seed: SHA-256 of a fixed label and the BIP 39 seed.
    pub fn from_seed(seed: &SecretVec<u8>) -> Self {
        let mut h = Sha256::new();
        h.update(b"kage x402z-agent ledger key v1");
        h.update(seed.expose_secret());
        Self(h.finalize().into())
    }

    /// `X402Z_AGENT_LEDGER_KEY` if set, else derived from `seed`; `None` if neither.
    pub fn from_env(seed: Option<&SecretVec<u8>>) -> Result<Option<Self>, String> {
        match std::env::var("X402Z_AGENT_LEDGER_KEY").ok().map(|v| v.trim().to_string()).filter(|v| !v.is_empty()) {
            Some(hex) => Self::from_hex(&hex).map(Some),
            None => Ok(seed.map(Self::from_seed)),
        }
    }

    pub(crate) fn seal(&self, purpose: &str, plaintext: &[u8]) -> Vec<u8> {
        let mut nonce = [0u8; sealed::NONCE_LEN];
        rand::fill(&mut nonce[..]);
        sealed::seal(&self.0, purpose, nonce, plaintext)
    }

    pub(crate) fn open(&self, purpose: &str, data: &[u8]) -> Result<Vec<u8>, sealed::SealError> {
        sealed::open(&self.0, purpose, data)
    }
}

/// The "Setup required" message when no ledger key is configured.
pub const MISSING_KEY: &str = "Setup required: X402Z_AGENT_LEDGER_KEY not configured (64 hex, e.g. `openssl rand -hex 32`) and no \
     X402Z_AGENT_SEED to derive one from. The ledger and the tab file are encrypted at rest with it.";

/// Read a sealed file: missing => `None`; plaintext from an older version, a wrong key or a
/// changed byte => an error naming `what`.
pub(crate) fn read_sealed(path: &Path, key: &LedgerKey, purpose: &str, what: &str) -> Result<Option<Vec<u8>>, String> {
    let raw = match std::fs::read(path) {
        Ok(raw) => raw,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("{what} unreadable ({}): {e}", path.display())),
    };
    match key.open(purpose, &raw) {
        Ok(plain) => Ok(Some(plain)),
        Err(sealed::SealError::NotSealed) if raw.first() == Some(&b'{') => Err(format!(
            "{what} {} is plaintext JSON from before encryption at rest; run `x402z-agent seal-ledger` once to encrypt it",
            path.display()
        )),
        Err(e) => Err(format!("{what} {}: {e}", path.display())),
    }
}

/// Atomic sealed write (temp file, rename), so a crash never leaves half a file.
pub(crate) fn write_sealed(path: &Path, key: &LedgerKey, purpose: &str, plaintext: &[u8]) -> Result<(), String> {
    let tmp = path.with_extension("sealed.tmp");
    std::fs::write(&tmp, key.seal(purpose, plaintext)).map_err(|e| format!("cannot write {}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, path).map_err(|e| format!("cannot replace {}: {e}", path.display()))
}

/// Encrypt a plaintext file written by an older version in place. `Ok(false)` if it is missing
/// or already sealed.
pub fn seal_plaintext_file(path: &Path, key: &LedgerKey, purpose: &str) -> Result<bool, String> {
    let raw = match std::fs::read(path) {
        Ok(raw) => raw,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(e) => return Err(format!("{} unreadable: {e}", path.display())),
    };
    if sealed::is_sealed(&raw) {
        return Ok(false);
    }
    serde_json::from_slice::<serde_json::Value>(&raw).map_err(|e| format!("{} is neither sealed nor JSON: {e}", path.display()))?;
    write_sealed(path, key, purpose, &raw)?;
    Ok(true)
}

/// Sealed-file purpose of the ledger.
pub const LEDGER_PURPOSE: &str = sealed::LEDGER_PURPOSE;

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
    /// The resource a `paid_fetch` payment was for, so a retry can resume it instead of paying again.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// For a broadcast payment: `Some(true)` once the server answered 200 for it, `Some(false)`
    /// once it refused it for good, `None` while that is not known yet.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub settled: Option<bool>,
}

/// How long a broadcast but unsettled payment is resumed instead of paying again: the server
/// keeps a challenge redeemable this long by default (`X402Z_REDEEM_WINDOW_SECS`).
pub const RESUME_WINDOW_SECS: u64 = 24 * 60 * 60;

/// Exclusive advisory lock on the ledger, released when dropped (or when the process exits).
pub struct LedgerLock {
    _file: std::fs::File,
}

pub struct LedgerFile {
    path: PathBuf,
    key: LedgerKey,
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

    /// A missing file starts an empty ledger; an unreadable, invalid, plaintext or tampered one,
    /// or the wrong key, is an error.
    pub fn load(path: &Path, key: &LedgerKey) -> Result<Self, String> {
        let data = match read_sealed(path, key, LEDGER_PURPOSE, "X402Z_AGENT_LEDGER")? {
            None => Ledger::default(),
            Some(raw) => serde_json::from_slice(&raw)
                .map_err(|e| format!("X402Z_AGENT_LEDGER is not valid ledger JSON ({}): {e}", path.display()))?,
        };
        Ok(Self { path: path.to_path_buf(), key: key.clone(), data })
    }

    /// Sealed atomic replace, so a crash never leaves half a ledger.
    pub fn save(&self) -> Result<(), String> {
        let raw = serde_json::to_vec(&self.data).map_err(|e| e.to_string())?;
        write_sealed(&self.path, &self.key, LEDGER_PURPOSE, &raw)
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

    /// The newest payment `agent` broadcast for `url` that the server has neither settled nor
    /// refused, within [`RESUME_WINDOW_SECS`]. A retry must present this one, not pay again.
    pub fn unresolved_payment(&self, agent: &str, url: &str, now: u64) -> Option<&AuditEntry> {
        self.data.audit.iter().rev().find(|e| {
            e.agent == agent
                && e.url.as_deref() == Some(url)
                && e.txid.is_some()
                && e.memo_nonce.is_some()
                && e.settled.is_none()
                && e.ts.saturating_add(RESUME_WINDOW_SECS) > now
        })
    }

    /// Record the server's final answer for the payment `txid`. False if no entry has that txid.
    pub fn mark_settled(&mut self, txid: &str, settled: bool) -> bool {
        let mut found = false;
        for e in self.data.audit.iter_mut().filter(|e| e.txid.as_deref() == Some(txid)) {
            e.settled = Some(settled);
            found = true;
        }
        found
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
            url: None,
            settled: None,
        }
    }

    fn tmp(name: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!("x402z-ledger-{}-{name}.json", std::process::id()));
        let _ = std::fs::remove_file(&p);
        p
    }

    fn k() -> LedgerKey {
        LedgerKey::from_bytes([42; 32])
    }

    #[test]
    fn cap_survives_restarts_via_paid_audit_entries() {
        let path = tmp("cap");
        let mut l = LedgerFile::load(&path, &k()).unwrap();
        assert!(l.data.audit.is_empty());
        l.append(entry(DAY0 - 10, 200_000, Some("aa"))); // yesterday: does not count
        l.append(entry(DAY0 + 5, 100_000, Some("bb")));
        l.append(entry(DAY0 + 6, 100_000, Some("cc")));
        l.append(entry(DAY0 + 7, 100_000, None)); // allowed but never broadcast: does not count
        l.save().unwrap();

        let l = LedgerFile::load(&path, &k()).unwrap();
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
        let l = LedgerFile::load(&tmp("allow"), &k()).unwrap();
        let g = l.guard("agent-1", &policy(), DAY0);
        let r = SpendRequest { agent: "agent-1", host: "replay.local", amount_zat: 1, now_unix: DAY0 };
        assert!(matches!(g.check(&r), Decision::Deny(Denial::HostNotAllowed { .. })));
    }

    #[test]
    fn invalid_ledger_is_an_error_not_a_reset() {
        let path = tmp("bad");
        std::fs::write(&path, b"{not json").unwrap();
        let e = LedgerFile::load(&path, &k()).err().unwrap();
        assert!(e.contains("X402Z_AGENT_LEDGER"), "{e}");
        // valid JSON sealed under the right key but not a ledger is an error too
        write_sealed(&path, &k(), LEDGER_PURPOSE, b"[1,2]").unwrap();
        assert!(LedgerFile::load(&path, &k()).err().unwrap().contains("not valid ledger JSON"));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn concurrent_runs_cannot_both_pass_the_cap() {
        // Cap 250k with 200k already paid today: room for exactly one more 50k payment.
        let path = tmp("race");
        let mut l = LedgerFile::load(&path, &k()).unwrap();
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
                    let mut l = LedgerFile::load(&path, &k()).unwrap();
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
        let l = LedgerFile::load(&path, &k()).unwrap();
        assert_eq!(l.guard("agent-1", &policy(), DAY0 + 10).spent_today("agent-1", DAY0 + 10), 250_000);
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(path.with_extension("json.lock"));
    }

    #[test]
    fn unresolved_payments_are_found_per_url_until_settled_or_stale() {
        let mut l = LedgerFile::load(&tmp("resume"), &k()).unwrap();
        let mut paid = entry(DAY0, 100_000, Some("tx1"));
        paid.memo_nonce = Some("x402z-1".into());
        paid.url = Some("http://127.0.0.1:4020/api/quote".into());
        l.append(paid.clone());
        let mut other = paid.clone();
        other.url = Some("http://127.0.0.1:4020/other".into());
        other.txid = Some("tx2".into());
        l.append(other);
        l.append(entry(DAY0 + 1, 100_000, None)); // allowed, never broadcast: nothing to resume
        let u = "http://127.0.0.1:4020/api/quote";
        assert_eq!(l.unresolved_payment("agent-1", u, DAY0 + 10).unwrap().txid.as_deref(), Some("tx1"));
        assert!(l.unresolved_payment("agent-2", u, DAY0 + 10).is_none(), "per agent");
        assert!(l.unresolved_payment("agent-1", u, DAY0 + RESUME_WINDOW_SECS + 1).is_none(), "stale");
        assert!(l.mark_settled("tx1", true));
        assert!(l.unresolved_payment("agent-1", u, DAY0 + 10).is_none(), "settled");
        assert!(!l.mark_settled("nope", true));
    }

    #[test]
    fn the_ledger_is_sealed_at_rest_and_survives_a_restart() {
        let path = tmp("sealed");
        let mut l = LedgerFile::load(&path, &k()).unwrap();
        let mut e = entry(DAY0, 300_000, Some("d3bf0b49408fed118fe9e1241239d983"));
        e.host = "api.example.org:4020".into();
        e.memo_nonce = Some("x402z-58712c3f95459bb9".into());
        l.append(e);
        l.save().unwrap();
        // `strings` on the file shows none of it
        let raw = std::fs::read(&path).unwrap();
        for needle in ["api.example.org", "d3bf0b49", "x402z-", "agent-1", "audit", "300000"] {
            assert!(!raw.windows(needle.len()).any(|w| w == needle.as_bytes()), "{needle} readable on disk");
        }
        // restart: same key reads it back
        let l = LedgerFile::load(&path, &k()).unwrap();
        assert_eq!(l.data.audit[0].host, "api.example.org:4020");
        assert_eq!(l.guard("agent-1", &policy(), DAY0 + 5).spent_today("agent-1", DAY0 + 5), 300_000);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn a_wrong_key_a_changed_byte_or_an_old_plaintext_ledger_is_refused_not_reset() {
        let path = tmp("refuse");
        let mut l = LedgerFile::load(&path, &k()).unwrap();
        l.append(entry(DAY0, 200_000, Some("aa")));
        l.save().unwrap();
        let e = LedgerFile::load(&path, &LedgerKey::from_bytes([43; 32])).err().unwrap();
        assert!(e.contains("wrong key, or the file was changed"), "{e}");
        let mut raw = std::fs::read(&path).unwrap();
        let mid = raw.len() / 2;
        raw[mid] ^= 0x40;
        std::fs::write(&path, &raw).unwrap();
        assert!(LedgerFile::load(&path, &k()).err().unwrap().contains("wrong key, or the file was changed"));
        // a plaintext ledger from before encryption: refused with the migration command named...
        std::fs::write(&path, br#"{"agents":[],"audit":[{"ts":1,"agent":"a","host":"h","amountZat":5,"decision":"allowed","txid":"bb"}]}"#).unwrap();
        assert!(LedgerFile::load(&path, &k()).err().unwrap().contains("x402z-agent seal-ledger"));
        // ...which seals it in place, once
        assert!(seal_plaintext_file(&path, &k(), LEDGER_PURPOSE).unwrap());
        assert!(!seal_plaintext_file(&path, &k(), LEDGER_PURPOSE).unwrap());
        assert_eq!(LedgerFile::load(&path, &k()).unwrap().data.audit[0].txid.as_deref(), Some("bb"));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn the_key_comes_from_the_env_or_the_seed() {
        let seed = SecretVec::new(vec![1u8; 64]);
        let a = LedgerKey::from_seed(&seed);
        assert_eq!(a.0, LedgerKey::from_seed(&SecretVec::new(vec![1u8; 64])).0, "deterministic");
        assert_ne!(a.0, LedgerKey::from_seed(&SecretVec::new(vec![2u8; 64])).0);
        assert!(LedgerKey::from_hex("nothex").unwrap_err().contains("X402Z_AGENT_LEDGER_KEY"));
    }

    #[test]
    fn upsert_and_dashboard_shape() {
        let mut l = LedgerFile::load(&tmp("shape"), &k()).unwrap();
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
