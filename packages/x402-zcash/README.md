# @kage/x402-zcash

x402 v1 over Zcash shielded payments for TypeScript, matching Kage's Rust server and facilitator.
No runtime dependencies; Node >= 22.18 runs the `.ts` sources directly (type stripping).

Client:
```ts
import { parsePaymentRequired, zip321Uri, fetchWithPayment } from '@kage/x402-zcash';
const r = await fetch('http://127.0.0.1:4020/api/quote');            // 402
const c = parsePaymentRequired(await r.json());                       // the zcash-shielded / zcash-testnet option
const uri = zip321Uri(c.payTo, c.amountZat, c.memoNonce);             // any Zcash wallet can pay this
// ...pay it, then present the txid (repeat while settlement.errorReason === 'settlement_pending'):
const out = await fetchWithPayment(c.resource, { txid, memoNonce: c.memoNonce });
```

Server (Express or plain `node:http`): `paywall({facilitatorUrl, facilitatorToken, receiptPubkeyHex, payTo, amountZat})`
answers 402 with a fresh challenge, settles `X-PAYMENT` through the Kage facilitator's `/settle`
(bearer `X402Z_FACILITATOR_TOKEN`), accepts only an ed25519 receipt from `X402Z_RECEIPT_PUBKEY`
for that nonce, txid, payTo and amount, burns the nonce once and calls `next()` with
`X-PAYMENT-RESPONSE` set. Like x402z-server it limits 402s and `X-PAYMENT` attempts to
`ratePerMin` (30) per client (429 + `Retry-After`; pass `clientKey` behind a proxy), gives up on
the facilitator after `facilitatorTimeoutMs` (90 s, a 502 that burns nothing), and answers the
same `X-PAYMENT` again for `resendWindowSecs` (600) after it settled, so a lost 200 is not a
refusal. Challenges live in memory by default (`memoryStore()`: one process, lost on restart,
pruned once their redeem window is over, at most `maxEntries` = 100,000, then 503). Pass a
durable `store` for production.

Not done: the x402 v2 header names (`PAYMENT-REQUIRED` / `PAYMENT-SIGNATURE`) and the Kage Tab
scheme; the Rust agent (`x402z-agent`, `x402z-mcp`) is still the paying client.

Tests (`npm test`, 11): 9 without a chain (pure functions, the config check, and the paywall's
limits over in-process HTTP: bounded store, 503 when full, rate limit, facilitator timeout), plus 2
live against the real `x402z-server` and `x402z-facilitator` binaries (published demo UIVK,
throwaway receipt key) and Zcash testnet: a real mined payment presented for another nonce is
refused by the server, and the middleware settles real tx `ccb4214c…0ed3` for its own nonce once,
serves it again inside the resend window without burning anything new, and refuses it after. Build the
binaries first: `cargo build -p x402z-server -p x402z-facilitator --bins --examples`.
