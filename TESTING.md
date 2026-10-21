# Testing Kage

75 tests, run one crate at a time (the Zcash crates are large; this keeps memory low):

```sh
cargo test -p spendguard -p x402z-protocol -p x402z-server
cargo test -p x402z-agent
cargo test -p x402z-mcp
cargo test -p x402z-facilitator          # includes the live testnet tests (network needed)
```

CI (`.github/workflows/ci.yml`) runs the same tests in three jobs. The fast crates go in one job. The facilitator's in-process real-transaction tests, which do Halo2 proving, go in a second. The live testnet tests go in a third.

| Crate / file | Tests | Runs against | What it proves |
|---|---|---|---|
| `spendguard` | 11 | — | allowlist (exact, `*.suffix`, case, ports, trailing dots), zero amount, inclusive daily cap per UTC day, overflow is a denial, approval threshold, first denial wins |
| `x402z-protocol` `tests/challenge.rs` | 6 | — | challenge shape and validation (scheme, mainnet refused, address prefix, nonce charset/length, expiry), `validate_shape` ignores expiry, `paid_in_time` with grace and no overflow, `minedAt` optional for old receipts |
| `x402z-protocol` `tests/x402_wire.rs` | 6 | — | the x402 v1 spec's own 402 example (EVM `exact`) parses next to a `zcash-shielded` option, which is picked; requirements ↔ challenge; `X-PAYMENT` is base64 JSON and round-trips (standard and URL-safe); the spec's EVM payload parses but is refused as Zcash; version/network/missing-nonce errors; `SettlementResponse` and `/supported` field names; the Kage Tab `X-PAYMENT` payload round-trips and is scheme-checked both ways |
| `x402z-protocol` `tests/zip321.rs` | 2 | — | ZIP 321 amounts (decimal ZEC, ≤ 8 decimals, no trailing zeros) and memo encoding (base64url, no padding; ZIP 321's own example) |
| `x402z-server` (unit) | 26 | in-process | ed25519 receipt signature and tampering; settle once; replay; underpaid; wrong address; unknown nonce; **mined-time expiry** (paid before expiry and redeemed after is accepted, mined inside grace is accepted, mined after grace is refused); **SQLite durability** (restart between 402 and redeem, replay after restart, pruning keeps spent rows); per-IP token bucket and 429 + `Retry-After`; **x402**: the 402 body shape, `X-PAYMENT` settles once with `X-PAYMENT-RESPONSE`, pending keeps the same challenge and burns nothing, underpaid, receipt from an untrusted signer, unknown nonce, malformed header, wrong network, facilitator down → 502 with nothing burned; **ZIP 321**: `extra.paymentUri`, local QR SVG, `POST /api/redeem` pending → rejected → paid → replay; **Kage Tab** (real RFC 9474 blind RSA, RSA-2048): one payment buys n tokens, each spendable once; double spend; forged, moved, re-randomized and self-signed tokens; another key epoch and relabelled keyId; reissue on the same nonce and redeeming it as a single payment; underpaid n burns nothing; more than maxTokens; key and spent set survive a restart |
| `x402z-agent` (unit) | 8 | — | ledger cap survives restarts (only broadcast payments count), replay can't bypass the allowlist, an invalid ledger is an error, not a reset, dashboard shape, **six concurrent runs against room for one payment: exactly one pays** (fails without the file lock), picking and validating the x402 option, host:port for spendguard, stable exit codes |
| `x402z-mcp` `tests/stdio.rs` | 3 | the real binary over stdio | MCP handshake (`initialize`, `notifications/initialized`, `ping`, version negotiation), `tools/list`, unknown method/tool errors; missing config → `setup_required` tool error; spendguard refusals (cap, allowlist, approval threshold) are `isError` results, nothing paid, and `ledger_tail` shows the three decisions |
| `x402z-facilitator` (unit + bin) | 3 | — | nonce → txid binding survives restart and refuses a second txid; x402 request parsing (payload nonce must match requirements, version, network, legacy body detection); `/supported` |
| `x402z-facilitator` `tests/verify_real_tx.rs` | 6 | in-process transactions | **In-process transactions with real keys, real note encryption and real Halo2 proofs, not broadcast.** They are built with the real `zcash_primitives` builder, serialized and re-parsed like lightwalletd bytes. Sapling outputs use sapling-crypto's mock *proof* bytes (encryption and decryption are real). Each verification runs through the merchant **UIVK** and the UFVK-reduced key, and they must agree. Covers: Ironwood and Sapling payments, split across pools, confirmations; underpaid, other nonce, other merchant, sibling diversified address (`WrongRecipient`), bad `payTo`, garbage bytes; transparent receivers/outputs never credited; **the merchant's own change and outgoing outputs (nonce in every memo) are visible to the full key and invisible to the UIVK**; receipt wire format incl. `minedAt` |
| `x402z-facilitator` `tests/zip321_uri.rs` | 1 | — | librustzcash's ZIP 321 parser reads Kage's URI back to the same address, amount and memo, and re-renders it byte for byte |
| `x402z-facilitator` `tests/live_testnet.rs` | 3 | **Zcash testnet** via testnet.zec.rocks | the six real Kage payments (030c3cbf, cd64980a, ccb4214c, 25a8b324, 408aba26, a7c3b0b2) verify with only the published demo merchant UIVK at their recorded heights, with pinned block times for two; the same real tx is refused for another nonce, 1 zat more, a stranger's key, a foreign `payTo`; unknown txid → not found |
| **Total** | **75** | | |

## What is not a chain test, and why

- **Server `X-PAYMENT` / `/api/redeem` tests** run against a small in-process HTTP stand-in for *our own* facilitator's `/settle`. The stand-in answers in the facilitator's wire format and signs receipts with the test key. It exists to test the server's handling of each facilitator answer. The facilitator's actual chain checks are covered by `verify_real_tx` (in-process transactions) and `live_testnet` (real chain). The same server path ran live on testnet: see the next section.
- **The MCP tests** use a localhost HTTP fixture that returns a fixed x402 402. Every `paid_fetch` there is refused by spendguard before any wallet or chain work, so nothing touches a chain.

## Live runs (not part of `cargo test`)

| Date | What | Result |
|---|---|---|
| 2026-10-20 | Agent paid with its own wallet (earlier receipt header) | `030c3cbf…8351` block 4521112, `cd64980a…916d` block 4521440 (demo video) |
| 2026-10-21 | Facilitator holding only the published UIVK, x402 v1 server, SQLite state. Dashboard 402 card → ZIP 321 URI paid by zingo-cli from a separate demo wallet → txid pasted → `POST /api/redeem` | `ccb4214c…0ed3` block 4524586: `paid`. Second redeem: `nonce_already_used` |
| 2026-10-21 | Same stack, `X-PAYMENT` retried every 20 s | `25a8b324…689d` block 4524602: `402 settlement_pending` ×n, then `200` + `X-PAYMENT-RESPONSE {"success":true,…}` |
| 2026-10-21 | Same stack, both paths repeated from the same zingo-cli demo wallet | `408aba26…8dd0` block 4524693 (`X-PAYMENT`), `a7c3b0b2…38bc` block 4524694 (ZIP 321 + paste-txid) |
| 2026-10-21 | Same stack, agent refusals with the x402 v1 body | cap → exit 3, allowlist → exit 3, no seed → exit 4 |
| 2026-10-21 | Facilitator `/supported`, x402 `/verify` and `/settle` on cd64980a | `isValid: true`; settle `success: true`, `minedAt 1792482929`; asking 100,001 zat → `invalid_payment` |
| 2026-10-21 | LLM agent (opencode, `openrouter/minimax/minimax-m3.1-flash:free`) over MCP | `spend_status` → `paid_fetch` refused by spendguard (cap 50,000 < price 100,000) → `ledger_tail` shows `denied`. [Transcript](docs/mcp-live-run.txt) |

A live payment from the *agent's own wallet* on the current code is pending. The agent wallet's keys are held outside this repo, and this round did not touch them. The agent's paying path (`x402z_agent::pay::paid_fetch`) is the one exercised by the Oct 20 runs, now ending in `X-PAYMENT` instead of the receipt header. That final `X-PAYMENT` step is the one the `25a8b324` run proved live.
