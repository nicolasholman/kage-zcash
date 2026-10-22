# Kage spec: the `zcash-shielded` x402 scheme

This document describes the wire format and the checks that Kage implements today. It maps x402 v1 onto
Zcash shielded payments. Anything not listed here is not implemented. The README has the runnable steps,
and [TESTING.md](../TESTING.md) lists the tests behind each rule.

- **Base spec:** coinbase/x402 [`specs/x402-specification-v1.md`](https://github.com/coinbase/x402/blob/main/specs/x402-specification-v1.md)
  and [`specs/transports-v1/http.md`](https://github.com/coinbase/x402/blob/main/specs/transports-v1/http.md).
- **Network:** Zcash testnet only. The protocol crate refuses mainnet challenges.
- **Scheme / network ids:** `zcash-shielded` / `zcash-testnet`. Kage Tab tokens use `zcash-shielded-tab`.

## 1. Roles

| Role | Holds | Never holds |
|---|---|---|
| **Client (agent or any wallet)** | its own spending key; `spendguard` policy and ledger | the merchant's keys |
| **Resource server** (`x402z-server`) | the merchant's shielded UA, the facilitator's receipt **public** key, a SQLite challenge book | any Zcash key |
| **Facilitator** (`x402z-facilitator`) | the merchant's unified **incoming** viewing key (UIVK), an ed25519 receipt signing key, a SQLite nonce book | the merchant's spending key or full viewing key |
| **lightwalletd** | the public chain | anything shielded |

## 2. Payment required (HTTP 402)

`GET <resource>` without a valid payment answers `402 Payment Required` with this body:

```json
{
  "x402Version": 1,
  "accepts": [{
    "scheme": "zcash-shielded",
    "network": "zcash-testnet",
    "maxAmountRequired": "100000",
    "resource": "http://127.0.0.1:4020/api/quote",
    "description": "…",
    "mimeType": "application/json",
    "payTo": "utest1…",
    "maxTimeoutSeconds": 900,
    "asset": "ZEC",
    "extra": {
      "memoNonce": "x402z-<24 hex>",
      "expiresAt": 1792561987,
      "paymentUri": "zcash:utest1…?amount=0.001&memo=<base64url(memoNonce)>",
      "tab": { "…": "optional, see §7" }
    }
  }],
  "error": "X-PAYMENT header is required"
}
```

- `maxAmountRequired` is in zatoshi (1 ZEC = 10^8 zat), as a decimal string.
- `payTo` must be a unified address with shielded receivers only. A UA with a transparent receiver is refused.
- `memoNonce` is single use, at most 64 characters from `[A-Za-z0-9_-]`. It goes into the payment's encrypted
  memo as text.
- `expiresAt` is a Unix time (seconds). The challenge is persisted, so a server restart does not lose it.
- `paymentUri` is a [ZIP 321](https://zips.z.cash/zip-0321) URI: `amount` in decimal ZEC (at most 8 decimals,
  no trailing zeros), `memo` as base64url without padding. Any Zcash wallet can pay it. The server also renders
  it as a QR SVG at `GET /api/challenge/{memoNonce}/qr.svg`.
- Each client IP gets a token bucket of `X402Z_CHALLENGE_RATE_PER_MIN` challenges and redeem attempts
  (default 30). Over the limit: `429` with `Retry-After`.

## 3. Paying

The client sends a shielded transaction that pays at least `maxAmountRequired` to `payTo`, with the memo
text equal to `memoNonce`. Sapling, Orchard and Ironwood outputs count. Sapling notes must go to the UA's
Sapling receiver. Orchard and Ironwood notes must go to its Orchard receiver. Outputs in several pools add up.
Transparent outputs never count.

The Kage agent runs `spendguard` before it builds anything:

1. **Host allowlist** (deny by default; exact host or `*.suffix`; case, port and trailing dot ignored).
2. **Amount > 0.**
3. **Daily cap** per agent and UTC day, inclusive, with checked arithmetic. An overflow is a denial.
4. **Approval threshold** (inclusive). The CLI asks the operator. The MCP server always declines.

The first denial wins, and a denied payment builds no transaction. A file lock on the ledger stops two agent
runs from both passing the same cap.

## 4. `X-PAYMENT`

The client retries the resource with `X-PAYMENT: base64(json)`:

```json
{ "x402Version": 1, "scheme": "zcash-shielded", "network": "zcash-testnet",
  "payload": { "txid": "<64 hex>", "memoNonce": "x402z-…" } }
```

A shielded transaction gives the server nothing to look up, so the payload names the challenge it paid.
The facilitator must then find that same nonce inside the encrypted memo. This stops a payment from being
pointed at a different challenge.

Wallets that cannot set headers use `POST /api/redeem {memoNonce, txid}` instead. It runs the same checks
and answers `200` (paid, with the content), `202` (pending), `422` (rejected) or `502` (facilitator down).

## 5. Facilitator

Endpoints: `GET /supported`, `POST /verify`, `POST /settle`, `GET /health`.

- `GET /supported` → `{kinds: [{x402Version: 1, scheme: "zcash-shielded", network: "zcash-testnet"}]}`.
- `POST /verify` and `POST /settle` take `{x402Version, paymentPayload, paymentRequirements}` and require
  `Authorization: Bearer $X402Z_FACILITATOR_TOKEN` (401 otherwise; the resource server holds the token).
  The payload's `memoNonce` must equal `paymentRequirements.extra.memoNonce`.
- `/verify` is read-only: `{isValid, invalidReason, payer}`.
- `/settle` binds the requirement (nonce, payTo, amount) to the txid in SQLite and returns the x402
  settlement fields plus Kage's `receipt` and `claims`. A requirement bound to one txid is refused for any
  other txid, also after a restart; a binding at another amount (a dust payment) doesn't block it.
- `GET /health` makes a live lightwalletd round trip.

For each request the facilitator:

1. fetches the transaction with lightwalletd `GetTransaction` and checks its txid;
2. parses it at the consensus branch of its mined height;
3. trial-decrypts every Sapling, Orchard and Ironwood output with the merchant UIVK;
4. sums the outputs whose memo text equals `memoNonce` and whose recipient is the matching `payTo` receiver;
5. reads the mined block's header time with `GetBlock`.

It refuses on: no matching output, wrong recipient (including a sibling diversified address), underpaid,
mined after `expiresAt + X402Z_EXPIRY_GRACE_SECS` (default 1200 s), a foreign `payTo`, a `payTo` with a
transparent receiver, or a nonce already bound to another txid. Below `X402Z_MIN_CONFIRMATIONS` (default 1)
the answer is "pending" and nothing is bound.

A UIVK decrypts notes sent to the merchant's external addresses and nothing else. It cannot see the
merchant's spends, change or outgoing payments, and it cannot spend. A full viewing key
(`X402Z_MERCHANT_UFVK`) is still accepted, but the facilitator keeps only its incoming part.

### Receipt

`base64url(claims_json) "." base64url(ed25519_signature)`, no padding. The signature covers the exact
`claims_json` bytes.

```json
{ "memoNonce": "x402z-…", "amountZat": 100000, "payTo": "utest1…", "txid": "…",
  "confirmations": 1, "issuedAt": 1792562301, "minedAt": 1792562282 }
```

`minedAt` is absent for a mempool-only payment.

## 6. Settlement at the resource server

The server sends the payment to the facilitator's `/settle`. It then checks the receipt signature against
`X402Z_RECEIPT_PUBKEY` and the claims against the stored challenge: same nonce, same `payTo`, and at least
the asked amount. It accepts when `minedAt <= expiresAt + grace`. A mempool-only receipt has no `minedAt`, so
the time of redemption is used instead. Then it burns the nonce in SQLite, once.

| Outcome | Status | `X-PAYMENT-RESPONSE` (base64 JSON) |
|---|---|---|
| Paid | `200` + content | `{success: true, transaction: <txid>, network: "zcash-testnet", payer: ""}` |
| Not confirmed yet | `402`, the **same** challenge, nothing burned | `{success: false, errorReason: "settlement_pending", …}` |
| Refused | `402`, a fresh challenge | `errorReason` is one of `invalid_payment`, `payment_expired`, `nonce_already_used`, `unknown_challenge`, `invalid_payload`, `invalid_network`, `invalid_scheme` |
| Facilitator unreachable | `502`, nothing burned | `facilitator_error` |

`payer` is always `""`. Nobody learns the payer's address, the merchant included.

The legacy `X-PAYMENT-RECEIPT` header (a receipt presented directly) and the legacy `/verify {challenge, txid}`
body are still accepted.

## 7. Kage Tab (`zcash-shielded-tab`)

One shielded payment buys N tokens. Each token is spent on one later request, and the server cannot link a
token to the payment or to the other tokens. Blind signatures are RFC 9474 RSABSSA-SHA384-PSS-Randomized
with an RSA-2048 key.

1. **Offer.** `extra.tab = {scheme: "zcash-shielded-tab", suite, keyId, publicKey (SPKI, base64),
   unitPriceZat, maxTokens, issuePath: "/tab/issue", epochEndsAt, graceSecs}`.
2. **Buy.** Pay `n × unitPriceZat` (n ≤ `maxTokens`) with the nonce as memo. Blind n random 32-byte messages
   and `POST /tab/issue {memoNonce, txid, keyId, blinded[n]}`. The server confirms the payment through the
   facilitator, checks that it covers n tokens, burns the nonce once and returns n blind signatures. An
   underpaid n burns nothing.
3. **Spend.** `X-PAYMENT: base64 {x402Version: 1, scheme: "zcash-shielded-tab", network: "zcash-testnet",
   payload: {keyId, msg, randomizer, sig}}`. The server verifies the signature and records
   `SHA-256(randomizer ‖ msg)` in a SQLite spent set. Success answers `200` with
   `transaction: "tab:<token id prefix>"`.

Keys rotate every `X402Z_TAB_EPOCH_SECS` (default 7 days). For `graceSecs` (`X402Z_TAB_GRACE_SECS`,
default 1800) after its epoch, the previous key still issues a tab paid under it and accepts its tokens.
`/tab/issue` with any other key is `409 tab_key_rotated` and burns nothing: blind again for the active key
and retry the same payment. Unlinkability assumes every buyer gets the same key per epoch; the agent pins
the key it saw per server per epoch and refuses another.

Clients: `x402z-agent tab-buy <n>` (agent wallet) or `tab-buy <n> --external` + `tab-redeem <txid>` (any
wallet pays the printed ZIP 321 request), then `tab-fetch`; the MCP tool `tab_fetch`. Spendguard checks the
whole `n × unitPriceZat` before anything is paid. Live on testnet: on 2026-10-21 tx `fa011720…c3ad` bought
5 tokens spent on 5 CLI requests and tx `d3bf0b49…d00d` bought 3 spent by MCP `tab_fetch` from a script;
on 2026-10-22 an LLM's `tab_fetch` paid tx `b75b2415…4b91` from the agent's own wallet and spent its 3
tokens (README, "Kage Tab").

## 8. MCP surface

`x402z-mcp` speaks MCP over stdio (newline-delimited JSON-RPC 2.0, protocol `2025-06-18`; `2025-03-26` and
`2024-11-05` also accepted). Tools: `paid_fetch {url}`, `redeem_payment {url, txid, memoNonce}`,
`tab_fetch {url, buy?}`, `spend_status {}`, `ledger_tail {n}`. `paid_fetch` returns
`settlement_pending` right after the broadcast, and a retry for the same URL presents that payment again
instead of paying twice; `redeem_payment` never pays. Spendguard refusals, wallet errors, rejected
payments and missing configuration come back as `isError: true` results.

## 9. What this does not hide

- The merchant sees the client's IP. The paid request is ordinary HTTP.
- lightwalletd sees which txid is looked up, and when.
- `X-PAYMENT` is a bearer credential until redeemed. The server's feed is on a loopback admin port and
  shows only a nonce prefix next to a txid.
- Kage Tab unlinkability assumes one key per epoch for every buyer.
- The agent ledger and tab tokens are sealed (XChaCha20-Poly1305); the ledger key or the agent seed opens
  them.
- The 402 and the next block can be linked by timing.

## 10. Latency

Testnet blocks arrive about every 75 s and sometimes much slower. The default is 1 confirmation. The CLI
agent retries `X-PAYMENT` every 15 s for up to `X402Z_AGENT_CONFIRM_TIMEOUT_SECS` (default 900); the MCP
tools return `settlement_pending` at once. Every `X-PAYMENT` attempt costs one token of the client's
per-minute budget on the server.
`X402Z_MIN_CONFIRMATIONS=0` on both server and facilitator accepts mempool-only payments; this is weaker,
and the server logs a warning.
