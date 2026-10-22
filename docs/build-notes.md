# Build notes

History and toolchain details that used to sit in the README. None of it is needed to run Kage.

## Background

A "Shielded x402" grant application was posted to Zcash Community Grants on 2026-09-25 by the forum user baby-zack-agent (forum topic 57826) and was rejected on 2026-09-28. It is another applicant's proposal, not this author's. Kage is an independent implementation by Nivesh Gajengi and shares none of that proposal's code.

## NU7 and the pre-release crate line

- NU7 activated on testnet at height 4465026 (branch id `0x77190AD9`).
- The stable crates (`zcash_protocol` 0.10.6, `zcash_primitives` 0.30, `zcash_client_backend` 0.24) reject every current testnet transaction with `invalid consensus branch id 0x77190ad9`.
- So the workspace pins the pre-release line: `zcash_protocol 0.11.0-pre.1`, `zcash_primitives 0.31.0-pre.2`, `zcash_client_backend 0.25.0-pre.2`, `zcash_client_sqlite 0.23.0-pre.2`, `zcash_keys 0.17.0-pre.2`. Check crates.io for stable releases before relying on these.
- These crates need Rust 1.88 or later, so `rust-toolchain.toml` pins `1.89.0` (it started at 1.85).

## Shielded pool

The crates trial-decrypt Ironwood bundles with the Orchard viewing key, and a UA's Orchard receiver also receives in Ironwood. The agent sends change to Ironwood. The facilitator accepts Sapling, Orchard and Ironwood outputs.

## Infrastructure

- **lightwalletd.** `testnet.zec.rocks:443` (ECC LightWalletD v0.5.4, chain `test`) was reachable throughout; no official doc listing it was found.
- **Faucet.** `testnet.zecfaucet.com` did not resolve (DNS failure, 2026-10-20). The [Valar testnet faucet](https://faucet.testnet.valargroup.dev/) worked: 0.125 TAZ per IP per day. It funded the agent wallet for the verified runs.
- **Wallet.** The agent uses `zcash_client_sqlite` directly; no external wallet binary is needed.

## Gotcha: truncated Sapling params

The first paid attempt failed with `sapling-spend.params failed validation`: the ~50 MB download was cut off (5,982,770 of 47,958,396 bytes arrived), so the hash check rejected it and no payment was sent. Fix: delete the partial `sapling-spend.params` from the Zcash params folder and download it again.

## spendguard origin

spendguard was written from the project spec rather than ported from an earlier TypeScript policy engine; its rules are documented in the README.

## First live run (2026-10-20, before funding)

- `GET /health` returned `chainTip 4520789` (and `4520801` later).
- `POST /verify` with txids of real mined NU7 Ironwood transactions (blocks 4520784–4520789) were fetched, parsed, txid-checked and trial-decrypted, and answered `422 rejected: no output in this transaction decrypts to the merchant viewing key`, which is correct: they don't pay the merchant.
- An unknown txid answered `404 not_found`.
- The agent with no seed exited 4; with a fresh unfunded wallet (birthday 4520790) it synced, reported `spendable 0 zat`, printed the address to fund and exited 4; with the cap or allowlist violated it exited 3 without touching the wallet.
