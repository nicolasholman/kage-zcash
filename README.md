# Kage — shielded x402: private machine payments for AI agents

HTTP 402 payments for AI agents, settled as **Zcash shielded** testnet transactions. The merchant's facilitator verifies a payment with a viewing key alone (it holds no spend key). Before the agent builds any transaction, **spendguard** checks it against a daily cap, a host allowlist and an approval threshold.

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
| `crates/x402z-facilitator`: lightwalletd gRPC, UFVK trial decryption (Sapling, Orchard, Ironwood), receipt signing | **Done.** 5 integration tests on real shielded transactions (real keys, real Halo2 proofs, real ed25519). Verified live against testnet.zec.rocks |
| `crates/x402z-agent`: 402 → spendguard → `zcash_client_sqlite` wallet → pay → receipt → retry, ledger writer | **Done.** 4 ledger tests. Paid live on testnet |
| Encrypted audit log (only the owner's viewing key can read it) | Not started. The ledger is plain JSON |
| Paid end-to-end runs on testnet | **Done.** [`030c3cbf…8351`](https://testnet.zecblock.com/tx/030c3cbf78ce8460553bba4e82c0ba813f039bd3ecf4d7f2e81c59837dc28351) (block 4521112) and the demo-video run [`cd64980a…916d`](https://testnet.zecblock.com/tx/cd64980a5247881a727aa916476a3ba757e59794263c2ff1bcde467654a0916d) (block 4521440); see [Verified testnet runs](#verified-testnet-runs) |

Nothing is mocked. If a binary is missing a key, a wallet or funds, it stops with an error and a non-zero exit code. It never fakes a payment.

## Do X → see Y

1. Run `cargo test --workspace`. You should see 34 tests pass: spendguard 11, protocol 4, server 10, agent 4, facilitator 5. The facilitator tests build real shielded transactions with Halo2 proofs and take about 30 s.
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
  - `422 rejected`: no matching output, wrong recipient, underpaid, foreign `payTo`, a `payTo` that carries a transparent receiver (`BadPayTo`; only shielded receivers are accepted), or a nonce already receipted for another tx.
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

## Running a paid payment yourself

Earlier unfunded runs (health, rejections of foreign txs, exit codes 3/4) are logged in [docs/build-notes.md](docs/build-notes.md).

**What a paid run needs** (all three were in place for the [verified runs](#verified-testnet-runs)):

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

## Verified testnet runs

Two payments went end to end on 2026-10-20 against `https://testnet.zec.rocks:443`, with nothing mocked. Both pay 100,000 zat as an Ironwood note to the same demo merchant wallet:

| Txid | Block | Memo nonce | Notes |
|---|---|---|---|
| [`030c3cbf78ce8460553bba4e82c0ba813f039bd3ecf4d7f2e81c59837dc28351`](https://testnet.zecblock.com/tx/030c3cbf78ce8460553bba4e82c0ba813f039bd3ecf4d7f2e81c59837dc28351) | 4521112 | `x402z-f19a63da20cbc0749ea800b3` | first paid run, details below |
| [`cd64980a5247881a727aa916476a3ba757e59794263c2ff1bcde467654a0916d`](https://testnet.zecblock.com/tx/cd64980a5247881a727aa916476a3ba757e59794263c2ff1bcde467654a0916d) | 4521440 | `x402z-af06b1d50af81616075f0cb3` | the run shown in the demo video |

First run (11:14 IST):

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

The demo-video payment checks the same way (`cd64980a5247881a727aa916476a3ba757e59794263c2ff1bcde467654a0916d x402z-af06b1d50af81616075f0cb3` → 100,000 zat). With a different memo nonce it prints `0 zat` and exits 2. If no output decrypts, it exits 1.

## Known limits

- **Pre-release Zcash crates.** NU7 is live on testnet and only the pre-release librustzcash line parses its transactions; see [docs/build-notes.md](docs/build-notes.md) for the pinned versions and toolchain.
- **Encrypted audit log.** Not built. `X402Z_AGENT_LEDGER` is plaintext JSON.
- **First payment downloads the Sapling params** (~50 MB). A truncated download fails the hash check before anything is sent; delete the partial file and retry ([build notes](docs/build-notes.md#gotcha-truncated-sapling-params)).
