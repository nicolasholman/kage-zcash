# Kage (zec-shielded-x402): Zecathon submission draft

**Event / track:** Zecathon, Shielded Payments. This is the team's one entry. Submission deadline: Oct 28 2026, 23:59 UTC.
**Name:** Kage: shielded x402, private machine payments for AI agents
**One line:** AI agents pay for HTTP APIs with Zcash shielded payments in the x402 v1 wire format. The merchant verifies with an incoming viewing key alone, and a spend guard caps every agent before any transaction exists.
**Repo:** https://github.com/nickthelegend/zec-shielded-x402 (MIT)
**Commit to submit:** `e444893` is the last change before this form-only commit. Submit the tip of `main` once it is pushed: `git log origin/main -1` must show this form commit on top of `e444893`. Today GitHub still serves `1e232bb`, an older README (34 tests, UFVK) and the internal planning notes at `docs/spec-shielded-x402-agents.md`, until Nick pushes.

## Form fields still open (Nick decides)
- **Demo video, about 2 minutes: NICK DECIDES. Nothing was recorded in this round.** The existing `x402-demo-v2.mp4` is 44 s, recorded on Oct 20 before the x402 v1 wire format, the UIVK-only facilitator, ZIP 321, MCP and the Kage Tab clients. It shows tx `cd64980a…916d` and the old receipt header. A new recording on the current code could show these four things:
  - `X-PAYMENT` → `settlement_pending` → `200`;
  - the QR / paste-txid path;
  - a Kage Tab buying 5 tokens, then 5 instant calls and a refused replay;
  - an MCP spendguard refusal.
  The non-paying parts (`check_uivk` on the eight txids, the `live_testnet` tests, MCP refusals) need no wallet.
- **Working demo URL: Demo URL after push: https://nickthelegend.github.io/zec-shielded-x402/** `.github/workflows/pages.yml` publishes `landing/` to GitHub Pages on every push to `main` that touches `landing/` (or by hand from the Actions tab). One-time setup before the push: Settings → Pages → Source "GitHub Actions", or `gh api -X POST repos/nickthelegend/zec-shielded-x402/pages -f build_type=workflow`. The page is one self-contained `index.html` (anchor links and absolute links only), checked locally under the `/zec-shielded-x402/` sub-path. It is docs and recorded output, not a hosted paywall. Nothing is live until Nick pushes and the Pages job goes green.

## Description (paste-ready, 1636 of 2,000 characters)

```
Kage lets AI agents pay for HTTP APIs with Zcash shielded payments, in the x402 v1 wire format. A server answers 402 with a zcash-shielded offer: a shielded address, an amount, a memo nonce and a ZIP 321 URI. The agent's spendguard checks a daily cap, a host allowlist and an approval threshold before any transaction exists. The agent pays, then retries with X-PAYMENT. The merchant's facilitator verifies the payment with only an incoming viewing key. It has no spend key, can't see the merchant's own spends, and returns payer: "".

Any wallet can pay the QR/ZIP 321 request and paste the txid. An MCP server gives LLM agents paid_fetch and tab_fetch. Kage Tab sells N RFC 9474 blind-RSA tokens for one shielded payment. Each token pays for one later request that the server can't link to the payment.

What is real: 8 testnet payments, each checkable by anyone with the published demo merchant UIVK and re-verified by a live test. They cover X-PAYMENT, ZIP 321 + paste-txid, and two Kage Tabs (1 tx -> 5 requests by CLI; 1 tx -> 3 MCP tab_fetch calls). There are 80 tests. 6 of them build transactions in-process with real keys and real Halo2 proofs.

What is not real or not done: testnet only, with pre-release librustzcash crates. In the in-process tests, Sapling outputs use mock proof bytes. Server tests use an in-process stand-in for our own facilitator. The Oct 21 payments came from a zingo-cli demo wallet, not the agent's own wallet. Not audited.

Leaks: the client IP reaches the merchant, lightwalletd sees txid lookups, and the agent ledger and tab tokens are plaintext local files. Timing can link a 402 to its block.
```

## What it does
- **x402 v1 over Zcash.** The 402 body is `{x402Version: 1, accepts: [{scheme: "zcash-shielded", network: "zcash-testnet", maxAmountRequired, payTo, asset: "ZEC", extra: {memoNonce, expiresAt, paymentUri}}], error}`. The client retries with `X-PAYMENT` (base64 `{x402Version, scheme, network, payload: {txid, memoNonce}}`) and gets `X-PAYMENT-RESPONSE`. The facilitator exposes `/verify`, `/settle` and `/supported`. All of this follows the coinbase/x402 v1 spec.
- **Least-privilege merchant.** The facilitator holds only the merchant's unified *incoming* viewing key. A test shows it can't even see the merchant's own change or outgoing payments. It trial-decrypts Sapling, Orchard and Ironwood outputs and checks recipient, amount, memo nonce and the mined block's time.
- **spendguard.** Daily cap, host allowlist and approval threshold are checked before the wallet is touched. A file lock stops two concurrent agent runs from both passing the cap.
- **Any wallet can pay.** Each 402 carries a ZIP 321 URI and a locally rendered QR code. The dashboard redeems a pasted txid.
- **No money lost on restarts or slow blocks.** Challenges, spent nonces and nonce→txid bindings live in SQLite. Expiry is judged by the block's `minedAt` plus a grace period.
- **MCP server** (`x402z-mcp`) with `paid_fetch`, `tab_fetch`, `spend_status` and `ledger_tail`. Spendguard refusals come back as tool errors.
- **Kage Tab.** One shielded payment buys N unlinkable RFC 9474 blind-RSA tokens, each spendable once with `X-PAYMENT` scheme `zcash-shielded-tab`. The agent CLI (`tab-buy`, `tab-redeem`, `tab-fetch`) and the MCP tool `tab_fetch` are the clients. Live on testnet: one 500,000 zat payment bought 5 tokens, spent on 5 requests with no new transaction (each under 1 ms on localhost); a replayed token was refused.

## Proof it works (all on Zcash testnet)
| Txid | Block | Path |
|---|---|---|
| `030c3cbf78ce8460553bba4e82c0ba813f039bd3ecf4d7f2e81c59837dc28351` | 4521112 | agent's own wallet (Oct 20) |
| `cd64980a5247881a727aa916476a3ba757e59794263c2ff1bcde467654a0916d` | 4521440 | agent's own wallet, the demo video (Oct 20) |
| `ccb4214c6a1fdf183c086ae5b4f559d181ead4c1c61dc7e711fc4a08b95b0ed3` | 4524586 | third-party wallet (zingo-cli) paying the dashboard's ZIP 321 request, txid pasted, `/api/redeem` → paid (Oct 21) |
| `25a8b324bebee30220382113c6fb9cc3be1144b036117c34778bde3b1d8b689d` | 4524602 | `X-PAYMENT` → `settlement_pending` → `200` + `X-PAYMENT-RESPONSE success` (Oct 21) |
| `408aba269ac7ca946e3b0eaedcf4cadd582a01090679e98391b4e7c706d88dd0` | 4524693 | `X-PAYMENT` path, repeated (Oct 21) |
| `a7c3b0b2eaeb406ddcd02276cd71e69e4e618b22a1cab8977c959f2da3fc38bc` | 4524694 | ZIP 321 + paste-txid path, repeated (Oct 21) |
| `fa01172015c63ce47a0285f7fae130a3efe5f821b938a8befa1ca547d00ac3ad` | 4526095 | Kage Tab: 500,000 zat → 5 tokens → 5 CLI requests (Oct 21) |
| `d3bf0b49408fed118fe9e1241239d98319cbb83088c45a5ba357f68cd97bd00d` | 4526113 | Kage Tab: 300,000 zat → 3 tokens → 3 MCP `tab_fetch` calls (Oct 21) |

Anyone can check all eight with the published demo merchant UIVK. See the README, "Verified testnet runs", and the `live_testnet` tests.

80 tests, listed in TESTING.md:
- 6 build in-process transactions with real keys, real note encryption and real Halo2 proofs for the Orchard/Ironwood parts (Sapling outputs use sapling-crypto's mock proof bytes); these are not broadcast;
- 3 verify the eight real payments live against testnet.zec.rocks;
- 4 drive the MCP binary over stdio;
- 5 attack the Kage Tab on the server, and 1 runs the agent's Tab client against a localhost issuer with real blind RSA.

## Honest limits
- Testnet only. Pre-release librustzcash crates, because NU7 is live on testnet.
- **What still leaks:**
  - the client's IP reaches the merchant;
  - lightwalletd sees which txids are looked up;
  - the agent ledger and unspent Kage Tab tokens are plaintext local files;
  - 402 → block timing can link a request to its payment.
- The merchant dashboard doesn't show the payer's spendguard ledger unless `X402Z_DEMO_AGENT_LEDGER` is set, which is for a one-machine demo only.
- A live paying run from the *agent's own wallet* on the newest code is not in this round. The new code paths were proven live with a separate demo wallet (txids above).
- Not audited.

## Pre-existing work (disclosure)
Built during the event window by the same author. Build history: `docs/build-notes.md`.
