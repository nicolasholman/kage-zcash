The mapped execution finishes at **573 PASS / 6 FAIL / 5 UNTESTED**, total **584**. All 222 original PASS rows, all six original FAIL rows, and every expected-result cell are preserved verbatim. The six Chrome failures are U-071, U-081, U-093, U-095, U-096 and U-101. Required status codes and Chrome diagnostics were retained; the dashboard was not changed.

The map contained 356 UNTESTED rows. Execution promoted 351 using complete assertions and passing named executions. Existing suite totals alone were never used to promote a row. The final commands passed **177 Rust tests and 45 TypeScript tests**. The final machine-readable [counts](counts.json) and [commands, PIDs and exit codes](verification-summary.json) distinguish audit rows from test counts.

| Surface | Final evidence | Probe source |
| --- | --- | --- |
| Spendguard and key-shape validation | [spendguard.log](spendguard.log) | [spendguard.rs](spendguard.rs) |
| Protocol shape, nonce and payload validation | [protocol.log](protocol.log) | [protocol.rs](protocol.rs) |
| Server handlers, SQLite, receipts, limits and tabs | [server.log](server.log) | [server.rs](server.rs) |
| Agent CLI, transport, ledger and blind-RSA client | [agent.log](agent.log) | [agent.rs](agent.rs), [agent-http.rs](agent-http.rs), [agent-tab.rs](agent-tab.rs), [concurrent fixture](agent-concurrent.rs) |
| Existing MCP regression suite | [mcp.log](mcp.log) | [stdio.rs](../../crates/x402z-mcp/tests/stdio.rs) |
| MCP stdio and real local free/replay routes | [mcp-probes.log](mcp-probes.log) | [mcp.py](mcp.py) |
| Facilitator auth, chain-response conditions, proofs, persistence and health | [facilitator.log](facilitator.log) | [facilitator.rs](facilitator.rs), [fac-grpc.rs](fac-grpc.rs), [transaction builder](fac-transaction.rs) |
| Real server startup and direct-receipt behavior | [server-live.log](server-live.log), [cli.log](cli.log) | [server-live.mjs](server-live.mjs), [cli.py](cli.py) |
| Rows explicitly designated static | [static.log](static.log) | [static.py](static.py) |
| TypeScript parser, middleware, transport and receipts | [audit probes](typescript-run.log), [existing unit suite](typescript-unit-run.log) | [typescript.test.ts](typescript.test.ts) |
| B-426, real recorded-transaction interoperability | [two passing live tests](typescript-live-run.log), [child process exits](live-pids.jsonl) | [live.test.ts](../../packages/x402-zcash/test/live.test.ts), [private-output/PID wrapper](track-children.mjs) |
| B-470, three live admin paths | [single GET per URL](live-get.log) | Recorded status, curl exit and one-line body note; no live POST |
| Heavy slot and cleanup | [slot.log](slot.log), [verification-summary.json](verification-summary.json) | [verify.py](verify.py) |

The server probe includes an actual 90-second reqwest timeout. The agent probe retries the same payment at real 15-second intervals through pending, 429, 502, 503 and a transport failure. The original read-only `audit/reverify/testnet.log` (3 passed) is reused; `live_testnet` was not rerun. The live TypeScript command ran only `packages/x402-zcash/test/live.test.ts`, looking up the already recorded ccb4214c and 25a8b324 payments. No transaction was built for broadcast or sent.

| Root fix | Row | Separate commit | Verification |
| --- | --- | --- | --- |
| Classify an empty HTTP(S) authority as an invalid origin | B-034 | `4be20bf` | CLI cases and `empty_public_origin_reports_the_origin_requirement` |
| Report expired direct receipts through the existing settlement-response header | B-076 | `6a701d8` | `expired_direct_receipt_reports_payment_expired_without_burning_nonce` and the audit boundary probe |
| Require the encrypted-ledger key before MCP redemption | B-358 | `3b2ec47` | All five tools without a ledger key over stdio; existing MCP regression expanded to all five |
| Validate the tab issue origin when accepting an offer | B-324 | `b652695` | External tab-buy preflight, zero foreign-origin requests and payment entries; shared offer validation also serves wallet purchases |

These changes preserve routes, wire fields, header names, scheme strings, MCP tool/argument names, CLI names and exit codes. The standard `X-PAYMENT-RESPONSE` on an expired direct receipt was already required by the unchanged expected cell.

| UNTESTED row | Exact dependency and reason |
| --- | --- |
| B-001 | Needs permission to bind default port 4020, which the task excludes. Reading the default string does not prove the listener and HTTP behavior. |
| B-002 | Needs permission to bind default admin port 4029, which the task excludes. Allowed-host handler checks were executed in-process without binding those ports. |
| B-300 | Needs a funded Zcash testnet send. The map proposes two real `pay` processes and requires one to pay successfully. A recorded synthetic ledger outcome cannot establish that full flow under the no-send constraint. |
| U-083 | needs a funded Zcash testnet send |
| U-084 | needs a funded Zcash testnet send |

Expected-result cells take precedence where a map description was inaccurate: MCP uses `X402Z_AGENT_TAB_SIZE`; tab replay refusal exits 5; connection failure restores the token; a changed issue path after rotation is refused. Those expected behaviors were asserted without editing the cells. The noncanonical `127.1` admin bind exercises the non-loopback-spelling warning while its actual listener stays on 127.0.0.1; foreign Host enforcement was checked with curl.

Regression tests contain labelled HTTP/lightwalletd response fixtures, synthetic post-broadcast ledger states and the existing Sapling mock proof bytes. The HTTP/lightwalletd fixtures exercise actual parsing, decryption, cryptographic signatures, blind RSA, policy and SQLite code. The in-process Ironwood transactions contain genuine encryption and Halo2 proofs and a synthetic funding outpoint; their bytes are never broadcast. Recorded-chain interoperability is established separately by the live tests and retained recorded-tx logs. No fallback or stub was added to the product runtime. Browser paid/pending states were not fabricated.

Long-lived application processes were terminated by their recorded PID and reaped; synchronous children were waited/reaped. The heavy slot was released in `finally`. Private keygen stdout for the final live run was routed through an untracked 0600 file under `/tmp`, read by the test process and deleted. No existing wallet or project `.env` was opened, and no private keys, facilitator tokens or seeds are copied into audit evidence. No new screenshots were needed.

To reproduce, use a suitable build cache via `CARGO_TARGET_DIR` if desired, then run `python3 audit/exec/verify.py`. That runner acquires/releases the heavy slot and executes crates serially with one compiler job. [run-rust.py](run-rust.py) temporarily includes the committed probe sources and restores production sources, the facilitator manifest and lockfile in `finally`; it adds only already locked test-codec/client dependencies for the audit harness. `python3 audit/exec/update-report.py` verifies complete passing logs and preserves the original table invariants before updating the report.

Origin was not pushed. No deployment was performed.
