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
| `crates/x402z-server`: paywalled `/api/quote`, receipt verification, dashboard, per-IP 402 rate limit | **Done.** 10 tests (ed25519 receipt, replay, underpaid, expired, wrong address, 429 + Retry-After) |
| `crates/x402z-facilitator`: lightwalletd gRPC, UFVK trial decryption (Sapling, Orchard, Ironwood), receipt signing | **Done.** 4 integration tests on real shielded transactions (real keys, real Halo2 proofs, real ed25519). Verified live against testnet.zec.rocks |
| `crates/x402z-agent`: 402 → spendguard → `zcash_client_sqlite` wallet → pay → receipt → retry, ledger writer | **Done.** 4 ledger tests. The live run reached the wallet and stopped: it has no testnet funds (see "Live testnet run") |
| Encrypted audit log (only the owner's viewing key can read it) | Not started. The ledger is plain JSON |
| A paid end-to-end run on testnet | **Not done yet.** Needs a funded testnet wallet (see below) |

Nothing is mocked. If a binary is missing a key, a wallet or funds, it stops with an error and a non-zero exit code. It never fakes a payment.

## Do X → see Y

1. Run `cargo test --workspace`. You should see 33 tests pass: spendguard 11, protocol 4, server 10, agent 4, facilitator 4. The facilitator tests build real shielded transactions with Halo2 proofs and take about 30 s.
2. Run `cargo run -p x402z-facilitator` with no `.env`. It prints `error: X402Z_MERCHANT_UFVK not configured` and exits with code 2.
   - If `ZCASH_LWD_URL` is unset, it says so and uses `https://testnet.zec.rocks:443`.
   - If lightwalletd is unreachable, it prints `error: cannot connect to lightwalletd: …` and exits with code 1.
3. Run `cargo run -p x402z-facilitator -- keygen`. It prints a fresh `X402Z_RECEIPT_SIGNING_KEY` / `X402Z_RECEIPT_PUBKEY` pair.
4. Run `X402Z_AGENT_WALLET_DIR=./wallets/merchant cargo run -p x402z-agent -- new-wallet`. It creates a real testnet wallet and prints its seed phrase, birthday height, shielded address and UFVK. Use one such wallet for the merchant (UA + UFVK) and another for the agent.
5. Run the agent. Each exit code means one thing:

   | Situation | Output | Exit |
   |---|---|---|
   | Cap below the price, or host not in `X402Z_AGENT_ALLOWED_HOSTS` | `spendguard refused the payment: … No transaction was built or broadcast.` | 3 |
   | No `X402Z_AGENT_SEED` | `X402Z_AGENT_SEED not configured: no funded testnet wallet to pay from …` | 4 |
   | Wallet synced but holding too little TAZ | `insufficient testnet balance: spendable 0 zat …, need at least 110000 zat … Send TAZ to utest1…` | 4 |
   | Facilitator rejects the tx | `facilitator rejected payment tx …` | 5 |
   | Paid | the paid JSON on stdout | 0 |

   Every decision is appended to `X402Z_AGENT_LEDGER`, and the dashboard shows it.
6. Send more than `X402Z_CHALLENGE_RATE_PER_MIN` requests (default 30) to `/api/quote` from one IP. You get `429 Too Many Requests` with a `Retry-After` header. A request that redeems a valid receipt is never rate limited.

## Facilitator API (merchant side, viewing key only)

- `GET /health` makes a live lightwalletd round trip. It returns `200 {ok, chainTip, lightwalletd, receiptPubkey, minConfirmations}`, or `503` with the error.
- `POST /verify` takes `{challenge: <the 402 JSON>, txid: "<explorer-order hex>"}`. The facilitator then:
  1. fetches the tx with `GetTransaction`;
  2. parses it at the consensus branch of its mined height;
  3. trial-decrypts it with the UFVK's external incoming viewing key;
  4. sums the outputs whose text memo equals `memoNonce` and whose recipient is `payTo`'s receiver for that pool. Sapling notes must go to the Sapling receiver. Orchard and Ironwood notes must go to the Orchard receiver.

  Responses:
  - `200 confirmed`, with a `receipt`.
  - `202 pending`: found, but below `X402Z_MIN_CONFIRMATIONS` (default 1).
  - `404 not_found`.
  - `422 rejected`: no matching output, wrong recipient, underpaid, foreign `payTo`, or a nonce already receipted for another tx.
  - `400`: bad challenge or txid.
  - `502 error`: lightwalletd failed.

  The types are `x402z_protocol::{VerifyRequest, VerifyResponse}`.

## Dashboard (served by x402z-server)

Fill in `X402Z_MERCHANT_UA`, `X402Z_RECEIPT_PUBKEY` and `X402Z_FACILITATOR_URL`, run `cargo run -p x402z-server`, then open http://127.0.0.1:4020.

- Press **Request paid data →** and within a second you get a live HTTP 402 challenge (payTo, amount, memo nonce, 15-minute countdown), and the event shows up in the **Live payment feed**.
- Send a request with a valid `X-PAYMENT-RECEIPT` and the feed shows `PAID` plus the txid, and `/api/quote` returns 200.
- A bad, replayed, underpaid or expired receipt shows up as `REJECTED` and comes back with a fresh 402 and an `x-payment-error` header.
- **Spend caps / audit trail** read `X402Z_AGENT_LEDGER`, a JSON file `{agents:[{agent,dailyCapZat,spentTodayZat,approvalThresholdZat,allowedHosts}], audit:[{ts,agent,host,amountZat,decision,reason}]}`. If it is unset or unreadable, the page shows a red banner naming the variable. `x402z-agent` writes this file. Its audit entries also carry `memoNonce`, plus `txid` once a payment is broadcast.
- Receipt wire format: `base64url(claims_json).base64url(ed25519_sig)`, with claims as in `x402z_protocol::ReceiptClaims`.
- Abuse limit: each client IP gets a token bucket of `X402Z_CHALLENGE_RATE_PER_MIN` challenges (default 30; `0` turns it off). This also caps how fast one client can grow the server's table of open challenges.
- The layout collapses to one column below 860px, so it works on a phone (`X402Z_BIND=0.0.0.0:4020`).

## spendguard rules

These are checked in order. The first denial wins, so a payment that would be denied never reaches an approval prompt.

1. **Host allowlist.** Hosts are denied by default. An entry is an exact host or a `*.suffix` wildcard; `*.data.test` matches `x.data.test` but not `data.test` or `evildata.test`. Host matching ignores case, ports and trailing dots.
2. **Amount must be > 0.**
3. **Daily cap** per agent, measured per UTC day. The cap is inclusive, and sums use checked arithmetic so an overflow counts as a denial.
4. **Approval threshold** (inclusive): at or above it, the result is `NeedsApproval`.

`record_spend` re-runs the check before it updates the ledger. Call it only after a broadcast.

## Latency policy

Testnet blocks arrive about every 75 s. By default, both the facilitator and the server require 1 confirmation (`X402Z_MIN_CONFIRMATIONS`). Below that, the facilitator answers `202 pending`. The agent polls every 15 s, for up to `X402Z_AGENT_CONFIRM_TIMEOUT_SECS` (default 900). Setting `X402Z_MIN_CONFIRMATIONS=0` on both accepts mempool-only payments; that is weaker, and the server logs a warning.

## Live testnet run

**What ran on 2026-10-20** against `https://testnet.zec.rocks:443` (ECC LightWalletD v0.5.4, chain `test`):

- **Facilitator health.** `GET /health` returned `chainTip 4520789`, and `4520801` on a later run.
- **Real transactions.** I sent `POST /verify` the txids of real mined NU7 Ironwood transactions (blocks 4520784–4520789). The facilitator fetched, parsed, txid-checked and trial-decrypted them. It answered `422 rejected: no output in this transaction decrypts to the merchant viewing key`, which is correct: they don't pay us.
- **Unknown txid.** It answered `404 not_found`.
- **Agent.** It got the 402 and spendguard allowed the payment:
  - With no seed, it exited 4.
  - With a fresh real wallet (`new-wallet`, birthday 4520790), it synced to the tip, reported `spendable 0 zat`, printed the address to fund, and exited 4.
  - With the cap or the allowlist violated, it exited 3 without touching the wallet.
- **No payment was made in this run**, because the wallet had no testnet ZEC yet. A later funded run paid end to end; see [Verified testnet run](#verified-testnet-run).

**What a paid run needs** (all three were in place for the [Verified testnet run](#verified-testnet-run)):

1. **A funded testnet wallet for the agent.** Put its 24-word BIP 39 phrase in `X402Z_AGENT_SEED` and its birthday height in `X402Z_AGENT_BIRTHDAY`. It needs at least 0.0011 TAZ spendable (price plus fee); more allows repeated runs. Alternatively, run `x402z-agent new-wallet` and send TAZ to the address it prints.
2. **A merchant UA and UFVK.** Any testnet wallet's UFVK works, as long as the UA is derived from it. Or run `new-wallet` into a second directory; you only need its seed later, to spend the income.
3. **A receipt key pair**, from `x402z-facilitator keygen`.

**Commands.** Put every variable from `.env.example` in one `.env`, then:
```sh
set -a; . ./.env; set +a
cargo build --workspace
target/debug/x402z-facilitator &      # logs "lightwalletd ... tip=<height>", listens on 127.0.0.1:4021
target/debug/x402z-server &           # dashboard on http://127.0.0.1:4020
target/debug/x402z-agent balance      # syncs the agent wallet: spendable must be >= 110000 zat
target/debug/x402z-agent              # 402 -> spendguard -> pay -> wait ~1 block -> receipt -> 200 JSON on stdout
X402Z_AGENT_DAILY_CAP_ZAT=1 target/debug/x402z-agent   # cap demo: exit 3, nothing broadcast
```
The first payment downloads the Sapling proving parameters to the default Zcash params folder (about 50 MB, once).

## Verified testnet run

**Paid end to end on 2026-10-20** (11:14 IST) against `https://testnet.zec.rocks:443`, with nothing mocked:

- **Payment txid:** [`030c3cbf78ce8460553bba4e82c0ba813f039bd3ecf4d7f2e81c59837dc28351`](https://testnet.zecblock.com/tx/030c3cbf78ce8460553bba4e82c0ba813f039bd3ecf4d7f2e81c59837dc28351) (testnet explorer; mined in block 4521112).
- **Price:** 100,000 zat (0.001 TAZ), plus a 10,000 zat network fee. The agent's spendable balance went from 12,500,000 to 12,390,000 zat.
- **Facilitator:** `CONFIRMED 100000 zat via [Ironwood], 2 conf (tip 4521113); receipt issued`.
- **Server:** `GET /api/quote` answered `200 OK` with the paid content and this settlement block:
  ```json
  "settlement": {
    "amountZat": 100000,
    "confirmations": 2,
    "memoNonce": "x402z-f19a63da20cbc0749ea800b3",
    "txid": "030c3cbf78ce8460553bba4e82c0ba813f039bd3ecf4d7f2e81c59837dc28351"
  }
  ```

**Check it yourself with the merchant's incoming viewing key.** ⚠️ This is a **demo-only testnet merchant wallet**. Its unified incoming viewing key is published on purpose and reveals every incoming payment (amount and memo) to this wallet. It cannot spend, and it is not a seed or spending key. Never publish a viewing key for a real wallet.

```
uivk=uivktest1ma9hgu9cmmmkv5f5npk7aktj8tradzqyxzwerez5trhku0xxgx7vsh0azzurz44pfdhmyrzxnur3n282f5t0glhje7kay4fw79njmg64g2wupcznmpfpy0gh6wngu0qw07ry404hnh6rq9vuj264gcjfxucylma4c8cuygxezqx2xwph5x66nckx4095pgvjzuq5rudsmr9dztnyv9f2w74unppttta6cu7fr3tx494zqstn2hv
```

**Why this is not a leak.** This is the merchant's own incoming viewing key for a demo-only testnet wallet. The merchant published it by choice so anyone can audit this payment. What it shows and doesn't show:
- **It shows** payments coming into this merchant wallet: their amounts, their memos, and which of the merchant's addresses received them. That covers every incoming payment to this wallet, not only this one.
- **It doesn't show** the merchant's spends or change, the payer's address, or anything about the agent's wallet: its balance, its other transactions or its keys.
- **It can't** spend funds.

In normal use the merchant keeps this key private, and the payer's side stays shielded either way.

```sh
cargo run --release -p x402z-facilitator --example check_uivk -- "$uivk" \
  030c3cbf78ce8460553bba4e82c0ba813f039bd3ecf4d7f2e81c59837dc28351 x402z-f19a63da20cbc0749ea800b3
```

`check_uivk` fetches the transaction from `testnet.zec.rocks` (override with `X402Z_LWD_URL`), checks the txid, and trial-decrypts every Sapling, Orchard and Ironwood output with the incoming key alone. Tested on 2026-10-20:

```
tx 030c3cbf…8351: mined at 4521112, 20 confirmation(s) (tip 4521131)
incoming 100000 zat via Ironwood, memo text "x402z-f19a63da20cbc0749ea800b3"
total to merchant with memo "x402z-f19a63da20cbc0749ea800b3": 100000 zat
```

With a different memo nonce it prints `0 zat` and exits 2. If no output decrypts, it exits 1.

**Gotcha: Sapling params.** The first attempt failed with `sapling-spend.params failed validation` because the ~50 MB download was cut off (only 5,982,770 of 47,958,396 bytes arrived), so the hash check rejected it and no payment was sent. Fix: delete the partial `sapling-spend.params` from the Zcash params folder and download it again; the retry above then went through.

## Blockers / open checks

- **NU7 is live on testnet.** It activated at height 4465026, with branch id `0x77190AD9`.
  - The stable crates (`zcash_protocol` 0.10.6, `zcash_primitives` 0.30, `zcash_client_backend` 0.24) reject every current testnet transaction with `invalid consensus branch id 0x77190ad9`.
  - So we pin the **pre-release** line: `zcash_protocol 0.11.0-pre.1`, `zcash_primitives 0.31.0-pre.2`, `zcash_client_backend 0.25.0-pre.2`, `zcash_client_sqlite 0.23.0-pre.2` and `zcash_keys 0.17.0-pre.2`.
  - Check crates.io for stable releases before the freeze.
- **Toolchain.** These crates need Rust 1.88 or later, so `rust-toolchain.toml` is now `1.89.0` (it was 1.85).

- **Leash port.** I couldn't read `../leash/packages/core` because the Loom sandbox blocks paths outside the project. spendguard here was written from the CLAUDE.md spec. It needs to be compared against leash once access is granted.
- **Shielded pool.** Settled. The crates trial-decrypt Ironwood bundles with the Orchard viewing key, and a UA's Orchard receiver also receives in Ironwood. The agent sends change to Ironwood. The facilitator accepts Sapling, Orchard and Ironwood outputs.
- **lightwalletd.** `testnet.zec.rocks:443` is reachable, but no official doc listing it was found.
- **Faucet.** `testnet.zecfaucet.com` does **not resolve** (DNS failure, 2026-10-20). The [Valar testnet faucet](https://faucet.testnet.valargroup.dev/) works: it gives 0.125 TAZ per IP per day, and it funded the agent wallet for the verified run.
- **Funds.** Settled. The agent wallet was funded with 0.125 TAZ from the Valar faucet, and a paid run succeeded (see [Verified testnet run](#verified-testnet-run)). The tools create the merchant keys, the receipt key and the agent wallet themselves (see above). `zingo-cli` is not installed and isn't needed: the agent uses `zcash_client_sqlite` directly.
- **Encrypted audit log.** Not built. `X402Z_AGENT_LEDGER` is plaintext JSON.
