# Kage (zec-shielded-x402): Zecathon submission draft

**Event / track:** Zecathon, Shielded Payments. This is the team's one entry. Submission deadline: Oct 28 2026, 23:59 UTC.
**Name:** Kage: shielded x402, private machine payments for AI agents
**One line:** AI agents pay for HTTP APIs with Zcash shielded payments in the x402 v1 wire format. The merchant verifies with an incoming viewing key alone, and a spend guard caps every agent before any transaction exists.
**Repo:** https://github.com/nickthelegend/zec-shielded-x402 (MIT)
**Commit to submit:** the tip of `main` on GitHub (`git log origin/main -1`, which must equal the local `git log main -1`). Do not submit `1e232bb` or any earlier hash named in older notes.

## Form fields still open (Nick decides)
- **Demo video (4:24, v6, ready; Nick uploads it and pastes the link):** the film is kept out of git
  (`grokbot/videos/kage/kage-film-v6.mp4`, 1920x1080, HyperFrames, Chatterbox narration, one-line captions; phone copy
  `grokbot/videos/phone/kage-film-v6-phone.mp4`). It opens
  with the problem in plain words over a live capture of x402scan.com (real public x402 payment totals: 3.13M
  payments and 34.87K buyers in 30 days, and one service's 591 buyers) and a kinetic-type line on runaway wallets
  (no invented numbers anywhere), then shows, all recorded live on Oct 27-28: `npm install kage-x402` from the public registry; the
  10-line agent code with its policy; the merchant's `pay('0.001 ZEC')` routes; `kage wallet new --role merchant`
  (seed to a 0600 file, UIVK for the facilitator); and an LLM agent chat on Zcash testnet that pays 0.001 ZEC for the
  live price (`401cad2c…77fa`, shown on testnet.zecblock.com), asks a human before paying 0.005 ZEC for the weekly
  report (`6c1229d1…d0d2`), is refused `HOST_NOT_ALLOWED` for a host off its list (no request made) and
  `CAP_EXCEEDED` for one report too many. v6 adds three more examples from a second live session on Oct 28 with a
  tighter policy (cap 0.003 ZEC/day, approval from 0.001 ZEC): the agent buys 12 ZEC in euros from a new converter
  endpoint for 0.0005 ZEC (`f5b4fdd1…f000`) and live Zcash network activity for 0.0005 ZEC (`c00b0175…04d4`), both
  shown on testnet.zecblock.com, then asks before paying 0.001 ZEC for 30 days of price history and the human
  declines (`APPROVAL_DENIED`, nothing paid). Idle pauses are shortened and block waits sped up, badged on screen.
  The closing card shows `npm install kage-x402` and no repository URL.
  Do NOT submit `x402-demo-v2.mp4`, the v4 film (v4 replays an older run) or v5 (replaced by v6).
- **Working demo URL:** https://nickthelegend.github.io/zec-shielded-x402/ (GitHub Pages; `.github/workflows/pages.yml` publishes `landing/` on every push to `main`). It is **docs and recorded output, not a hosted paywall**; the page says so. A judge can run the whole stack locally with four commands (a cargo build, then `cd e2e && npm ci && npm run stack`, and `npm run test:e2e` in Google Chrome). Hosted now: the same page at https://kage-landing.vercel.app, and a live testnet 402 at https://kage-api.201-18-198-162.sslip.io/api/quote (facilitator health: https://kage-facilitator.201-18-198-162.sslip.io/health), on a small VPS behind TLS.

## Description (paste-ready, 1,810 of 2,000 characters)

```
Kage lets AI agents pay for HTTP APIs with Zcash shielded payments, in the x402 v1 wire format, inside a spend policy the developer writes. `npm install kage-x402`: createKage({wallet, policy, onApproval}) gives an agent kage.fetch, which answers a 402 by checking a daily cap, a host allowlist and an approval threshold before any transaction exists, then pays from the agent's own shielded wallet and returns the paid response. Above the threshold a human approves through a secret the LLM never sees. Merchants add kagePaywall('0.001 ZEC') per route; their facilitator verifies payments with only an incoming viewing key and returns payer: "".

Also: an MCP server for LLM clients, any-wallet ZIP 321 payment with paste-txid, Kage Tab (N blind-RSA tokens for one payment, unlinkable to it), a CLI that keeps seeds in 0600 files.

What is real: 15 testnet payments, each checkable on testnet.zecblock.com; on Oct 27 an LLM agent chat paid 0.001 and (human-approved) 0.005 ZEC for live market data from a paid API built on the SDK, and was refused for a host off its list and over its cap. kage-x402 is on npm. 150 tests (131 Rust, 19 TypeScript); 6 build transactions with real Halo2 proofs; the SDK tests drive the real engine against live lightwalletd.

Not real or not done: testnet only, pre-release librustzcash crates, macOS arm64 engine binary only (others build from source), not audited.

Leaks: the client IP reaches the merchant; lightwalletd sees txid lookups; X-PAYMENT is a bearer credential; Kage Tab unlinkability assumes one key per epoch; timing can link a 402 to its block. The agent ledger is encrypted at rest.
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
- **TypeScript** (`packages/kage`, npm `kage-x402`): client helpers and an Express/Node paywall middleware (per-client rate limit, facilitator timeout, pruned and capped in-memory store, resend window), tested against the real binaries and testnet.

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

| `5080f61a476cad3eafd19769d5694fd442128cde7cdfb92955a5012b4e8f0302` | — | **kage-x402 agent chat** (`examples/agent-chat`): LLM buys the live ZEC price, 0.001 ZEC (Oct 27) |
| `4f74601665fe8f288200012b1270eddbd782da49b15ac1df917777fd4a0ae891` | 4548529 | agent chat, film take 1: live price (Oct 27) |
| `80af9466590d7e3125f7bfd7037d8ab86ae2df742fe1978aa5b9a2f73613acd6` | — | agent chat, take 1: weekly report, 0.005 ZEC after human approval (Oct 27) |
| `401cad2c15c26dd26b2d3f17400318bbe73c39c35e04032037ed8ae33c1a77fa` | 4548551 | agent chat, film take 2 (in the v5 film): live price, 0.001 ZEC (Oct 27) |
| `6c1229d115b82acbd0af4be834e6d5ba1c36703009758136713d0d50eb81d0d2` | — | agent chat, take 2 (in the film): weekly report, 0.005 ZEC after human approval (Oct 27) |
| `f5b4fdd10a07a2e28d3f92edc0995043693f31f7fa5f3ddeedf1756a2b33f000` | 4550355 | agent chat, second session (in the v6 film): `/v1/zec/convert` 12 ZEC in euros, 0.0005 ZEC, no approval needed (Oct 28) |
| `c00b01754d20dc7bdd41a5d87f8b4e5789792511ea7978213ca093d1e2a104d4` | 4550358 | agent chat, second session (in the v6 film): `/v1/zec/network` live network activity, 0.0005 ZEC (Oct 28); the same session's 0.001 ZEC history request was declined by the human, nothing paid |
Anyone can check the first ten with the published demo merchant UIVK; the seven agent-chat payments went to a new merchant created with `kage wallet new --role merchant`. See the README, "Verified testnet runs", the `live_testnet` tests and `docs/mcp-paid-run.txt` (the LLM run, rendered from the raw opencode events in `docs/mcp-paid-run.jsonl`).

150 tests, listed in TESTING.md:
- 131 Rust (8 MCP stdio tests, one proving a wrong or missing approval secret approves nothing): 6 build in-process transactions with real keys, real note encryption and real Halo2 proofs for the Orchard/Ironwood parts (Sapling outputs use sapling-crypto's mock proof bytes; not broadcast); 3 verify the ten real payments live against testnet.zec.rocks; 8 drive the MCP binary over stdio; 6 attack the Kage Tab on the server; 11 run the agent's Tab client, key rules, redirect refusal, racing calls, lost answers and the pre-request allowlist against localhost servers.
- 19 TypeScript: 8 drive the kage-x402 SDK against the real engine and testnet lightwalletd (policy refusals, human approval, wallet sync); 2 run the client and middleware against the real binaries and testnet; 9 need no chain.
- Separately, 35 Playwright specs drive the dashboard, the landing page and the spec page in Google Chrome at 1280 px and 375 px (69 passed, 1 mobile-only spec skipped on desktop, Oct 28).

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
