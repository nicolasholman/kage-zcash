// Kage agent chat: an LLM research agent with its own shielded ZEC wallet and a spend policy.
// Ask it a question; when it needs paid data it calls kage.fetch(), which pays the API's HTTP 402
// with a real Zcash testnet transaction, inside the policy below. Every step prints as it happens.
import { createInterface } from 'node:readline/promises';
import { stdin, stdout } from 'node:process';
import { createKage, KageError, type ApprovalRequest, type KageEvent } from 'kage-x402/agent';
import { readSeedFile } from 'kage-x402/keys';
import { formatZec } from 'kage-x402';

const MODEL = process.env.CHAT_MODEL ?? 'nvidia/nemotron-3-ultra-550b-a55b:free';
const API = process.env.MARKET_API ?? 'http://127.0.0.1:4300';
if (!process.env.OPENROUTER_API_KEY) {
  console.error('Setup required: OPENROUTER_API_KEY not configured (examples/agent-chat/.env)');
  process.exit(2);
}

// ── terminal styling ───────────────────────────────────────────────────────────────────────────
const c = (n: number) => (s: string) => `\x1b[${n}m${s}\x1b[0m`;
const [dim, bold, gold, green, red, cyan, magenta] = [c(2), c(1), c(33), c(32), c(31), c(36), c(35)];
const step = (icon: string, text: string) => console.log(`   ${icon}  ${text}`);
// **bold** and `code` from the model's markdown, as terminal styles
const md = (t: string) => t.replace(/\*\*(.+?)\*\*/g, (_, x) => bold(x)).replace(/`([^`]+)`/g, (_, x) => cyan(x)).replace(/ +\n/g, '\n');
const short = (tx: string) => `${tx.slice(0, 8)}…${tx.slice(-4)}`;

const rl = createInterface({ input: stdin, output: stdout });

// ── the agent's wallet and policy ──────────────────────────────────────────────────────────────
const { seed, birthday } = readSeedFile(process.env.KAGE_SEED_FILE ?? '../../.kage/agent/seed.env');

const kage = await createKage({
  wallet: { seed, birthday, dir: process.env.KAGE_AGENT_DIR ?? '../../.kage/agent' },
  name: process.env.KAGE_AGENT_NAME ?? 'research-agent',
  policy: {
    dailyCap: process.env.KAGE_DAILY_CAP ?? '0.008 ZEC',                     // hard daily budget
    allowedHosts: [new URL(API).host],                                       // the only API it may pay
    approvalThreshold: process.env.KAGE_APPROVAL_THRESHOLD ?? '0.003 ZEC',   // a human approves anything this big
  },
  onApproval: async (r: ApprovalRequest) => {
    const a = await rl.question(`   ${gold('✋')}  ${bold(`Approve ${r.amount} ZEC to ${r.host}?`)} ${dim(`(policy: approve at ≥ ${formatZec(r.thresholdZat)} ZEC)`)} [y/N] `);
    return /^y(es)?$/i.test(a.trim());
  },
  onEvent: (e: KageEvent) => {
    if (e.type === 'request') step(cyan('→'), `GET ${e.url}`);
    if (e.type === 'approval_requested') step(gold('🛡'), `policy: ${e.request.amount} ZEC is above the approval threshold`);
    if (e.type === 'approved') step(green('✓'), 'approved by you');
    if (e.type === 'broadcast') {
      step(gold('🛡'), 'policy: allowed (host on the allowlist, inside the daily cap)');
      step(magenta('⛓'), `paid ${formatZec(e.amountZat)} ZEC, shielded · tx ${short(e.txid)}`);
      step(dim('↗'), dim(e.explorer));
    }
    if (e.type === 'waiting') step(dim('⏳'), dim(`waiting for the block… ${Math.round(e.elapsedMs / 1000)} s`));
    if (e.type === 'settled') step(green('✅'), `settled: the facilitator saw it with the merchant's viewing key and signed a receipt`);
    if (e.type === 'refused') step(red('⛔'), `${bold(e.code)}: ${e.message}`);
  },
});

// ── tools the model can call ───────────────────────────────────────────────────────────────────
const tools = [
  { type: 'function', function: { name: 'list_paid_apis', description: `List the paid endpoints of the market-data API at ${API} and their prices (free).`, parameters: { type: 'object', properties: {} } } },
  { type: 'function', function: { name: 'buy', description: 'GET a paid URL. Pays its price in shielded ZEC from your wallet if your spend policy allows, then returns the data. Refusals explain why; nothing is paid then.', parameters: { type: 'object', properties: { url: { type: 'string' } }, required: ['url'] } } },
  { type: 'function', function: { name: 'budget', description: 'Your wallet balance and how much of today\'s budget is left.', parameters: { type: 'object', properties: {} } } },
];

async function runTool(name: string, args: { url?: string }): Promise<unknown> {
  if (name === 'list_paid_apis') return (await fetch(`${API}/`)).json();
  if (name === 'budget') {
    const [s, w] = await Promise.all([kage.status(), kage.wallet()]);
    return { spentTodayZec: formatZec(s.spentTodayZat), dailyCapZec: formatZec(s.dailyCapZat), remainingZec: formatZec(s.remainingZat), walletSpendableZec: formatZec(w.spendableZat) };
  }
  if (name === 'buy') {
    try {
      const r = await kage.fetch(String(args.url));
      return { status: r.status, paidZec: r.payment?.amount ?? '0', txid: r.payment?.txid, data: r.body };
    } catch (e) {
      if (e instanceof KageError) return { refused: e.code, reason: e.message, moneyMoved: e.paid };
      throw e;
    }
  }
  return { error: `unknown tool ${name}` };
}

type Msg = { role: string; content: string | null; tool_calls?: { id: string; function: { name: string; arguments: string } }[]; tool_call_id?: string };
const messages: Msg[] = [
  {
    role: 'system',
    content:
      `You are a market research agent with your own shielded Zcash wallet. You can buy data from the API at ${API} (call list_paid_apis to see prices). ` +
      'Buy only what the question needs, then answer in 2-4 short sentences with the numbers, and say what you paid. ' +
      'When the user asks you to get data from any URL, call buy on it: your wallet\'s spend policy, not you, decides what may be paid. ' +
      'If a purchase is refused, tell the user plainly why (budget, approval, or a host you are not allowed to pay) and do not retry it. Never invent data. Plain text, no markdown tables.',
  },
];

async function llm(): Promise<Msg> {
  for (let attempt = 0; ; attempt++) {
    const r = await fetch('https://openrouter.ai/api/v1/chat/completions', {
      method: 'POST',
      headers: { authorization: `Bearer ${process.env.OPENROUTER_API_KEY}`, 'content-type': 'application/json' },
      body: JSON.stringify({ model: MODEL, messages, tools, temperature: 0.2 }),
    });
    const j = await r.json();
    if (j.choices?.[0]?.message) return j.choices[0].message;
    if (attempt >= 2) throw new Error(`model error: ${JSON.stringify(j.error ?? j).slice(0, 200)}`);
    await new Promise((res) => setTimeout(res, 3000));
  }
}

async function turn(question: string) {
  messages.push({ role: 'user', content: question });
  for (let i = 0; i < 8; i++) {
    const m = await llm();
    messages.push({ role: 'assistant', content: m.content ?? null, tool_calls: m.tool_calls });
    if (!m.tool_calls?.length) {
      console.log(`\n${bold(gold('kage-agent'))}  ${md(m.content?.trim() ?? '')}\n`);
      return;
    }
    for (const call of m.tool_calls) {
      const args = JSON.parse(call.function.arguments || '{}');
      console.log(dim(`   ⚙  ${call.function.name}(${args.url ?? ''})`));
      const out = await runTool(call.function.name, args);
      messages.push({ role: 'tool', tool_call_id: call.id, content: JSON.stringify(out) });
    }
  }
}

// ── header ─────────────────────────────────────────────────────────────────────────────────────
const w = await kage.wallet();
const s = await kage.status();
console.log(`\n${bold(gold('影 Kage agent'))}  ${dim(`model ${MODEL}`)}`);
console.log(dim(`   wallet  ${w.address.slice(0, 18)}…  ${formatZec(w.spendableZat)} ZEC spendable (testnet)`));
console.log(dim(`   policy  cap ${formatZec(s.dailyCapZat)} ZEC/day · spent ${formatZec(s.spentTodayZat)} · approve ≥ ${formatZec(s.approvalThresholdZat ?? 0n)} ZEC · pays only ${s.allowedHosts.join(', ')}\n`));

const scripted = process.argv.slice(2);
try {
  if (scripted.length) {
    for (const q of scripted) {
      console.log(`${bold(cyan('you'))} › ${q}`);
      await turn(q);
    }
  } else {
    for (;;) {
      const q = (await rl.question(`${bold(cyan('you'))} › `)).trim();
      if (!q || q === 'exit') break;
      await turn(q);
    }
  }
} finally {
  kage.close();
  rl.close();
}
