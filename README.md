# Kage — shielded x402: private machine payments for AI agents

HTTP 402 payments for AI agents, settled as **Zcash shielded** testnet transactions, in the **x402 v1 wire format**.
- **Merchant side.** The facilitator verifies a payment with the merchant's **incoming viewing key** alone. It holds no spend key, and it can't see the merchant's own spends or change.
- **Agent side.** Before the agent builds any transaction, **spendguard** checks it against a daily cap, a host allowlist and an approval threshold.
- **Any wallet can pay.** Each 402 carries a ZIP 321 URI and a QR code.
- **LLM agents** call the same flow through an MCP server.

```
agent -> GET /api/quote
      <- 402 {x402Version: 1, accepts: [{scheme: "zcash-shielded", network: "zcash-testnet",
              maxAmountRequired, payTo, asset: "ZEC", extra: {memoNonce, expiresAt, paymentUri}}]}
      -> spendguard.check() -> shielded tx from the agent's own wallet (memo = nonce)
      -> GET /api/quote  X-PAYMENT: base64 {x402Version, scheme, network, payload: {txid, memoNonce}}
         server -> facilitator POST /settle -> lightwalletd -> trial-decrypt with the merchant UIVK
                -> ed25519 receipt (minedAt) -> server checks signature, burns the nonce (SQLite)
      <- 200 + X-PAYMENT-RESPONSE: base64 {success, transaction, network, payer: ""}
```

## Status (honest)

| Part | State |
|---|---|
| `crates/spendguard`: cap, allowlist, approval threshold | **Done.** 11 unit tests |
| `crates/x402z-protocol`: x402 v1 types (`PaymentRequiredResponse`, `PaymentRequirements`, `X-PAYMENT`, `X-PAYMENT-RESPONSE`, `/settle`, `/supported`), challenge validation, ZIP 321 URIs | **Done.** 13 tests, including the x402 spec's own 402 and payload examples, and ZIP 321's memo example |
| `crates/x402z-server`: paywalled `/api/quote`, `X-PAYMENT` settlement via the facilitator, `POST /api/redeem` (paste a txid), QR, dashboard, SQLite challenge book, per-IP rate limit | **Done.** 21 tests |
| `crates/x402z-facilitator`: lightwalletd gRPC, UIVK trial decryption (Sapling, Orchard, Ironwood), mined-block-time expiry, SQLite nonce bindings, `/verify` `/settle` `/supported` | **Done.** 13 tests. 6 of them build transactions in-process with real keys, real note encryption and real Halo2 proofs; these are **not broadcast**. 3 verify the four real Kage payments live against testnet.zec.rocks with only the published UIVK |
| `crates/x402z-agent`: 402 → spendguard → `zcash_client_sqlite` wallet → pay → `X-PAYMENT` retry, file-locked ledger | **Done.** 8 tests. Paid live on testnet (Oct 20) |
| `crates/x402z-mcp`: MCP server over stdio, with tools `paid_fetch`, `spend_status`, `ledger_tail` | **Done.** 3 stdio tests. Live LLM run via opencode ([transcript](docs/mcp-live-run.txt)) |
| Encrypted audit log (only the owner's viewing key can read it) | Not started. The ledger is plain JSON |
| Paid end-to-end runs on testnet | **Done.** Four txids, two of them on the current code; see [Verified testnet runs](#verified-testnet-runs) |

Nothing is mocked. If a binary is missing a key, a wallet or funds, it stops with an error and a non-zero exit code. It never fakes a payment. Test counts and what each test does and doesn't prove are in [TESTING.md](TESTING.md).

## What still leaks

Shielding hides the amount, the memo and both parties from the chain. It doesn't hide everything:
- **The merchant sees the client's IP.** The paid request is ordinary HTTP. Use Tor or a proxy if that matters.
- **lightwalletd sees which txid is looked up**, and when. The payer's wallet broadcasts through it, and the facilitator fetches the tx by id, so the operator can link the two lookups to the merchant's facilitator.
- **The agent ledger is local plaintext JSON** (`X402Z_AGENT_LEDGER`): hosts, amounts, nonces and txids. It is not encrypted yet.
- **Timing.** A payment lands in the block after the 402 was issued, so someone watching both the HTTP traffic and the chain can correlate the two by time, even though the transaction itself reveals nothing.

## Do X → see Y

1. Run the fast tests one crate at a time (low-RAM friendly):
   ```sh
   cargo test -p spendguard -p x402z-protocol -p x402z-server   # 11 + 13 + 21
   cargo test -p x402z-agent                                      # 8
   cargo test -p x402z-mcp                                        # 3 (drives the binary over stdio)
   cargo test -p x402z-facilitator                                # 13: 6 prove Halo2 bundles (~40 s), 3 hit testnet live
   ```
   That makes 69 tests in all (facilitator 13 includes 3 live testnet tests that need network).
2. Run `cargo run -p x402z-facilitator` with no `.env`. It prints `error: X402Z_MERCHANT_UIVK not configured` and exits with code 2.
   - If `ZCASH_LWD_URL` is unset, it says so and uses `https://testnet.zec.rocks:443`.
   - If lightwalletd is unreachable, it prints `error: cannot connect to lightwalletd: …` and exits with code 1.
3. Run `cargo run -p x402z-facilitator -- keygen`. It prints a fresh `X402Z_RECEIPT_SIGNING_KEY` / `X402Z_RECEIPT_PUBKEY` pair.
4. Run `X402Z_AGENT_WALLET_DIR=./wallets/merchant cargo run -p x402z-agent -- new-wallet`. It creates a real testnet wallet and prints its seed phrase, birthday height, shielded address and incoming viewing key (`X402Z_MERCHANT_UIVK`). Use one such wallet for the merchant (UA + UIVK) and another for the agent.
5. Run the agent. Each exit code means one thing:

   | Situation | Output | Exit |
   |---|---|---|
   | Cap below the price, or host not in `X402Z_AGENT_ALLOWED_HOSTS` | `spendguard refused the payment: … No transaction was built or broadcast.` | 3 |
   | No `X402Z_AGENT_SEED` | `X402Z_AGENT_SEED not configured: no funded testnet wallet to pay from …` | 4 |
   | Wallet synced but holding too little TAZ | `insufficient testnet balance: spendable 0 zat …, need at least 110000 zat … Send TAZ to utest1…` | 4 |
   | Server/facilitator refuses the payment | `server refused payment tx … (402 Payment Required, invalid_payment): …` | 5 |
   | Paid | the paid JSON on stdout, `X-PAYMENT-RESPONSE: success=true …` on stderr | 0 |

   Every decision is appended to `X402Z_AGENT_LEDGER`, and the dashboard shows it. Two agent runs at once take turns on `<ledger>.lock`, so they can't both pass the cap.
6. Send more than `X402Z_CHALLENGE_RATE_PER_MIN` requests (default 30) to `/api/quote` from one IP. You get `429 Too Many Requests` with a `Retry-After` header. A request that redeems a valid receipt is never rate limited.

## x402 wire format

Kage follows coinbase/x402 [`specs/x402-specification-v1.md`](https://github.com/coinbase/x402/blob/main/specs/x402-specification-v1.md) and [`specs/transports-v1/http.md`](https://github.com/coinbase/x402/blob/main/specs/transports-v1/http.md) (read 2026-10-21). A v2 spec also exists, with CAIP-2 networks and `PAYMENT-SIGNATURE` / `PAYMENT-REQUIRED` headers. Kage implements v1.

- **402 body.** `{x402Version: 1, accepts: [PaymentRequirements], error}`. The Zcash entry is: `scheme: "zcash-shielded"`, `network: "zcash-testnet"`, `maxAmountRequired` (zatoshi, as a string), `resource`, `description`, `mimeType`, `payTo` (a shielded-only UA), `maxTimeoutSeconds: 900`, `asset: "ZEC"`, and `extra: {memoNonce, expiresAt, paymentUri}`.
- **`X-PAYMENT`.** Base64 JSON `{x402Version: 1, scheme, network, payload: {txid, memoNonce}}`. The payload adds `memoNonce` to the txid for a reason: a shielded transaction gives the server nothing to look up, so the payer names the challenge it paid. The facilitator then finds that same nonce inside the encrypted memo, so a payment can't be pointed at a different challenge.
- **`X-PAYMENT-RESPONSE`.** Base64 JSON `{success, errorReason?, transaction, network, payer}`. `payer` is always `""`: nobody, the merchant included, learns the payer's address. A payment that isn't confirmed yet gets `402` with the *same* challenge and `errorReason: "settlement_pending"`, so the client retries the same `X-PAYMENT`. Other reasons are `invalid_payment`, `payment_expired`, `nonce_already_used`, `unknown_challenge`, `invalid_payload`, `invalid_network`, `invalid_scheme` and `facilitator_error` (that last one comes with a 502, and nothing is burned).
- **Facilitator endpoints.** `GET /supported` returns `{kinds: [{x402Version: 1, scheme: "zcash-shielded", network: "zcash-testnet"}]}`. `POST /verify` and `POST /settle` take `{x402Version, paymentPayload, paymentRequirements}`. `/verify` is read-only and returns `{isValid, invalidReason, payer}`. `/settle` binds the nonce and returns the settlement fields plus Kage's ed25519 `receipt` and `claims`. The resource server trusts only receipts signed by `X402Z_RECEIPT_PUBKEY`.
- **Back-compat.** `X-PAYMENT-RECEIPT` (a receipt presented directly) and the legacy `/verify {challenge, txid}` body still work.

## Any wallet can pay (ZIP 321)

`extra.paymentUri` is a [ZIP 321](https://zips.z.cash/zip-0321) URI: `zcash:<ua>?amount=0.001&memo=<base64url(nonce), no padding>`. librustzcash's own ZIP 321 parser reads it back to the same address, amount and memo (test `zip321_uri`).

The dashboard's 402 card shows:
- the QR code, rendered by the server at `/api/challenge/{nonce}/qr.svg`, with no external service involved;
- the URI, with **copy** and **Open in wallet** buttons;
- a **paste txid** box.

The paste box calls `POST /api/redeem {memoNonce, txid}`. That runs the same checks as `X-PAYMENT` and answers `200 paid` (with the content), `202 pending` (the page re-checks every 15 s), `422 rejected` or `502`. This path was run live on 2026-10-21 with zingo-cli as the third-party wallet (tx `ccb4214c…0ed3`, below).

## Durable state and expiry (no money lost on a slow block)

- **Server.** Issued challenges and redeemed nonces live in SQLite (`X402Z_SERVER_DB`, default `x402z-server.sqlite`). A restart between the 402 and the redeem keeps the challenge redeemable, and a redeemed nonce stays burned. Unredeemed challenges are kept for `X402Z_REDEEM_WINDOW_SECS` (default 24 h) after expiry.
- **Facilitator.** Nonce → txid bindings live in SQLite (`X402Z_FACILITATOR_DB`). After a restart, a nonce still can't be receipted for a second transaction.
- **Expiry is judged by the mined block's header time, not by when the payer asks.** The facilitator reads the block time with `GetBlock` and puts it in the receipt as `minedAt`. The server accepts a receipt when `minedAt <= expiresAt + X402Z_EXPIRY_GRACE_SECS` (default 1200 s, about 16 blocks), even if the receipt is redeemed later. Block header times can be off by a little (consensus allows some miner clock drift), and the grace absorbs that. A mempool-only receipt has no `minedAt`, so it must be redeemed in time.

## MCP server (LLM agents)

`x402z-mcp` speaks MCP over stdio as newline-delimited JSON-RPC 2.0, protocol `2025-06-18` (it also accepts `2025-03-26` and `2024-11-05`). It uses the agent's configuration (`X402Z_AGENT_*`).

| Tool | Does |
|---|---|
| `paid_fetch {url}` | Runs the agent's x402 flow. Spendguard refusals (`spendguard_refused`), wallet problems (`wallet`), rejected payments (`payment_rejected`) and missing config (`setup_required`) come back as `isError: true` tool results with the reason. Payments at or above the approval threshold are always declined, because there's no operator to approve them over MCP |
| `spend_status {}` | Spent today vs the daily cap, remaining budget, allowlist and threshold |
| `ledger_tail {n}` | The last n spendguard decisions |

Example client config (opencode `opencode.json`; other MCP clients take the same command and env):
```json
{ "mcp": { "kage": { "type": "local", "command": ["/path/to/target/release/x402z-mcp"],
  "environment": { "X402Z_AGENT_LEDGER": "/path/ledger.json", "X402Z_AGENT_WALLET_DIR": "/path/wallet",
                   "X402Z_AGENT_SEED": "…", "X402Z_AGENT_BIRTHDAY": "…",
                   "X402Z_AGENT_ALLOWED_HOSTS": "127.0.0.1", "X402Z_AGENT_DAILY_CAP_ZAT": "500000" } } } }
```
Live run (2026-10-21, opencode with `openrouter/minimax/minimax-m3.1-flash:free`, cap set to 50,000 zat against the real server's 100,000 zat 402):
- the model called `spend_status`, then `paid_fetch` on `http://127.0.0.1:4020/api/quote`;
- `paid_fetch` came back as a tool error: `spendguard refused the payment: daily cap exceeded … No transaction was built or broadcast.`;
- `ledger_tail` then showed the `denied` entry.

The full transcript is in [docs/mcp-live-run.txt](docs/mcp-live-run.txt). A *paying* MCP run needs a funded agent wallet in `X402Z_AGENT_SEED`; it uses the same `paid_fetch` code path the CLI agent used for the paid runs below.

## Facilitator key: incoming viewing key by default

The facilitator reads `X402Z_MERCHANT_UIVK` (`uivktest1…`), the merchant's unified *incoming* viewing key. That is the least key that can see a payment: it decrypts notes sent to the merchant's external addresses, and nothing else. It cannot see the merchant's spends, change or outgoing payments, and it cannot spend.

`X402Z_MERCHANT_UFVK` (a full viewing key) still works for older setups, but the facilitator logs a warning at startup, keeps only the incoming part, and `/health` reports `viewingKey: "ufvk (incoming part only…)"`. Get the UIVK from a UFVK with `X402Z_MERCHANT_UFVK=… x402z-facilitator derive-uivk`.

Tests run every real-transaction case through the UIVK (decoded from its string) and through the UFVK-reduced key, and the two must agree. A separate test builds the merchant's own change and outgoing outputs, with the open challenge's nonce in every memo. The full key sees all four outputs. The incoming key sees none, so none of them can be credited.

## How the facilitator decides

For `/settle` (and `/verify`) the facilitator:
1. fetches the tx with `GetTransaction`;
2. parses it at the consensus branch of its mined height;
3. trial-decrypts every Sapling, Orchard and Ironwood output with the merchant's incoming viewing key;
4. sums the outputs whose text memo equals `memoNonce` and whose recipient is `payTo`'s receiver for that pool. Sapling notes must go to the Sapling receiver; Orchard and Ironwood notes must go to the Orchard receiver;
5. reads the mined block's time and checks it against `expiresAt + grace`.

It refuses on any of these:
- no matching output;
- wrong recipient;
- underpaid;
- paid after expiry;
- foreign `payTo`;
- a `payTo` with a transparent receiver (only shielded receivers are accepted);
- a nonce already receipted for another tx.

`GET /health` makes a live lightwalletd round trip.

## Dashboard (served by x402z-server)

Fill in `X402Z_MERCHANT_UA`, `X402Z_RECEIPT_PUBKEY` and `X402Z_FACILITATOR_URL`, run `cargo run -p x402z-server`, then open http://127.0.0.1:4020.

- Press **Request paid data →** and within a second you get a live x402 challenge: QR, ZIP 321 URI, payTo, amount, memo nonce and a 15-minute countdown. The event also lands in the **Live payment feed**.
- Pay it from any wallet and paste the txid. Or let an agent send `X-PAYMENT`. While the payment confirms, the feed shows `confirming`; once it settles, it shows `paid` with the txid.
- A bad, replayed, underpaid or late payment shows up as `refused` and comes back with a fresh 402 and an `x-payment-error` header.
- **The spendguard panel is a local demo:** the payer's ledger and the merchant's dashboard sit on one machine. It reads `X402Z_AGENT_LEDGER`, a JSON file `{agents:[{agent,dailyCapZat,spentTodayZat,approvalThresholdZat,allowedHosts}], audit:[{ts,agent,host,amountZat,decision,reason,memoNonce,txid}]}`. In a real deployment the merchant never sees the agent's ledger. If the file is unset or unreadable, the page shows a red banner naming the variable.
- Abuse limit: each client IP gets a token bucket of `X402Z_CHALLENGE_RATE_PER_MIN` challenges and redeem attempts (default 30; `0` turns it off).
- The layout collapses to one column on narrow screens and works at 375 px (`X402Z_BIND=0.0.0.0:4020`).

## spendguard rules

These are checked in order. The first denial wins, so a payment that would be denied never reaches an approval prompt.

1. **Host allowlist.** Hosts are denied by default. An entry is an exact host or a `*.suffix` wildcard; `*.data.test` matches `x.data.test` but not `data.test` or `evildata.test`. Host matching ignores case, ports and trailing dots.
2. **Amount must be > 0.**
3. **Daily cap** per agent, measured per UTC day. The cap is inclusive, and sums use checked arithmetic so an overflow counts as a denial.
4. **Approval threshold** (inclusive): at or above it, the result is `NeedsApproval`.

`record_spend` re-runs the check before it updates the ledger. Call it only after a broadcast.

## Latency policy

Testnet blocks arrive about every 75 s, sometimes much slower. By default both the facilitator and the server require 1 confirmation (`X402Z_MIN_CONFIRMATIONS`). Below that, `X-PAYMENT` gets `402 settlement_pending` with the same challenge. The agent retries every 15 s, for up to `X402Z_AGENT_CONFIRM_TIMEOUT_SECS` (default 900). Setting `X402Z_MIN_CONFIRMATIONS=0` on both accepts mempool-only payments; that is weaker, and the server logs a warning.

## Running a paid payment yourself

Earlier unfunded runs (health, rejections of foreign txs, exit codes 3/4) are logged in [docs/build-notes.md](docs/build-notes.md).

**What a paid run needs:**

1. **A funded testnet wallet for the agent.** Put its 24-word BIP 39 phrase in `X402Z_AGENT_SEED` and its birthday height in `X402Z_AGENT_BIRTHDAY`. It needs at least 0.0011 TAZ spendable (price plus fee); more allows repeated runs. Alternatively, run `x402z-agent new-wallet` and send TAZ to the address it prints. Or skip the agent and pay the dashboard's QR from any wallet.
2. **A merchant UA and UIVK.** Any testnet wallet works, as long as the UA is derived from the same key. Run `new-wallet` into a second directory: it prints the UA and UIVK. You only need its seed later, to spend the income.
3. **A receipt key pair**, from `x402z-facilitator keygen`.

**Commands.** Put every variable from `.env.example` in one `.env`, then:
```sh
set -a; . ./.env; set +a
cargo build --workspace
target/debug/x402z-facilitator &      # logs "lightwalletd ... tip=<height>", listens on 127.0.0.1:4021
target/debug/x402z-server &           # dashboard on http://127.0.0.1:4020
target/debug/x402z-agent balance      # syncs the agent wallet: spendable must be >= 110000 zat
target/debug/x402z-agent              # 402 -> spendguard -> pay -> X-PAYMENT until settled (~1 block) -> 200 JSON on stdout
X402Z_AGENT_DAILY_CAP_ZAT=1 target/debug/x402z-agent   # cap demo: exit 3, nothing broadcast
```
The first payment downloads the Sapling proving parameters to the default Zcash params folder (about 50 MB, once).

## Verified testnet runs

Four payments went end to end against `https://testnet.zec.rocks:443`, with nothing mocked. Each pays 100,000 zat as an Ironwood note to the same demo merchant wallet. The Oct 20 runs used the agent's own wallet and the earlier receipt header. The Oct 21 runs used the current code: x402 v1 wire format, a facilitator holding only the published UIVK, and SQLite state. Both Oct 21 payments came from a separate zingo-cli demo wallet, not the agent's.

| Txid | Block | Memo nonce | Path |
|---|---|---|---|
| [`030c3cbf78ce8460553bba4e82c0ba813f039bd3ecf4d7f2e81c59837dc28351`](https://testnet.zecblock.com/tx/030c3cbf78ce8460553bba4e82c0ba813f039bd3ecf4d7f2e81c59837dc28351) | 4521112 | `x402z-f19a63da20cbc0749ea800b3` | first paid run, details below |
| [`cd64980a5247881a727aa916476a3ba757e59794263c2ff1bcde467654a0916d`](https://testnet.zecblock.com/tx/cd64980a5247881a727aa916476a3ba757e59794263c2ff1bcde467654a0916d) | 4521440 | `x402z-af06b1d50af81616075f0cb3` | the run shown in the demo video |
| [`ccb4214c6a1fdf183c086ae5b4f559d181ead4c1c61dc7e711fc4a08b95b0ed3`](https://testnet.zecblock.com/tx/ccb4214c6a1fdf183c086ae5b4f559d181ead4c1c61dc7e711fc4a08b95b0ed3) | 4524586 | `x402z-b7a15cc628bb7a06bf9a26cd` | 2026-10-21. **Any wallet:** the dashboard's 402 card showed the ZIP 321 URI and QR. zingo-cli paid the URI's address, amount and memo. The txid was pasted into the dashboard, and `POST /api/redeem` answered `paid`. A second redeem answered `nonce_already_used` |
| [`25a8b324bebee30220382113c6fb9cc3be1144b036117c34778bde3b1d8b689d`](https://testnet.zecblock.com/tx/25a8b324bebee30220382113c6fb9cc3be1144b036117c34778bde3b1d8b689d) | 4524602 | `x402z-696c58afd509cc0f8753f3b0` | 2026-10-21. **x402 `X-PAYMENT`:** the client retried `GET /api/quote` with `X-PAYMENT` every 20 s. It got `402` + `X-PAYMENT-RESPONSE {"success":false,"errorReason":"settlement_pending"}` until the block landed, then `200` + `{"success":true,"transaction":"25a8b324…","network":"zcash-testnet","payer":""}`. The receipt carried `minedAt 1792562282` |

First run (2026-10-20, 11:14 IST):

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

The other three check the same way, each showing 100,000 zat with its nonce:
- `cd64980a5247881a727aa916476a3ba757e59794263c2ff1bcde467654a0916d x402z-af06b1d50af81616075f0cb3`
- `ccb4214c6a1fdf183c086ae5b4f559d181ead4c1c61dc7e711fc4a08b95b0ed3 x402z-b7a15cc628bb7a06bf9a26cd`
- `25a8b324bebee30220382113c6fb9cc3be1144b036117c34778bde3b1d8b689d x402z-696c58afd509cc0f8753f3b0`

Because the UIVK is published, anyone can run the merchant side of the Oct 21 runs. `check_uivk address "$uivk"` prints the merchant UA, and `x402z-facilitator keygen` gives a fresh receipt key. With a different memo nonce it prints `0 zat` and exits 2. If no output decrypts, it exits 1.

## Known limits

- **Pre-release Zcash crates.** NU7 is live on testnet, and only the pre-release librustzcash line parses its transactions. See [docs/build-notes.md](docs/build-notes.md) for the pinned versions and toolchain.
- **Encrypted audit log.** Not built. `X402Z_AGENT_LEDGER` is plaintext JSON (see [What still leaks](#what-still-leaks)).
- **Testnet only.** Mainnet challenges are refused by the protocol crate.
- **First payment downloads the Sapling params** (~50 MB). A truncated download fails the hash check before anything is sent. Delete the partial file and retry ([build notes](docs/build-notes.md#gotcha-truncated-sapling-params)).

## License

MIT, Copyright (c) 2026 Nivesh Gajengi. See [LICENSE](LICENSE).
