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
| `crates/x402z-server`, `x402z-facilitator`, `x402z-agent` | **Scaffold only.** They check config and exit non-zero |
| Encrypted audit log, receipts, lightwalletd scanning, dashboard | Not started (M2–M4) |

Nothing is mocked. The binaries do no fake work: they stop with an error.

## Do X → see Y

1. `cargo test` → you should see `11 passed` (spendguard) and `4 passed` (protocol).
2. `cargo run -p x402z-facilitator` with no `.env` → it prints `error: ZCASH_LWD_URL not configured` and exits with code 2.
3. `cp .env.example .env` and fill it in → each binary moves on to the next missing var, then reports `not implemented yet (milestone Mx)` and exits with code 1.

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
