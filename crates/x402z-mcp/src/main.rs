//! `x402z-mcp`: a Model Context Protocol server over stdio (newline-delimited JSON-RPC 2.0) that
//! lets an LLM agent pay for HTTP APIs with shielded Zcash testnet payments, inside spendguard.
//!
//! Tools:
//! - `paid_fetch {url}`: GET the URL; on an x402 402, pay it from the agent's own wallet if
//!   spendguard allows (cap, allowlist, approval threshold), then present `X-PAYMENT` once. It
//!   returns right after the broadcast with `status: "settlement_pending"` and the txid when the
//!   payment still needs its block (MCP clients time requests out at about 60 s; a block takes
//!   about 75 s). Calling it again for the same URL presents that payment again and never pays
//!   twice. Spendguard refusals, wallet problems and rejected payments come back as tool errors
//!   (`isError: true`) naming the reason; nothing is paid in those cases.
//! - `redeem_payment {url, txid, memoNonce}`: present an earlier payment with `X-PAYMENT` once.
//!   Never pays.
//! - `spend_status {}`: today's spend against the daily cap, allowlist, approval threshold.
//! - `ledger_tail {n?}`: the last n spendguard decisions (default 10, max 100).
//! - `tab_fetch {url, buy?}`: GET the URL spending one Kage Tab token (one shielded payment
//!   bought N unlinkable blind-signed tokens). With no token left it buys a tab of `buy` tokens
//!   (default `X402Z_AGENT_TAB_SIZE`, 5) from the agent's own wallet, inside spendguard, first,
//!   and returns `status: "tab_pending"` while that payment confirms; the next call collects the
//!   tokens without paying again.
//!
//! Requests are served concurrently; stdout carries one JSON-RPC message per line.
//!
//! Configuration is the agent's (`X402Z_AGENT_*`, `ZCASH_LWD_URL`; see `.env.example`), read on
//! every call so a missing variable is reported as "Setup required" instead of a crash.
//! Approval-threshold payments are always declined here: an MCP client cannot give the
//! operator's approval, so raise `X402Z_AGENT_APPROVAL_ZAT` or pay those with the CLI.
//! stdout carries only JSON-RPC; logs go to stderr.

use serde_json::{json, Value};
use spendguard::day_of;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use x402z_agent::{
    ledger::LedgerFile,
    pay::{self, AgentConfig, PayError},
    tab,
};

const SUPPORTED_VERSIONS: [&str; 3] = ["2025-06-18", "2025-03-26", "2024-11-05"];

fn tools() -> Value {
    json!([
        {
            "name": "paid_fetch",
            "title": "Fetch a URL, paying its x402 402 with shielded Zcash (testnet)",
            "description": "GET an HTTP URL. If it answers HTTP 402 with an x402 zcash-shielded offer, pay it from the agent's own Zcash testnet wallet (memo = the challenge nonce) when spendguard allows it (daily cap, host allowlist, approval threshold), then present X-PAYMENT. The server settles a payment once it has 1 block confirmation (about 75 s, sometimes several minutes), so this usually returns status \"settlement_pending\" with the txid right after paying: then call redeem_payment (or paid_fetch again for the same URL) a minute later. Neither pays a second time. Refusals are errors and nothing is paid.",
            "inputSchema": {
                "type": "object",
                "properties": { "url": { "type": "string", "description": "http(s) URL of the paid resource" } },
                "required": ["url"],
                "additionalProperties": false
            }
        },
        {
            "name": "redeem_payment",
            "title": "Present an earlier payment again (never pays)",
            "description": "Present a payment this agent already broadcast (txid and memoNonce from paid_fetch) to the URL with X-PAYMENT, once. Returns the paid response with X-PAYMENT-RESPONSE when the server settles it, or status \"settlement_pending\" while the transaction still waits for its block. An already_redeemed error means the payment settled earlier and its answer was lost: do not pay again. Never builds or broadcasts a transaction.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "url": { "type": "string", "description": "the URL paid_fetch was called with" },
                    "txid": { "type": "string", "description": "64-hex transaction id from paid_fetch" },
                    "memoNonce": { "type": "string", "description": "the challenge nonce from paid_fetch" }
                },
                "required": ["url", "txid", "memoNonce"],
                "additionalProperties": false
            }
        },
        {
            "name": "tab_fetch",
            "title": "Fetch a URL with a Kage Tab token (one payment, N unlinkable requests)",
            "description": "GET an HTTP URL spending one prepaid Kage Tab token: an RFC 9474 blind-RSA token the server signed without seeing it, so it can't link this request to the shielded payment that bought it or to the other tokens. A token request needs no new transaction and returns in milliseconds. If no token is left for the URL's server, first buy a tab of `buy` tokens (default 5) with one shielded Zcash testnet payment from the agent's own wallet, inside spendguard (the whole tab price counts against the daily cap); that purchase waits for 1 block. Returns the response and how many tokens remain.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "url": { "type": "string", "description": "http(s) URL of the paid resource" },
                    "buy": { "type": "integer", "minimum": 1, "maximum": 100, "description": "tab size to buy when no token is left (default X402Z_AGENT_TAB_SIZE or 5)" }
                },
                "required": ["url"],
                "additionalProperties": false
            }
        },
        {
            "name": "spend_status",
            "title": "Spend so far today vs the daily cap",
            "description": "The agent's spendguard state: spent today (UTC day, broadcast payments only), daily cap, remaining, allowed hosts and approval threshold. All amounts in zatoshi (1 TAZ = 100,000,000 zat).",
            "inputSchema": { "type": "object", "properties": {}, "additionalProperties": false }
        },
        {
            "name": "ledger_tail",
            "title": "Recent spendguard decisions",
            "description": "The last n spendguard decisions from the agent ledger (allowed, denied, needs_approval), newest last, with host, amount, reason, memo nonce and txid when broadcast.",
            "inputSchema": {
                "type": "object",
                "properties": { "n": { "type": "integer", "minimum": 1, "maximum": 100, "description": "how many entries (default 10)" } },
                "additionalProperties": false
            }
        }
    ])
}

fn text_result(v: &Value, is_error: bool) -> Value {
    json!({
        "content": [{ "type": "text", "text": serde_json::to_string_pretty(v).unwrap_or_default() }],
        "structuredContent": v,
        "isError": is_error
    })
}

fn tool_error(kind: &str, message: &str) -> Value {
    text_result(&json!({ "error": kind, "message": message }), true)
}

/// The tool-error kind of `e`: missing or invalid configuration is always "setup_required".
fn error_kind(e: &PayError) -> &'static str {
    if matches!(e, PayError::Config(_)) {
        "setup_required"
    } else {
        e.kind()
    }
}

fn setup_required(e: &PayError) -> Value {
    tool_error(error_kind(e), e.message())
}

/// Approvals can't be given through MCP; decline and say why (it lands in the ledger too).
fn decline(host: &str, amount: u64, threshold: u64) -> bool {
    eprintln!("x402z-mcp: {amount} zat to {host} is at or above the approval threshold {threshold} zat; declined (no operator approval over MCP)");
    false
}

fn paid_result(r: Result<pay::PaidFetch, PayError>) -> Value {
    match r {
        Ok(pay::PaidFetch::Done(f)) => {
            let body = serde_json::from_str::<Value>(&f.body).unwrap_or(Value::String(f.body.clone()));
            let paid = f.paid.map(|(txid, amount, nonce)| {
                json!({ "txid": txid, "amountZat": amount, "memoNonce": nonce, "explorer": format!("https://testnet.zecblock.com/tx/{txid}") })
            });
            text_result(&json!({ "status": f.status, "paid": paid, "settlement": f.settlement, "body": body }), false)
        }
        Ok(pay::PaidFetch::Pending(p)) => text_result(
            &json!({
                "status": "settlement_pending",
                "url": p.url, "txid": p.txid, "memoNonce": p.memo_nonce, "amountZat": p.amount_zat,
                "resumed": p.resumed, "message": p.message,
                "explorer": format!("https://testnet.zecblock.com/tx/{}", p.txid),
                "next": "The payment is broadcast and waits for its block (about 75 s). Call redeem_payment with this url, txid and memoNonce in about a minute, or paid_fetch again for the same URL; neither pays a second time."
            }),
            false,
        ),
        Err(e) => tool_error(error_kind(&e), e.message()),
    }
}

async fn paid_fetch(args: &Value) -> Value {
    let Some(url) = args.get("url").and_then(Value::as_str) else {
        return tool_error("invalid_arguments", "paid_fetch needs {\"url\": \"http(s)://...\"}");
    };
    let cfg = match AgentConfig::from_env() {
        Ok(c) => c,
        Err(e) => return setup_required(&e),
    };
    paid_result(pay::paid_fetch_nowait(&cfg, url, &decline).await)
}

async fn redeem_payment(args: &Value) -> Value {
    let field = |k: &str| args.get(k).and_then(Value::as_str);
    let (Some(url), Some(txid), Some(nonce)) = (field("url"), field("txid"), field("memoNonce")) else {
        return tool_error("invalid_arguments", "redeem_payment needs {\"url\", \"txid\", \"memoNonce\"}");
    };
    let cfg = match AgentConfig::from_env() {
        Ok(c) => c,
        Err(e) => return setup_required(&e),
    };
    if let Err(e) = cfg.key() {
        return setup_required(&e);
    }
    match pay::redeem_payment(&cfg, url, txid, nonce).await {
        Err(PayError::Config(m)) if m.contains("txid must be") => tool_error("invalid_arguments", &m),
        r => paid_result(r),
    }
}

async fn tab_fetch(args: &Value) -> Value {
    let Some(url) = args.get("url").and_then(Value::as_str) else {
        return tool_error("invalid_arguments", "tab_fetch needs {\"url\": \"http(s)://...\"}");
    };
    let size = match args.get("buy") {
        None | Some(Value::Null) => match std::env::var("X402Z_AGENT_TAB_SIZE").ok().filter(|v| !v.trim().is_empty()) {
            None => tab::DEFAULT_TAB_SIZE,
            Some(v) => match v.trim().parse::<u32>() {
                Ok(n) if n >= 1 => n,
                _ => return tool_error("setup_required", "X402Z_AGENT_TAB_SIZE must be a positive integer"),
            },
        },
        Some(v) => match v.as_u64() {
            Some(n) if (1..=100).contains(&n) => n as u32,
            _ => return tool_error("invalid_arguments", "buy must be an integer from 1 to 100"),
        },
    };
    let cfg = match AgentConfig::from_env() {
        Ok(c) => c,
        Err(e) => return setup_required(&e),
    };
    let mut bought = None;
    let fetched = match tab::fetch(&cfg, url).await {
        Ok(Some(f)) => f,
        Ok(None) => {
            eprintln!("x402z-mcp: no tab token for {url}; collecting a paid tab or buying one of {size}");
            let b = match tab::buy_or_resume(&cfg, url, size, &decline, false).await {
                Ok(tab::BuyOutcome::Bought(b)) => b,
                Ok(tab::BuyOutcome::Pending { purchase: p, message }) => {
                    let txid = p.txid.unwrap_or_default();
                    return text_result(
                        &json!({
                            "status": "tab_pending",
                            "tokens": p.n, "amountZat": p.amount_zat, "txid": txid, "memoNonce": p.memo_nonce, "keyId": p.offer.key_id,
                            "message": message,
                            "explorer": format!("https://testnet.zecblock.com/tx/{txid}"),
                            "next": "The tab is paid and the server issues its tokens once the payment has its block (about 75 s). Call tab_fetch again for this URL in about a minute; it collects the tokens and does not pay again."
                        }),
                        false,
                    );
                }
                Err(e) => return tool_error(error_kind(&e), &format!("no tab token left, and buying a tab of {size} failed: {}", e.message())),
            };
            bought = Some(json!({
                "tokens": b.n, "amountZat": b.amount_zat, "txid": b.txid, "memoNonce": b.memo_nonce, "keyId": b.key_id,
                "explorer": format!("https://testnet.zecblock.com/tx/{}", b.txid)
            }));
            match tab::fetch(&cfg, url).await {
                Ok(Some(f)) => f,
                Ok(None) => return tool_error("runtime", "the tab was bought but no token is stored for this URL"),
                Err(e) => return tool_error(error_kind(&e), e.message()),
            }
        }
        Err(e) => return tool_error(error_kind(&e), e.message()),
    };
    let body = serde_json::from_str::<Value>(&fetched.body).unwrap_or(Value::String(fetched.body.clone()));
    text_result(
        &json!({
            "status": fetched.status, "paidWith": "tab_token", "elapsedMs": fetched.elapsed_ms as u64,
            "remainingTokens": fetched.remaining, "bought": bought, "settlement": fetched.settlement, "body": body
        }),
        false,
    )
}

fn ledger_and_policy() -> Result<(LedgerFile, spendguard::Policy, String), PayError> {
    let path = std::env::var("X402Z_AGENT_LEDGER")
        .ok()
        .filter(|v| !v.trim().is_empty())
        .ok_or_else(|| PayError::Config("X402Z_AGENT_LEDGER not configured (see .env.example)".into()))?;
    let seed = std::env::var("X402Z_AGENT_SEED")
        .ok()
        .filter(|v| !v.trim().is_empty())
        .map(|p| x402z_agent::wallet::seed_from_phrase(&p).map_err(|e| PayError::Config(format!("X402Z_AGENT_SEED invalid: {e}"))))
        .transpose()?;
    let key = x402z_agent::ledger::LedgerKey::from_env(seed.as_ref())
        .map_err(PayError::Config)?
        .ok_or_else(|| PayError::Config(x402z_agent::ledger::MISSING_KEY.into()))?;
    let ledger = LedgerFile::load(std::path::Path::new(path.trim()), &key).map_err(PayError::Config)?;
    let policy = AgentConfig::policy_from_env()?;
    let agent = std::env::var("X402Z_AGENT_NAME").ok().filter(|v| !v.trim().is_empty()).unwrap_or_else(|| "agent-1".into());
    Ok((ledger, policy, agent))
}

fn spend_status() -> Value {
    let (ledger, policy, agent) = match ledger_and_policy() {
        Ok(x) => x,
        Err(e) => return setup_required(&e),
    };
    let now = pay::now();
    let spent = ledger.guard(&agent, &policy, now).spent_today(&agent, now);
    text_result(
        &json!({
            "agent": agent,
            "utcDay": day_of(now),
            "spentTodayZat": spent,
            "dailyCapZat": policy.daily_cap_zat,
            "remainingZat": policy.daily_cap_zat.saturating_sub(spent),
            "allowedHosts": policy.allowed_hosts,
            "approvalThresholdZat": policy.approval_threshold_zat,
            "network": "zcash-testnet"
        }),
        false,
    )
}

fn ledger_tail(args: &Value) -> Value {
    let n = match args.get("n") {
        None | Some(Value::Null) => 10,
        Some(v) => match v.as_u64() {
            Some(n) if (1..=100).contains(&n) => n as usize,
            _ => return tool_error("invalid_arguments", "n must be an integer from 1 to 100"),
        },
    };
    let (ledger, _, _) = match ledger_and_policy() {
        Ok(x) => x,
        Err(e) => return setup_required(&e),
    };
    let all = &ledger.data.audit;
    let tail = &all[all.len().saturating_sub(n)..];
    text_result(&json!({ "total": all.len(), "entries": tail }), false)
}

/// Handle one JSON-RPC message. `None` for notifications (no reply).
async fn handle(msg: Value) -> Option<Value> {
    let id = msg.get("id").cloned();
    let method = msg.get("method").and_then(Value::as_str).unwrap_or_default().to_string();
    let params = msg.get("params").cloned().unwrap_or(json!({}));
    let id = id?; // notifications (initialized, cancelled, ...) get no response
    let result = match method.as_str() {
        "initialize" => {
            let asked = params.get("protocolVersion").and_then(Value::as_str).unwrap_or(SUPPORTED_VERSIONS[0]);
            let version = if SUPPORTED_VERSIONS.contains(&asked) { asked } else { SUPPORTED_VERSIONS[0] };
            Ok(json!({
                "protocolVersion": version,
                "capabilities": { "tools": { "listChanged": false } },
                "serverInfo": { "name": "x402z-mcp", "title": "Kage shielded x402", "version": env!("CARGO_PKG_VERSION") },
                "instructions": "Pay for HTTP APIs with shielded Zcash testnet payments. Call spend_status before paying; paid_fetch refuses anything spendguard denies."
            }))
        }
        "ping" => Ok(json!({})),
        "tools/list" => Ok(json!({ "tools": tools() })),
        "tools/call" => {
            let name = params.get("name").and_then(Value::as_str).unwrap_or_default();
            let args = params.get("arguments").cloned().unwrap_or(json!({}));
            match name {
                "paid_fetch" => Ok(paid_fetch(&args).await),
                "redeem_payment" => Ok(redeem_payment(&args).await),
                "tab_fetch" => Ok(tab_fetch(&args).await),
                "spend_status" => Ok(spend_status()),
                "ledger_tail" => Ok(ledger_tail(&args)),
                other => Err((-32602, format!("unknown tool '{other}'"))),
            }
        }
        other => Err((-32601, format!("method not found: {other}"))),
    };
    Some(match result {
        Ok(r) => json!({ "jsonrpc": "2.0", "id": id, "result": r }),
        Err((code, message)) => json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } }),
    })
}

#[tokio::main]
async fn main() {
    let mut lines = BufReader::new(tokio::io::stdin()).lines();
    // One writer owns stdout; each request runs in its own task, so a slow tool call (wallet
    // sync, proving) doesn't hold up ping, spend_status or another call.
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<Value>();
    let writer = tokio::spawn(async move {
        let mut out = tokio::io::stdout();
        while let Some(r) = rx.recv().await {
            let mut s = serde_json::to_string(&r).unwrap_or_default();
            s.push('\n');
            if out.write_all(s.as_bytes()).await.is_err() || out.flush().await.is_err() {
                break;
            }
        }
    });
    eprintln!("x402z-mcp: ready on stdio (tools: paid_fetch, redeem_payment, tab_fetch, spend_status, ledger_tail)");
    let mut tasks = tokio::task::JoinSet::new();
    while let Ok(Some(line)) = lines.next_line().await {
        if line.trim().is_empty() {
            continue;
        }
        let tx = tx.clone();
        match serde_json::from_str::<Value>(&line) {
            Ok(msg) => {
                tasks.spawn(async move {
                    if let Some(r) = handle(msg).await {
                        let _ = tx.send(r);
                    }
                });
            }
            Err(e) => {
                let _ = tx.send(json!({ "jsonrpc": "2.0", "id": null, "error": { "code": -32700, "message": format!("parse error: {e}") } }));
            }
        }
    }
    // stdin closed: let calls in flight finish and their answers go out.
    while tasks.join_next().await.is_some() {}
    drop(tx);
    let _ = writer.await;
}
