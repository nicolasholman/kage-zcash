# Shielded x402: private machine payments for AI agents

HTTP 402 payments for AI agents, settled as **Zcash shielded** testnet transactions. The merchant's facilitator verifies a payment with a viewing key alone (it holds no spend key). Before the agent builds any transaction, **spendguard** checks it against a daily cap, a host allowlist and an approval threshold.

> Context: the ecosystem asked for a "Shielded x402" grant (ZCG, $42K). It was **rejected** on Sep 28 2026, so nobody funded it. This repo is our own independent implementation of the idea and does not reuse that proposal's code.

```
agent -> GET /api/quote -> 402 {scheme, network, payTo, amountZat, memoNonce, expiresAt}
      -> spendguard.check() -> shielded tx (memo = nonce)
      -> facilitator scans with UFVK via lightwalletd -> ed25519 receipt
      -> agent retries with X-PAYMENT-RECEIPT -> 200
```

## Status (honest)

| Part | State |
|---|---|
| `crates/spendguard`: cap, allowlist, approval threshold | **Done.** 11 unit tests |
| `crates/x402z-protocol`: 402 challenge type and validation | **Done.** 4 tests |
| `crates/x402z-server`: paywalled `/api/quote`, receipt verification, dashboard | **Done.** 4 tests (ed25519 receipt, replay, underpaid, expired, wrong address) |
| `x402z-facilitator`, `x402z-agent` | **Scaffold only.** They check config and exit non-zero |
| Encrypted audit log, receipt *issuing*, lightwalletd scanning, agent ledger writer | Not started (M2–M3) |

Nothing is mocked. The binaries do no fake work: they stop with an error.

## Do X → see Y

1. `cargo test` → you should see `11 passed` (spendguard) and `4 passed` (protocol).
2. `cargo run -p x402z-facilitator` with no `.env` → it prints `error: ZCASH_LWD_URL not configured` and exits with code 2.
3. `cp .env.example .env` and fill it in → each binary moves on to the next missing var, then reports `not implemented yet (milestone Mx)` and exits with code 1.

## Dashboard (served by x402z-server)

Fill in `X402Z_MERCHANT_UA`, `X402Z_RECEIPT_PUBKEY` and `X402Z_FACILITATOR_URL`, run `cargo run -p x402z-server`, then open http://127.0.0.1:4020.

- Press **Request paid data →** and within a second you get a live HTTP 402 challenge (payTo, amount, memo nonce, 15-minute countdown), and the event shows up in the **Live payment feed**.
- Send a request with a valid `X-PAYMENT-RECEIPT` and the feed shows `PAID` plus the txid, and `/api/quote` returns 200.
- A bad, replayed, underpaid or expired receipt shows up as `REJECTED` and comes back with a fresh 402 and an `x-payment-error` header.
- **Spend caps / audit trail** read `X402Z_AGENT_LEDGER`, a JSON file `{agents:[{agent,dailyCapZat,spentTodayZat,approvalThresholdZat,allowedHosts}], audit:[{ts,agent,host,amountZat,decision,reason}]}`. If it is unset or unreadable the page shows a red banner naming the variable. Nothing writes this file yet: the agent is still a scaffold.
- Receipt wire format: `base64url(claims_json).base64url(ed25519_sig)`, with claims as in `x402z_protocol::ReceiptClaims`.
- The layout collapses to one column below 860px, so it works on a phone (`X402Z_BIND=0.0.0.0:4020`).

## spendguard rules

These are checked in order. The first denial wins, so a payment that would be denied never reaches an approval prompt.

1. **Host allowlist.** Hosts are denied by default. An entry is an exact host or a `*.suffix` wildcard; `*.data.test` matches `x.data.test` but not `data.test` or `evildata.test`. Host matching ignores case, ports and trailing dots.
2. **Amount must be > 0.**
3. **Daily cap** per agent, measured per UTC day. The cap is inclusive, and sums use checked arithmetic so an overflow counts as a denial.
4. **Approval threshold** (inclusive): at or above it, the result is `NeedsApproval`.

`record_spend` re-runs the check before it updates the ledger. Call it only after a broadcast.

## Latency policy

Testnet blocks arrive about every 75 s. The facilitator will accept a payment at 1 confirmation by default. An optional mode to accept from the mempool is planned for low-value quotes, and it will be labelled clearly as weaker.

## Blockers / open checks

- **Leash port.** I couldn't read `../leash/packages/core` because the Loom sandbox blocks paths outside the project. spendguard here was written from the CLAUDE.md spec. It needs to be compared against leash once access is granted.
- **Shielded pool.** CLAUDE.md says Orchard is sealed and Ironwood/NU6.3 is live. `zcash_client_backend 0.24.0` exposes an `orchard` feature but **no Ironwood feature**. Confirm which pool the current crates can send to before writing the wallet code.
- **lightwalletd.** `testnet.zec.rocks:443` is reachable, but no official doc listing it was found.
- **Faucet.** `testnet.zecfaucet.com` does **not resolve** (DNS failure, 2026-10-20). We need a working faucet or TAZ from someone.
- **Keys/wallet.** We have no merchant UA/UFVK, receipt signing key or funded agent wallet yet, and `zingo-cli` is not installed.
