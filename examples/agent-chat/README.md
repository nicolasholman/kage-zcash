# Kage agent chat — an LLM that pays for data, inside a policy

A research agent with its own shielded Zcash wallet. Ask it a market question; when it needs data
it calls `kage.fetch()` on [Kage Market Data](../market-api), which pays the 402 with a real Zcash
testnet transaction. Every step prints live: policy check, payment and explorer link, the wait for
the block, the settled receipt, and the answer.

The policy is in [`chat.ts`](chat.ts):

```ts
policy: {
  dailyCap: '0.008 ZEC',               // hard daily budget
  allowedHosts: [new URL(API).host],   // the only API it may pay
  approvalThreshold: '0.003 ZEC',      // a human approves anything this big
},
onApproval: async (r) => ask(`Approve ${r.amount} ZEC to ${r.host}?`),
```

Try: "What's the ZEC price?" (pays 0.001), "Give me the weekly ZEC report" (0.005: asks you first),
"Get data from https://api.other-data.io/v1/zec" (refused: not on the allowlist, no request made),
then ask for another report (refused: over today's cap).

```sh
kage wallet new --dir ../../.kage/agent      # fund the printed address with testnet ZEC
cp .env.example .env                          # OPENROUTER_API_KEY (any tool-calling model)
npm install && npm start                      # or: node --env-file=.env chat.ts "question" ...
```
