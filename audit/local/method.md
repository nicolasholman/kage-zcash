Audit method and boundaries

Repository commit: `4e51b2b84c831bf297031b63416e9962c803c12c`. All product source and AUDIT.md remain unchanged. No commit, push, or deployment was performed; Loom owns committing this worktree.

The shared heavy slot was acquired with `/Volumes/Extreme SSD/Projects/grokbot/tools/heavy-slot.sh acquire kage-local-audit` before builds, stack, or Chrome. Only this project was built, sequentially. The stack was kept in a foreground PTY shell while it served requests. An initial short-lived execution shell did not preserve the stack's children; `stack.sh stop` cleared those stale PIDs, the stack was restarted under the held shell, and every retained successful test/browser result was produced against that running instance. The stack was stopped and the slot released before report generation. See cleanup logs for individual PIDs and verified absence.

Commands executed:

```text
cargo build -p x402z-server -p x402z-facilitator -p x402z-agent --bins --examples
e2e/stack.sh
(cd e2e && npm ci)
(cd e2e && npm run test:e2e)
node audit/local/probe.mjs initial
node audit/local/probe.mjs main
node audit/local/api-probe.mjs
node audit/local/gaps.mjs
cargo test -p x402z-facilitator --test live_testnet -- --test-threads=1
cargo test -p x402z-mcp -- --test-threads=1
cargo test -p x402z-agent -- --test-threads=1
cargo test -p spendguard -p x402z-protocol -p x402z-server -- --test-threads=1
(cd packages/x402-zcash && node --test test/unit.test.ts)
e2e/stack.sh stop
/Volumes/Extreme SSD/Projects/grokbot/tools/heavy-slot.sh release kage-local-audit
```

The initial-state probe ran before the suite populated the merchant feed. The main/API probes were rerun after correcting harness-only URL decoding, locator, and timer-wait issues. Final retained JSON/logs correspond to the final harness. The UI suites were run once; all requested package commands were run once. `run-tests.py` records each test subprocess PID and exit status. Supplemental scripts retain assertion statuses in JSON and logs; their process exit code indicates script completion, so read the per-row PASS/FAIL values.

Browser: installed Google Chrome channel `chrome`, version 155.0.8059.85, driven by Playwright 1.63.0. The existing suite runs desktop 1280×800 and mobile 375×812 (DPR 2). Supplemental scripts use the same Chrome installation, with one browser process per script and sequential contexts at widths 1280/375. The supplements collect every console error, page error, request failure, and HTTP status, including cross-origin resources. The two refused-network requests are intentional U-096 fault injection via Playwright request abort; the product's visible banner was correct. Expected HTTP statuses do not hide console errors in the supplements.

Literal mismatches remain recorded without changing product code or AUDIT.md. For U-011 the expected Spec label is compared exactly. For U-062 the four HTML `section` elements are compared with the five expected sections; `#keys` exists, but is a `div`. For U-067 rendered `innerText` is compared exactly; CSS visually separates `0` and `TAZ`, while the text is `0TAZ`. These are distinguished from payment-behavior failures in the report.

Chain safety: the reviewed `live_testnet.rs` uses `GetLightdInfo`, `GetLatestBlock`, `GetTransaction`, and `GetBlock` through the facilitator client. Its ten-entry payment loop verifies txid equality, recorded mined height, merchant UIVK decryption, memo nonce, amount, Ironwood pool, >=1 confirmation, and the two pinned block timestamps. The other tests reject wrong nonce/amount/key/recipient and check an unknown txid. No transaction construction or SendTransaction path is invoked by this chain command.

No protected wallet, wallet seed file, project .env, or signing-key file was opened by these audit scripts. `new-wallet` was never invoked. No manual facilitator keygen was invoked. The user's expressly authorized existing `stack.sh` generated a fresh receipt key and facilitator token under its isolated `e2e/.run`; its existing browser receipt fixture internally loads that throwaway signing key. Those files were not printed or copied to evidence. Existing agent/MCP tests use isolated fixture paths and refuse before funded-wallet operations; no user seed is required. Fixture random txids used for signed local receipt events are synthetic identifiers, not new on-chain transactions.

The TypeScript `test/live.test.ts` was not executed. U-083/U-084 remain untested because no fresh matching paid/unconfirmed transaction was created. Hosted endpoint checks and backend branches not established by this task's exact assertions remain explicitly UNTESTED. Aggregate suite success is not used as blanket coverage for all 584 AUDIT rows.

Evidence: screenshots are JPEG, quality 60–65; retained suite PNGs were converted to JPEG and capped at 1600 px maximum dimension. Original suite videos/traces/PNGs stay in ignored e2e output and are not delivery files. The evidence delivery contains no .env files, receipt signing keys, facilitator tokens, wallet database, or sealed agent ledger. `excerpts.md` links runtime assertions to audit rows; raw logs and scripts allow review/reproduction within the same safety boundary.
