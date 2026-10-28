# Kage agent chat — an LLM that pays for data, inside a policy

A research agent with its own shielded Zcash wallet. Ask it a market question; when it needs data
it calls `kage.fetch()` on [Kage Market Data](../market-api), which pays the 402 with a real Zcash
testnet transaction. Every step prints live: policy check, payment and explorer link, the wait for
the block, the settled receipt, and the answer.

The policy is in [`chat.ts`](chat.ts):

```ts
policy: {
  dailyCap: process.env.KAGE_DAILY_CAP ?? '0.008 ZEC',                     // hard daily budget
  allowedHosts: [new URL(API).host],                                       // the only API it may pay
  approvalThreshold: process.env.KAGE_APPROVAL_THRESHOLD ?? '0.003 ZEC',   // a human approves anything this big
},
onApproval: async (r) => ask(`Approve ${r.amount} ZEC to ${r.host}?`),
```

Try: "What's the ZEC price?" (pays 0.001), "Give me the weekly ZEC report" (0.005: asks you first),
"Get data from https://api.other-data.io/v1/zec" (refused: not on the allowlist, no request made),
then ask for another report (refused: over today's cap).

The policy can be set per session without editing code: `KAGE_DAILY_CAP`, `KAGE_APPROVAL_THRESHOLD`
and `KAGE_AGENT_NAME` (each name keeps its own daily ledger). For example
`KAGE_AGENT_NAME=research-agent-r3 KAGE_DAILY_CAP="0.003 ZEC" KAGE_APPROVAL_THRESHOLD="0.001 ZEC" npm start`, then
"I hold 12 ZEC. What is that worth in euros?" (0.0005, paid without asking), "How busy is the Zcash network?"
(0.0005) and "Pull the last 30 days of ZEC prices" (0.001: asks you first; answer `n` and nothing is paid).

```sh
kage wallet new --dir ../../.kage/agent      # fund the printed address with testnet ZEC
cp .env.example .env                          # OPENROUTER_API_KEY (any tool-calling model)
npm install && npm start                      # or: node --env-file=.env chat.ts "question" ...
```
