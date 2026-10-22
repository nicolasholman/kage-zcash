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
`X-PAYMENT-RESPONSE` set. Challenges live in memory by default (`store` takes your own).

Not done: the x402 v2 header names (`PAYMENT-REQUIRED` / `PAYMENT-SIGNATURE`) and the Kage Tab
scheme; the Rust agent (`x402z-agent`, `x402z-mcp`) is still the paying client.

Tests (`npm test`, 7): pure functions, plus a live run against the real `x402z-server` and
`x402z-facilitator` binaries (published demo UIVK, throwaway receipt key) and Zcash testnet: a
real mined payment presented for another nonce is refused by the server, and the middleware
settles real tx `ccb4214c…0ed3` for its own nonce once, then refuses the replay. Build the
binaries first: `cargo build -p x402z-server -p x402z-facilitator --bins --examples`.
