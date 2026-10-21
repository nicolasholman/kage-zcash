# Kage (zec-shielded-x402): Zecathon submission draft

**Event / track:** Zecathon, Shielded Payments. This is the team's one entry. Submission deadline: Oct 28 2026, 23:59 UTC.
**Name:** Kage: shielded x402, private machine payments for AI agents
**One line:** AI agents pay for HTTP APIs with Zcash shielded payments in the x402 v1 wire format. The merchant verifies with an incoming viewing key alone, and a spend guard caps every agent before any transaction exists.
**Repo:** github.com/nickthelegend/zec-shielded-x402 (MIT)
**Video:** `x402-demo-v2.mp4`, shared separately until hosted. It shows the Oct 20 agent run (tx `cd64980a…916d`). The x402 v1 wire format, UIVK-only facilitator, ZIP 321 path and MCP server came after the recording and are documented below with their own live txids.

## What it does
- **x402 v1 over Zcash.** The 402 body is `{x402Version: 1, accepts: [{scheme: "zcash-shielded", network: "zcash-testnet", maxAmountRequired, payTo, asset: "ZEC", extra: {memoNonce, expiresAt, paymentUri}}], error}`. The client retries with `X-PAYMENT` (base64 `{x402Version, scheme, network, payload: {txid, memoNonce}}`) and gets `X-PAYMENT-RESPONSE`. The facilitator exposes `/verify`, `/settle` and `/supported`. All of this follows the coinbase/x402 v1 spec.
- **Least-privilege merchant.** The facilitator holds only the merchant's unified *incoming* viewing key. A test shows it can't even see the merchant's own change or outgoing payments. It trial-decrypts Sapling, Orchard and Ironwood outputs and checks recipient, amount, memo nonce and the mined block's time.
- **spendguard.** Daily cap, host allowlist and approval threshold are checked before the wallet is touched. A file lock stops two concurrent agent runs from both passing the cap.
- **Any wallet can pay.** Each 402 carries a ZIP 321 URI and a locally rendered QR code. The dashboard redeems a pasted txid.
- **No money lost on restarts or slow blocks.** Challenges, spent nonces and nonce→txid bindings live in SQLite. Expiry is judged by the block's `minedAt` plus a grace period.
- **MCP server** (`x402z-mcp`) with `paid_fetch`, `spend_status` and `ledger_tail`. Spendguard refusals come back as tool errors.

## Proof it works (all on Zcash testnet)
| Txid | Block | Path |
|---|---|---|
| `030c3cbf78ce8460553bba4e82c0ba813f039bd3ecf4d7f2e81c59837dc28351` | 4521112 | agent's own wallet (Oct 20) |
| `cd64980a5247881a727aa916476a3ba757e59794263c2ff1bcde467654a0916d` | 4521440 | agent's own wallet, the demo video (Oct 20) |
| `ccb4214c6a1fdf183c086ae5b4f559d181ead4c1c61dc7e711fc4a08b95b0ed3` | 4524586 | third-party wallet (zingo-cli) paying the dashboard's ZIP 321 request, txid pasted, `/api/redeem` → paid (Oct 21) |
| `25a8b324bebee30220382113c6fb9cc3be1144b036117c34778bde3b1d8b689d` | 4524602 | `X-PAYMENT` → `settlement_pending` → `200` + `X-PAYMENT-RESPONSE success` (Oct 21) |

Anyone can check all four with the published demo merchant UIVK. See the README, "Verified testnet runs", and the `live_testnet` tests.

69 tests, listed in TESTING.md:
- 6 build in-process transactions with real keys, note encryption and Halo2 proofs (not broadcast);
- 3 verify the four real payments live against testnet.zec.rocks;
- 3 drive the MCP binary over stdio.

## Honest limits
- Testnet only. Pre-release librustzcash crates, because NU7 is live on testnet.
- **What still leaks:**
  - the client's IP reaches the merchant;
  - lightwalletd sees which txids are looked up;
  - the agent ledger is plaintext JSON;
  - 402 → block timing can link a request to its payment.
- The dashboard's spendguard panel is a local demo (payer and merchant on one machine).
- A live paying run from the *agent's own wallet* on the newest code is not in this round. The new code paths were proven live with a separate demo wallet (txids above).
- Not audited.

## Pre-existing work (disclosure)
Built during the event window by the same author. Build history: `docs/build-notes.md`.
