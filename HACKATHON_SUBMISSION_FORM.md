# Kage (zec-shielded-x402): Zecathon submission draft

**Event / track:** Zecathon, Shielded Payments. This is the team's one entry. Submission deadline: Oct 28 2026, 23:59 UTC.
**Name:** Kage: shielded x402, private machine payments for AI agents
**One line:** AI agents pay for HTTP APIs with Zcash shielded payments in the x402 v1 wire format. The merchant verifies with an incoming viewing key alone, and a spend guard caps every agent before any transaction exists.
**Repo:** https://github.com/nickthelegend/zec-shielded-x402 (MIT)
**Commit to submit:** the tip of `main` after Nick's push. Before submitting, `git log origin/main -1` must equal the local tip (`git log main -1`). Do not submit `1e232bb` (what GitHub serves until the push) or any earlier hash named in older notes.

## Form fields still open (Nick decides)
- **Demo video, about 2 minutes: NICK RECORDS IT on the submitted commit. Do NOT submit `x402-demo-v2.mp4`:** it is 44 s, recorded Oct 20 before x402 v1, the UIVK-only facilitator, ZIP 321, MCP and Kage Tab, and shows the old `X-PAYMENT-RECEIPT` header. Suggested order, with captions and no text-edited output:
  1. dashboard (admin port): Send request → 402 card with QR and ZIP 321 URI;
  2. pay it from a small zingo-cli testnet wallet, paste the txid → `settlement_pending` → `paid`;
  3. `X-PAYMENT` → `200` + `X-PAYMENT-RESPONSE` with `payer: ""`;
  4. `tab-buy 5 --external` → 5 instant `tab-fetch` calls → a replayed token refused;
  5. an MCP spendguard refusal, then the LLM-paid run (docs/mcp-paid-run.txt);
  6. `check_uivk` with the published UIVK on the new txid.
- **Working demo URL:** https://nickthelegend.github.io/zec-shielded-x402/ once Pages is enabled and `main` is pushed (`gh api -X POST repos/nickthelegend/zec-shielded-x402/pages -f build_type=workflow`, then the push; `.github/workflows/pages.yml` publishes `landing/`). It is **docs and recorded output, not a hosted paywall**; the page says so. A judge can run the whole stack locally with four commands (a cargo build, then `cd e2e && npm ci && npm run stack`, and `npm run test:e2e` in Google Chrome). Hosted now: the same page at https://kage-landing.vercel.app, and a live testnet 402 at https://kage-api.201-18-198-162.sslip.io/api/quote (facilitator health: https://kage-facilitator.201-18-198-162.sslip.io/health), on a small VPS behind TLS.

## Description (paste-ready, 1,810 of 2,000 characters)

```
Kage lets AI agents pay for HTTP APIs with Zcash shielded payments, in the x402 v1 wire format. A server answers 402 with a zcash-shielded offer: a shielded address, an amount, a memo nonce and a ZIP 321 URI. The agent's spendguard checks a daily cap, a host allowlist and an approval threshold before any transaction exists. The agent pays, then retries with X-PAYMENT. The merchant's facilitator verifies the payment with only an incoming viewing key, never a spend key, and returns payer: "".

Any wallet can pay the QR and paste the txid. An MCP server gives LLM agents paid_fetch, redeem_payment and tab_fetch. Kage Tab sells N RFC 9474 blind-RSA tokens for one shielded payment, each paying one later request the server can't link to the payment. A TypeScript client and Express middleware speak the same protocol.

What is real: 10 testnet payments, each checkable with the published demo merchant UIVK and re-verified by a live test. Two were paid from the agent's own wallet by an LLM over MCP (a paid_fetch and a 3-token tab). 139 tests (128 Rust, 11 TypeScript); 6 build transactions in-process with real Halo2 proofs; 5 run against live testnet. A Playwright suite drives the dashboard in Chrome.

Not real or not done: testnet only, pre-release librustzcash crates. Sapling outputs in the in-process tests use mock proof bytes. Server tests use an in-process stand-in for our own facilitator. Every recorded live run was on one machine; the hosted 402 has not been paid yet. Not audited.

Leaks: the client IP reaches the merchant; lightwalletd sees txid lookups; X-PAYMENT is a bearer credential (served again for 10 min after it settles); Kage Tab unlinkability assumes one key per epoch for every buyer; timing can link a 402 to its block. The agent ledger and tab tokens are encrypted at rest.
```

## What it does
- **x402 v1 over Zcash.** The 402 body is `{x402Version: 1, accepts: [{scheme: "zcash-shielded", network: "zcash-testnet", maxAmountRequired, payTo, asset: "ZEC", extra: {memoNonce, expiresAt, paymentUri}}], error}`. The client retries with `X-PAYMENT` (base64 `{x402Version, scheme, network, payload: {txid, memoNonce}}`) and gets `X-PAYMENT-RESPONSE`. The facilitator exposes `/verify`, `/settle` (bearer token) and `/supported`. All of this follows the coinbase/x402 v1 spec.
- **Least-privilege merchant.** The facilitator holds only the merchant's unified *incoming* viewing key. A test shows it can't even see the merchant's own change or outgoing payments. It trial-decrypts Sapling, Orchard and Ironwood outputs and checks recipient, amount, memo nonce and the mined block's time. Bindings are keyed by (nonce, payTo, amount), so a dust payment can't lock out the real one.
- **spendguard.** Daily cap, host allowlist (port- and scheme-aware) and approval threshold are checked before the wallet is touched, under a file lock; the allowlist and an https rule run before the agent sends its first request. The agent follows no redirects, charges a broadcast with an unknown outcome to the cap, writes the txid to its ledger before the broadcast, and presents a broadcast but unsettled payment again instead of paying twice, also when two MCP calls race. A lost 200 is answered again by the server for 10 minutes and never re-paid.
- **Any wallet can pay.** Each 402 carries a ZIP 321 URI and a locally rendered QR code. The dashboard redeems a pasted txid.
- **No money lost on restarts or slow blocks.** Challenges, spent nonces and bindings live in SQLite. Expiry is judged by the block's `minedAt` plus a grace period.
- **Private by default on both sides.** The dashboard, feed and status sit on a loopback admin port that answers only to its own Host (DNS rebinding); the public port serves only the paid API, and every payment attempt shares a per-client budget. The agent ledger and Kage Tab tokens are sealed with XChaCha20-Poly1305.
- **MCP server** (`x402z-mcp`) with `paid_fetch`, `redeem_payment`, `tab_fetch`, `spend_status` and `ledger_tail`. `paid_fetch` returns `settlement_pending` right after the broadcast, so an MCP client's ~60 s timeout can't trigger a second payment.
- **Kage Tab.** One shielded payment buys N unlinkable RFC 9474 blind-RSA tokens, each spendable once with `X-PAYMENT` scheme `zcash-shielded-tab`. The agent pins the merchant's key per epoch; a key rotation keeps a grace period and never strands a paid tab.
- **TypeScript** (`packages/x402-zcash`): client helpers and an Express/Node paywall middleware (per-client rate limit, facilitator timeout, pruned and capped in-memory store, resend window), tested against the real binaries and testnet.

**Prior art.** coinbase/x402 v1 defines the wire format for EVM payments; Kage adds a `zcash-shielded` scheme on it. A Zcash Community Grants application, "Shielded x402" (forum topic 57826, posted 2026-09-25 by the forum user baby-zack-agent), proposed private machine payments over x402 and was rejected on 2026-09-28. That application is another applicant's, not this team's: Kage is a separate build by Nivesh Gajengi and shares none of its code. Kage adds the UIVK-only facilitator, spendguard before the wallet, Kage Tab blind-RSA batching, x402 v1 wire compliance and live, checkable transactions.

## Proof it works (all on Zcash testnet)
| Txid | Block | Path |
|---|---|---|
| `030c3cbf78ce8460553bba4e82c0ba813f039bd3ecf4d7f2e81c59837dc28351` | 4521112 | agent's own wallet (Oct 20, pre-x402-v1 code) |
| `cd64980a5247881a727aa916476a3ba757e59794263c2ff1bcde467654a0916d` | 4521440 | agent's own wallet (Oct 20, pre-x402-v1 code) |
| `ccb4214c6a1fdf183c086ae5b4f559d181ead4c1c61dc7e711fc4a08b95b0ed3` | 4524586 | third-party wallet (zingo-cli) paying the dashboard's ZIP 321 request, txid pasted, `/api/redeem` → paid (Oct 21) |
| `25a8b324bebee30220382113c6fb9cc3be1144b036117c34778bde3b1d8b689d` | 4524602 | `X-PAYMENT` → `settlement_pending` → `200` + `X-PAYMENT-RESPONSE success` (Oct 21) |
| `408aba269ac7ca946e3b0eaedcf4cadd582a01090679e98391b4e7c706d88dd0` | 4524693 | `X-PAYMENT` path, repeated (Oct 21) |
| `a7c3b0b2eaeb406ddcd02276cd71e69e4e618b22a1cab8977c959f2da3fc38bc` | 4524694 | ZIP 321 + paste-txid path, repeated (Oct 21) |
| `fa01172015c63ce47a0285f7fae130a3efe5f821b938a8befa1ca547d00ac3ad` | 4526095 | Kage Tab: 500,000 zat → 5 tokens → 5 CLI requests (Oct 21) |
| `d3bf0b49408fed118fe9e1241239d98319cbb83088c45a5ba357f68cd97bd00d` | 4526113 | Kage Tab: 300,000 zat → 3 tokens → 3 MCP `tab_fetch` calls from a Python stdio script (Oct 21) |
| `f037d5978fec7817340ff118d328995288f0a575a1850b083ca05f7d71d4e22a` | 4529425 | **LLM over MCP, agent's own wallet:** `paid_fetch` → `settlement_pending` → `redeem_payment` → `200` (Oct 22) |
| `b75b2415347a7ce317a9b8b5799d07a2ad9ebc44eea92090f0db9d9b590e4b91` | 4529427 | **Kage Tab, LLM over MCP, agent's own wallet:** 300,000 zat → 3 tokens → 3 requests; four retries collected the same tab, none paid again (Oct 22) |

Anyone can check all ten with the published demo merchant UIVK. See the README, "Verified testnet runs", the `live_testnet` tests and `docs/mcp-paid-run.txt` (the LLM run, rendered from the raw opencode events in `docs/mcp-paid-run.jsonl`).

139 tests, listed in TESTING.md:
- 128 Rust: 6 build in-process transactions with real keys, real note encryption and real Halo2 proofs for the Orchard/Ironwood parts (Sapling outputs use sapling-crypto's mock proof bytes; not broadcast); 3 verify the ten real payments live against testnet.zec.rocks; 7 drive the MCP binary over stdio; 6 attack the Kage Tab on the server; 11 run the agent's Tab client, key rules, redirect refusal, racing calls, lost answers and the pre-request allowlist against localhost servers.
- 11 TypeScript: 2 run the client and middleware against the real binaries and testnet; 9 need no chain.
- Separately, 32 Playwright specs drive the dashboard and the landing page in Google Chrome at 1280 px and 375 px.

## Honest limits
- Testnet only. Pre-release librustzcash crates, because NU7 is live on testnet.
- **What still leaks:**
  - the client's IP reaches the merchant;
  - lightwalletd sees which txids are looked up;
  - `X-PAYMENT` is a bearer credential, and the server serves the same settled pair again for 10 minutes so a lost 200 is not a refusal (the server's feed no longer shows unspent nonces next to txids, and is on a loopback admin port);
  - Kage Tab unlinkability assumes the merchant gives every buyer the same key per epoch; the agent refuses a key change, but can't see what other buyers got;
  - 402 → block timing can link a request to its payment;
  - the sealed agent ledger and tab tokens are readable by anyone holding the ledger key or the agent seed.
- The merchant dashboard doesn't show the payer's spendguard ledger unless `X402Z_DEMO_AGENT_LEDGER` and its key are set, which is for a one-machine demo only.
- Every recorded live run had client, server and facilitator on one machine. A small hosted server and facilitator answer a live testnet 402 at https://kage-api.201-18-198-162.sslip.io/api/quote; no payment has been made to it yet.
- Not audited.

## Pre-existing work (disclosure)
Built during the event window by the same author. Build history: `docs/build-notes.md`.
