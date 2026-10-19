//! spendguard: decides whether an agent may pay a 402 quote *before* any
//! transaction is built or broadcast.
//!
//! Rules, checked in order:
//! 1. host must match the allowlist (deny by default; an empty list allows nothing)
//! 2. amount must be > 0
//! 3. today's spend + amount must not exceed the agent's daily cap
//! 4. amounts at or above `approval_threshold_zat` need a human approval
//!
//! Days are UTC days (unix seconds / 86400). All amounts are zatoshi (u64);
//! sums use checked arithmetic so an overflow is a denial, never a wrap.

use std::collections::HashMap;
use std::fmt;

pub const SECS_PER_DAY: u64 = 86_400;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Policy {
    pub daily_cap_zat: u64,
    /// Exact hosts (`api.example.com`) or suffix wildcards (`*.example.com`).
    pub allowed_hosts: Vec<String>,
    /// Payments >= this need approval. `None` means never ask.
    pub approval_threshold_zat: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Denial {
    HostNotAllowed { host: String },
    ZeroAmount,
    CapExceeded { cap_zat: u64, spent_today_zat: u64, requested_zat: u64 },
    UnknownAgent { agent: String },
    Overflow,
}

impl fmt::Display for Denial {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Denial::HostNotAllowed { host } => write!(f, "host '{host}' is not on the allowlist"),
            Denial::ZeroAmount => write!(f, "amount must be greater than zero"),
            Denial::CapExceeded { cap_zat, spent_today_zat, requested_zat } => write!(
                f,
                "daily cap exceeded: spent {spent_today_zat} + requested {requested_zat} > cap {cap_zat} zat"
            ),
            Denial::UnknownAgent { agent } => write!(f, "no policy configured for agent '{agent}'"),
            Denial::Overflow => write!(f, "amount overflow"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    Allow,
    NeedsApproval { threshold_zat: u64 },
    Deny(Denial),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpendRequest<'a> {
    pub agent: &'a str,
    pub host: &'a str,
    pub amount_zat: u64,
    pub now_unix: u64,
}

/// Normalise a host: lowercase, strip a trailing dot and any `:port`.
pub fn normalize_host(host: &str) -> String {
    let h = host.trim().to_ascii_lowercase();
    let h = match h.rsplit_once(':') {
        Some((name, port)) if !name.is_empty() && port.chars().all(|c| c.is_ascii_digit()) => name.to_string(),
        _ => h,
    };
    h.trim_end_matches('.').to_string()
}

pub fn host_allowed(allowed: &[String], host: &str) -> bool {
    let host = normalize_host(host);
    if host.is_empty() {
        return false;
    }
    allowed.iter().any(|pattern| {
        let p = normalize_host(pattern);
        match p.strip_prefix("*.") {
            // `*.example.com` matches sub.example.com but not example.com or badexample.com
            Some(suffix) => !suffix.is_empty() && host.ends_with(&format!(".{suffix}")),
            None => !p.is_empty() && p == host,
        }
    })
}

pub fn day_of(unix: u64) -> u64 {
    unix / SECS_PER_DAY
}

/// In-memory guard. Callers persist via the audit log; `record_spend` must be
/// called only after a payment was actually broadcast.
#[derive(Debug, Default)]
pub struct SpendGuard {
    policies: HashMap<String, Policy>,
    /// (agent, utc day) -> zat spent
    spent: HashMap<(String, u64), u64>,
}

impl SpendGuard {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set_policy(&mut self, agent: impl Into<String>, policy: Policy) {
        self.policies.insert(agent.into(), policy);
    }

    pub fn spent_today(&self, agent: &str, now_unix: u64) -> u64 {
        self.spent.get(&(agent.to_string(), day_of(now_unix))).copied().unwrap_or(0)
    }

    pub fn remaining_today(&self, agent: &str, now_unix: u64) -> Option<u64> {
        let p = self.policies.get(agent)?;
        Some(p.daily_cap_zat.saturating_sub(self.spent_today(agent, now_unix)))
    }

    pub fn check(&self, req: &SpendRequest<'_>) -> Decision {
        let Some(policy) = self.policies.get(req.agent) else {
            return Decision::Deny(Denial::UnknownAgent { agent: req.agent.to_string() });
        };
        if !host_allowed(&policy.allowed_hosts, req.host) {
            return Decision::Deny(Denial::HostNotAllowed { host: normalize_host(req.host) });
        }
        if req.amount_zat == 0 {
            return Decision::Deny(Denial::ZeroAmount);
        }
        let spent = self.spent_today(req.agent, req.now_unix);
        let Some(total) = spent.checked_add(req.amount_zat) else {
            return Decision::Deny(Denial::Overflow);
        };
        if total > policy.daily_cap_zat {
            return Decision::Deny(Denial::CapExceeded {
                cap_zat: policy.daily_cap_zat,
                spent_today_zat: spent,
                requested_zat: req.amount_zat,
            });
        }
        match policy.approval_threshold_zat {
            Some(t) if req.amount_zat >= t => Decision::NeedsApproval { threshold_zat: t },
            _ => Decision::Allow,
        }
    }

    /// Record a broadcast payment. Re-checks the cap so a racing caller cannot
    /// push the agent over it.
    pub fn record_spend(&mut self, req: &SpendRequest<'_>) -> Result<u64, Denial> {
        if let Decision::Deny(d) = self.check(req) {
            return Err(d);
        }
        let key = (req.agent.to_string(), day_of(req.now_unix));
        let entry = self.spent.entry(key).or_insert(0);
        *entry = entry.checked_add(req.amount_zat).ok_or(Denial::Overflow)?;
        Ok(*entry)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DAY0: u64 = 1_792_000_000 / SECS_PER_DAY * SECS_PER_DAY; // start of a UTC day

    fn guard() -> SpendGuard {
        let mut g = SpendGuard::new();
        g.set_policy(
            "agent-1",
            Policy {
                daily_cap_zat: 500_000,
                allowed_hosts: vec!["api.example.com".into(), "*.data.test".into()],
                approval_threshold_zat: Some(200_000),
            },
        );
        g
    }

    fn req(host: &str, amount: u64, now: u64) -> SpendRequest<'_> {
        SpendRequest { agent: "agent-1", host, amount_zat: amount, now_unix: now }
    }

    #[test]
    fn allows_within_cap_on_allowlisted_host() {
        assert_eq!(guard().check(&req("api.example.com", 100_000, DAY0)), Decision::Allow);
    }

    #[test]
    fn host_normalisation_port_case_trailing_dot() {
        let g = guard();
        assert_eq!(g.check(&req("API.Example.com:8443", 1, DAY0)), Decision::Allow);
        assert_eq!(g.check(&req("api.example.com.", 1, DAY0)), Decision::Allow);
    }

    #[test]
    fn wildcard_matches_subdomains_only() {
        let g = guard();
        assert_eq!(g.check(&req("x.data.test", 1, DAY0)), Decision::Allow);
        assert_eq!(g.check(&req("a.b.data.test", 1, DAY0)), Decision::Allow);
        for bad in ["data.test", "evildata.test", "data.test.evil.com"] {
            assert!(matches!(g.check(&req(bad, 1, DAY0)), Decision::Deny(Denial::HostNotAllowed { .. })), "{bad}");
        }
    }

    #[test]
    fn empty_allowlist_denies_everything() {
        let mut g = SpendGuard::new();
        g.set_policy("a", Policy { daily_cap_zat: 10, allowed_hosts: vec![], approval_threshold_zat: None });
        let r = SpendRequest { agent: "a", host: "api.example.com", amount_zat: 1, now_unix: DAY0 };
        assert!(matches!(g.check(&r), Decision::Deny(Denial::HostNotAllowed { .. })));
    }

    #[test]
    fn unknown_agent_and_zero_amount_denied() {
        let g = guard();
        let r = SpendRequest { agent: "nobody", host: "api.example.com", amount_zat: 1, now_unix: DAY0 };
        assert!(matches!(g.check(&r), Decision::Deny(Denial::UnknownAgent { .. })));
        assert_eq!(g.check(&req("api.example.com", 0, DAY0)), Decision::Deny(Denial::ZeroAmount));
    }

    #[test]
    fn cap_is_inclusive_and_cumulative() {
        let mut g = guard();
        g.record_spend(&req("api.example.com", 150_000, DAY0)).unwrap();
        g.record_spend(&req("api.example.com", 150_000, DAY0 + 10)).unwrap();
        g.record_spend(&req("api.example.com", 150_000, DAY0 + 20)).unwrap();
        // exactly at cap is fine
        assert_eq!(g.check(&req("api.example.com", 50_000, DAY0 + 30)), Decision::Allow);
        g.record_spend(&req("api.example.com", 50_000, DAY0 + 30)).unwrap();
        assert_eq!(g.remaining_today("agent-1", DAY0 + 30), Some(0));
        assert_eq!(
            g.check(&req("api.example.com", 1, DAY0 + 40)),
            Decision::Deny(Denial::CapExceeded { cap_zat: 500_000, spent_today_zat: 500_000, requested_zat: 1 })
        );
    }

    #[test]
    fn record_spend_refuses_over_cap_and_does_not_mutate() {
        let mut g = guard();
        assert!(g.record_spend(&req("api.example.com", 600_000, DAY0)).is_err());
        assert_eq!(g.spent_today("agent-1", DAY0), 0);
    }

    #[test]
    fn cap_resets_at_utc_midnight() {
        let mut g = guard();
        g.record_spend(&req("api.example.com", 190_000, DAY0 + SECS_PER_DAY - 1)).unwrap();
        g.record_spend(&req("api.example.com", 190_000, DAY0 + SECS_PER_DAY - 1)).unwrap();
        assert!(matches!(g.check(&req("api.example.com", 190_000, DAY0 + SECS_PER_DAY - 1)), Decision::Deny(_)));
        assert_eq!(g.check(&req("api.example.com", 190_000, DAY0 + SECS_PER_DAY)), Decision::Allow);
    }

    #[test]
    fn approval_threshold_inclusive() {
        let g = guard();
        assert_eq!(g.check(&req("api.example.com", 199_999, DAY0)), Decision::Allow);
        assert_eq!(
            g.check(&req("api.example.com", 200_000, DAY0)),
            Decision::NeedsApproval { threshold_zat: 200_000 }
        );
    }

    #[test]
    fn deny_beats_approval() {
        // over cap and over threshold -> deny, never prompt
        assert!(matches!(guard().check(&req("api.example.com", 600_000, DAY0)), Decision::Deny(Denial::CapExceeded { .. })));
    }

    #[test]
    fn overflow_is_denied() {
        let mut g = SpendGuard::new();
        g.set_policy("a", Policy { daily_cap_zat: u64::MAX, allowed_hosts: vec!["h.test".into()], approval_threshold_zat: None });
        let r = |amt| SpendRequest { agent: "a", host: "h.test", amount_zat: amt, now_unix: DAY0 };
        g.record_spend(&r(u64::MAX)).unwrap();
        assert_eq!(g.check(&r(1)), Decision::Deny(Denial::Overflow));
    }
}
