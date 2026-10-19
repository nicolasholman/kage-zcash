# Shielded x402 — private machine payments for AI agents (ZECATHON)

## Goal & target track
ZECATHON Shielded Payments (alt: Core & Tooling). DEADLINE Oct 28 2026 (time and timezone not public, so treat it as Oct 27 EOD IST). Prizes: $15K track, $20K grand. Code freeze Oct 26, then video and README on Oct 27.

## Fact check (do not overclaim)
The $42K ZCG grant 'Shielded x402' (baby-zack-agent) EXISTS but was REJECTED Sep 28 2026 (openzcash.org/zcg/decisions). Do not claim it's funded. Pitch it as: 'the grant the ecosystem asked for but didn't fund — we shipped it.' Do not copy their code; build our own.

## Honest feasibility
High. x402 is HTTP 402 + payment header; settlement verification = scanning a shielded payment to a merchant address with its viewing key and matching the memo nonce. Latency is the honest weakness (~75s block time) — design an 'accept on mempool/1 conf' policy and say so.

## Scoped MVP (1 Claude CLI agent, ~1-2 days)
1. `x402z-server` (Rust axum or TS express): paywalled `/api/quote` returns 402 with JSON `{scheme:"zcash-shielded", network:"testnet", payTo:<UA>, amountZat, memoNonce, expiresAt}`.
2. `x402z-facilitator`: holds merchant UFVK only (no spend key), syncs via lightwalletd, confirms a note with matching memo nonce + amount, issues signed receipt (ed25519) -> client retries with `X-PAYMENT-RECEIPT`.
3. `x402z-agent` client lib + demo agent: on 402 pays with zingo-cli/zcash_client_backend, then retries.
4. `spendguard`: per-agent daily cap, per-host allowlist, approval threshold (prompt over CLI), append-only audit log encrypted so only the owner's viewing key holder can read it.
5. Small Next.js dashboard: agent spend vs cap, audit trail, receipts.

## Stack & install
```sh
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
sudo apt-get install -y protobuf-compiler build-essential pkg-config libssl-dev
# Zcash wallet libs (librustzcash): zcash_client_backend, zcash_client_sqlite, zcash_keys, zcash_primitives, zcash_protocol
cargo add zcash_client_backend zcash_client_sqlite zcash_keys zcash_primitives zcash_protocol --features zcash_client_backend/lightwalletd-tonic
# CLI wallet alternative: zingolib / zingo-cli
git clone https://github.com/zingolabs/zingolib && cd zingolib && cargo build --release -p zingo-cli
# testnet: public lightwalletd (verify current host in zcash docs), e.g. https://testnet.zec.rocks:443
# testnet faucet: https://testnet.zecfaucet.com (verify live)
```
Check versions on crates.io before pinning; librustzcash crates move fast (Ironwood/NU6.3 pool is live since Jul 28 2026 — Orchard is sealed, use the pool the current crates support).

## Architecture
agent -> GET /api/quote -> 402 -> spendguard check -> shielded tx (memo=nonce) -> facilitator scans with UFVK via lightwalletd -> receipt -> agent retries -> 200.

## Milestones & acceptance tests
- M1 spec doc SPEC.md mapping x402 fields to Zcash. AT: spec reviewed, JSON schema validates sample.
- M2 402 server + facilitator on testnet. AT: real testnet payment unlocks endpoint; wrong amount/expired nonce rejected (integration test against live lightwalletd).
- M3 agent client + spendguard. AT: cap exceeded => payment refused with no tx broadcast; allowlist enforced.
- M4 dashboard + README + 2-min video.

## Demo script (≤2 min)
Agent asks for paid data -> 402 -> pays 0.001 TAZ shielded -> facilitator confirms -> data returned. Then agent tries to exceed cap -> blocked, audit log shows it. Show block explorer: amounts/addresses hidden.

## Hard rules
- NO mocked APIs, stubbed responses, or fake chain data. Zcash testnet + NEAR testnet only, real endpoints.
- Missing key/endpoint/wallet => fail loudly: exit non-zero in CLI, red banner in UI naming the missing var (e.g. `ZCASH_LWD_URL not configured`).
- Secrets only in `.env` (gitignored); ship `.env.example`.
- One project at a time; `cargo clean` / delete `node_modules` and `target/` of finished projects to free disk.
- Do not push, make repos public, deploy to mainnet, or sign mainnet txs without Nick's explicit OK.
- README written for AI judges: "do X -> see Y" steps.
