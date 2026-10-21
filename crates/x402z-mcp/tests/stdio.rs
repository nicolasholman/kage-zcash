//! Drive the real `x402z-mcp` binary over stdio, as an MCP client does.
//!
//! The paid resource is a tiny HTTP server on 127.0.0.1 that answers every request with a fixed
//! x402 402 (the shape `x402z-server` sends, with a Kage Tab offer). The tests only go as far as
//! spendguard and the wallet check: every paid_fetch and tab_fetch here is refused before any
//! wallet or chain work, so nothing is paid and no network beyond localhost is touched.

use std::{
    io::{BufRead, BufReader, Read, Write},
    net::TcpListener,
    path::PathBuf,
    process::{Child, ChildStdin, ChildStdout, Command, Stdio},
};

use serde_json::{json, Value};

const PAY_TO: &str = "utest1sampleaddressforschemaonly000000";

/// Serve one fixed x402 402 for every request; returns the base URL.
fn fixture_402() -> String {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = l.local_addr().unwrap();
    std::thread::spawn(move || {
        for stream in l.incoming() {
            let Ok(mut s) = stream else { continue };
            let mut buf = [0u8; 4096];
            let _ = s.read(&mut buf);
            let body = json!({
                "x402Version": 1,
                "error": "X-PAYMENT header is required",
                "accepts": [{
                    "scheme": "zcash-shielded", "network": "zcash-testnet", "maxAmountRequired": "100000",
                    "resource": format!("http://{addr}/api/quote"), "description": "fixture", "mimeType": "application/json",
                    "payTo": PAY_TO, "maxTimeoutSeconds": 900, "asset": "ZEC",
                    "extra": { "memoNonce": "x402z-fixture", "expiresAt": 4_000_000_000u64,
                               "tab": { "scheme": "zcash-shielded-tab", "suite": "RSABSSA-SHA384-PSS-Randomized", "keyId": "fixture",
                                        "publicKey": "AA==", "unitPriceZat": 100000, "maxTokens": 20, "issuePath": "/tab/issue",
                                        "epochEndsAt": 4_000_000_000u64 } }
                }]
            })
            .to_string();
            let _ = write!(s, "HTTP/1.1 402 Payment Required\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}", body.len());
        }
    });
    format!("http://{addr}")
}

struct Mcp {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    next_id: u64,
}

impl Mcp {
    fn start(env: &[(&str, String)]) -> Self {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_x402z-mcp"));
        cmd.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null());
        for v in ["X402Z_AGENT_LEDGER", "X402Z_AGENT_WALLET_DIR", "X402Z_AGENT_SEED", "X402Z_AGENT_TAB_FILE", "X402Z_AGENT_TAB_SIZE", "X402Z_AGENT_ALLOWED_HOSTS", "X402Z_AGENT_DAILY_CAP_ZAT", "X402Z_AGENT_APPROVAL_ZAT", "X402Z_AGENT_NAME"] {
            cmd.env_remove(v);
        }
        for (k, v) in env {
            cmd.env(k, v);
        }
        let mut child = cmd.spawn().unwrap();
        let stdin = child.stdin.take().unwrap();
        let stdout = BufReader::new(child.stdout.take().unwrap());
        Self { child, stdin, stdout, next_id: 1 }
    }

    fn send(&mut self, v: &Value) {
        writeln!(self.stdin, "{v}").unwrap();
        self.stdin.flush().unwrap();
    }

    fn request(&mut self, method: &str, params: Value) -> Value {
        let id = self.next_id;
        self.next_id += 1;
        self.send(&json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }));
        let mut line = String::new();
        self.stdout.read_line(&mut line).unwrap();
        let v: Value = serde_json::from_str(&line).unwrap_or_else(|e| panic!("not JSON-RPC on stdout: {line:?}: {e}"));
        assert_eq!(v["jsonrpc"], "2.0");
        assert_eq!(v["id"], id);
        v
    }

    fn call(&mut self, tool: &str, args: Value) -> Value {
        let v = self.request("tools/call", json!({ "name": tool, "arguments": args }));
        v["result"].clone()
    }
}

impl Drop for Mcp {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn tmp_ledger(name: &str) -> PathBuf {
    let p = std::env::temp_dir().join(format!("x402z-mcp-{}-{name}.json", std::process::id()));
    let _ = std::fs::remove_file(&p);
    p
}

fn structured(r: &Value) -> Value {
    // text content and structuredContent carry the same JSON
    let text: Value = serde_json::from_str(r["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(text, r["structuredContent"]);
    text
}

#[test]
fn handshake_and_tool_list() {
    let mut m = Mcp::start(&[]);
    let init = m.request("initialize", json!({ "protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": { "name": "test", "version": "0" } }));
    assert_eq!(init["result"]["protocolVersion"], "2025-06-18");
    assert_eq!(init["result"]["serverInfo"]["name"], "x402z-mcp");
    assert!(init["result"]["capabilities"]["tools"].is_object());
    m.send(&json!({ "jsonrpc": "2.0", "method": "notifications/initialized" })); // no reply expected
    assert_eq!(m.request("ping", json!({}))["result"], json!({}));

    let list = m.request("tools/list", json!({}));
    let names: Vec<&str> = list["result"]["tools"].as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap()).collect();
    assert_eq!(names, ["paid_fetch", "tab_fetch", "spend_status", "ledger_tail"]);
    for t in list["result"]["tools"].as_array().unwrap() {
        assert_eq!(t["inputSchema"]["type"], "object");
    }
    // an older client version is answered with that version; an unknown one with ours
    let mut m2 = Mcp::start(&[]);
    assert_eq!(m2.request("initialize", json!({ "protocolVersion": "2024-11-05" }))["result"]["protocolVersion"], "2024-11-05");
    assert_eq!(m.request("nope", json!({}))["error"]["code"], -32601);
    assert_eq!(m.request("tools/call", json!({ "name": "nope" }))["error"]["code"], -32602);
}

#[test]
fn missing_config_is_setup_required_not_a_crash() {
    let mut m = Mcp::start(&[]);
    let r = m.call("spend_status", json!({}));
    assert_eq!(r["isError"], true);
    let v = structured(&r);
    assert_eq!(v["error"], "setup_required");
    assert!(v["message"].as_str().unwrap().contains("X402Z_AGENT_LEDGER"));
    let r = m.call("paid_fetch", json!({ "url": "http://127.0.0.1:9/x" }));
    assert_eq!(r["isError"], true);
    assert_eq!(structured(&r)["error"], "setup_required");
    let r = m.call("paid_fetch", json!({}));
    assert_eq!(structured(&r)["error"], "invalid_arguments");
    let r = m.call("tab_fetch", json!({ "url": "http://127.0.0.1:9/x" }));
    assert_eq!(structured(&r)["error"], "setup_required");
    assert_eq!(structured(&m.call("tab_fetch", json!({ "url": "http://127.0.0.1:9/x", "buy": 0 })))["error"], "invalid_arguments");
}

#[test]
fn tab_fetch_without_tokens_buys_a_tab_inside_spendguard() {
    let url = format!("{}/api/quote", fixture_402());
    let ledger = tmp_ledger("tab");
    let wallet_dir = std::env::temp_dir().join(format!("x402z-mcp-{}-tab-wallet", std::process::id()));
    let base = vec![
        ("X402Z_AGENT_LEDGER", ledger.display().to_string()),
        ("X402Z_AGENT_WALLET_DIR", wallet_dir.display().to_string()),
        ("X402Z_AGENT_ALLOWED_HOSTS", "127.0.0.1".to_string()),
    ];

    // a tab of 5 costs 500,000 zat: over a 300,000 zat cap, so spendguard refuses the whole tab
    let mut env = base.clone();
    env.push(("X402Z_AGENT_DAILY_CAP_ZAT", "300000".into()));
    let mut m = Mcp::start(&env);
    let v = structured(&m.call("tab_fetch", json!({ "url": url })));
    assert_eq!(v["error"], "spendguard_refused", "{v}");
    assert!(v["message"].as_str().unwrap().contains("buying a tab of 5 failed"), "{v}");
    assert!(v["message"].as_str().unwrap().contains("daily cap exceeded"), "{v}");
    drop(m);

    // a tab of 2 fits the cap, but there is no funded wallet: a wallet error, nothing paid
    let mut m = Mcp::start(&env);
    let r = m.call("tab_fetch", json!({ "url": url, "buy": 2 }));
    assert_eq!(r["isError"], true);
    let v = structured(&r);
    assert_eq!(v["error"], "wallet", "{v}");
    assert!(v["message"].as_str().unwrap().contains("X402Z_AGENT_SEED not configured"), "{v}");

    let t = structured(&m.call("ledger_tail", json!({ "n": 5 })));
    let entries = t["entries"].as_array().unwrap();
    let got: Vec<(&str, u64)> = entries.iter().map(|e| (e["decision"].as_str().unwrap(), e["amountZat"].as_u64().unwrap())).collect();
    assert_eq!(got, [("denied", 500_000), ("allowed", 200_000)]);
    assert!(entries.iter().all(|e| e.get("txid").is_none()));
    assert!(entries[1]["reason"].as_str().unwrap().contains("not paid"));
    for p in [ledger.clone(), ledger.with_extension("json.lock"), ledger.with_extension("tab.json"), ledger.with_extension("tab.json.lock")] {
        let _ = std::fs::remove_file(p);
    }
}

#[test]
fn spendguard_refusals_are_tool_errors_and_land_in_the_ledger() {
    let url = format!("{}/api/quote", fixture_402());
    let ledger = tmp_ledger("refusals");
    let wallet_dir = std::env::temp_dir().join(format!("x402z-mcp-{}-wallet", std::process::id()));
    let base = vec![
        ("X402Z_AGENT_LEDGER", ledger.display().to_string()),
        ("X402Z_AGENT_WALLET_DIR", wallet_dir.display().to_string()),
        ("X402Z_AGENT_ALLOWED_HOSTS", "127.0.0.1".to_string()),
    ];

    // 1. daily cap below the price
    let mut env = base.clone();
    env.push(("X402Z_AGENT_DAILY_CAP_ZAT", "50000".into()));
    let mut m = Mcp::start(&env);
    let r = m.call("paid_fetch", json!({ "url": url }));
    assert_eq!(r["isError"], true, "{r}");
    let v = structured(&r);
    assert_eq!(v["error"], "spendguard_refused");
    assert!(v["message"].as_str().unwrap().contains("daily cap exceeded"), "{v}");
    assert!(v["message"].as_str().unwrap().contains("No transaction was built or broadcast"));

    let s = structured(&m.call("spend_status", json!({})));
    assert_eq!((s["spentTodayZat"].as_u64(), s["dailyCapZat"].as_u64(), s["remainingZat"].as_u64()), (Some(0), Some(50_000), Some(50_000)));
    assert_eq!(s["allowedHosts"], json!(["127.0.0.1"]));
    drop(m);

    // 2. host not on the allowlist
    let mut env = base.clone();
    env[2].1 = "api.example.com".into();
    let mut m = Mcp::start(&env);
    let v = structured(&m.call("paid_fetch", json!({ "url": url })));
    assert!(v["message"].as_str().unwrap().contains("not on the allowlist"), "{v}");
    drop(m);

    // 3. at the approval threshold: MCP can't approve, so it is declined
    let mut env = base.clone();
    env.push(("X402Z_AGENT_APPROVAL_ZAT", "100000".into()));
    let mut m = Mcp::start(&env);
    let v = structured(&m.call("paid_fetch", json!({ "url": url })));
    assert_eq!(v["error"], "spendguard_refused");
    assert!(v["message"].as_str().unwrap().contains("approval required"), "{v}");

    // all three decisions are in the ledger, none with a txid
    let t = structured(&m.call("ledger_tail", json!({ "n": 5 })));
    assert_eq!(t["total"], 3);
    let decisions: Vec<&str> = t["entries"].as_array().unwrap().iter().map(|e| e["decision"].as_str().unwrap()).collect();
    assert_eq!(decisions, ["denied", "denied", "needs_approval"]);
    assert!(t["entries"].as_array().unwrap().iter().all(|e| e.get("txid").is_none() && e["memoNonce"] == "x402z-fixture"));
    let t = structured(&m.call("ledger_tail", json!({ "n": 1 })));
    assert_eq!(t["entries"].as_array().unwrap().len(), 1);
    assert_eq!(structured(&m.call("ledger_tail", json!({ "n": 0 })))["error"], "invalid_arguments");
    let _ = std::fs::remove_file(&ledger);
    let _ = std::fs::remove_file(ledger.with_extension("json.lock"));
}
