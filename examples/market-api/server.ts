// A paid API in ~140 lines: live Zcash market data, priced per call, paid in shielded ZEC.
// The server holds no keys at all. Payments go to KAGE_PAY_TO; the Kage facilitator checks them
// with the merchant's incoming viewing key and signs a receipt this server verifies.
import express from 'express';
import { kagePaywall, kagePaywallEnv } from 'kage-x402';

const pay = kagePaywall(kagePaywallEnv()); // KAGE_PAY_TO, KAGE_FACILITATOR_URL, KAGE_FACILITATOR_TOKEN, KAGE_RECEIPT_PUBKEY
const app = express();

const PRICES = { price: '0.001 ZEC', report: '0.005 ZEC', convert: '0.0005 ZEC', network: '0.0005 ZEC', history: '0.001 ZEC' } as const;
const FIATS = ['usd', 'eur', 'gbp', 'inr', 'jpy', 'chf', 'cad', 'aud'];

async function getJson(url: string) {
  const r = await fetch(url, { signal: AbortSignal.timeout(10_000) });
  if (!r.ok) throw new Error(`${new URL(url).host} answered ${r.status}`);
  return r.json();
}

// Free: what's for sale.
app.get('/', (_req, res) => {
  res.json({
    name: 'Kage Market Data',
    paidWith: 'shielded ZEC (x402, Zcash testnet)',
    endpoints: [
      { path: '/v1/zec/price', price: PRICES.price, returns: 'live ZEC/USD, 24h change, volume, market cap' },
      { path: '/v1/zec/report', price: PRICES.report, returns: '7-day ZEC report: trend, range, volatility, network activity' },
      { path: '/v1/zec/convert?amount=<ZEC>&to=<fiat>', price: PRICES.convert, returns: `an amount of ZEC in one fiat currency at the live rate (to: ${FIATS.join(', ')})` },
      { path: '/v1/zec/network', price: PRICES.network, returns: 'Zcash mainnet right now: best block, blocks and transactions in 24h, mempool, hashrate, nodes' },
      { path: '/v1/zec/history?days=<7-90>', price: PRICES.history, returns: 'daily ZEC/USD closes for the last N days, with the range and change' },
    ],
  });
});

// 0.001 ZEC: live price.
app.get('/v1/zec/price', pay(PRICES.price, { description: 'Live ZEC/USD price' }), async (_req, res, next) => {
  try {
    const d = await getJson('https://api.coingecko.com/api/v3/simple/price?ids=zcash&vs_currencies=usd&include_24hr_change=true&include_market_cap=true&include_24hr_vol=true');
    const z = d.zcash;
    res.json({ asset: 'ZEC', usd: z.usd, change24hPct: +z.usd_24h_change.toFixed(2), volume24hUsd: Math.round(z.usd_24h_vol), marketCapUsd: Math.round(z.usd_market_cap), source: 'CoinGecko', at: new Date().toISOString() });
  } catch (e) {
    next(e);
  }
});

// 0.005 ZEC: a computed 7-day report.
app.get('/v1/zec/report', pay(PRICES.report, { description: '7-day ZEC market report' }), async (_req, res, next) => {
  try {
    const [chart, net] = await Promise.all([
      getJson('https://api.coingecko.com/api/v3/coins/zcash/market_chart?vs_currency=usd&days=7&interval=daily'),
      getJson('https://api.blockchair.com/zcash/stats'),
    ]);
    const prices: number[] = chart.prices.map((p: [number, number]) => p[1]);
    const first = prices[0];
    const last = prices[prices.length - 1];
    const returns = prices.slice(1).map((p, i) => Math.log(p / prices[i]));
    const mean = returns.reduce((a, b) => a + b, 0) / returns.length;
    const vol = Math.sqrt(returns.reduce((a, r) => a + (r - mean) ** 2, 0) / returns.length);
    const n = net.data;
    res.json({
      asset: 'ZEC',
      window: '7d',
      priceUsd: { open: +first.toFixed(2), close: +last.toFixed(2), high: +Math.max(...prices).toFixed(2), low: +Math.min(...prices).toFixed(2) },
      changePct: +(((last - first) / first) * 100).toFixed(2),
      trend: last > first * 1.02 ? 'up' : last < first * 0.98 ? 'down' : 'flat',
      dailyVolatilityPct: +(vol * 100).toFixed(2),
      network: { blocks24h: n.blocks_24h, transactions24h: n.transactions_24h, mempoolTransactions: n.mempool_transactions, bestBlock: n.best_block_height },
      sources: ['CoinGecko', 'Blockchair'],
      at: new Date().toISOString(),
    });
  } catch (e) {
    next(e);
  }
});

// 0.0005 ZEC: an amount of ZEC in fiat at the live rate.
app.get('/v1/zec/convert', (req, res, next) => {
  // validate before the paywall: a bad request is never charged
  const amount = Number(req.query.amount);
  const to = String(req.query.to ?? '').toLowerCase();
  if (!(amount > 0) || !FIATS.includes(to)) {
    res.status(400).json({ error: `use ?amount=<ZEC, > 0>&to=<one of ${FIATS.join(', ')}>` });
    return;
  }
  next();
}, pay(PRICES.convert, { description: 'ZEC to fiat at the live rate' }), async (req, res, next) => {
  try {
    const amount = Number(req.query.amount);
    const to = String(req.query.to).toLowerCase();
    const d = await getJson(`https://api.coingecko.com/api/v3/simple/price?ids=zcash&vs_currencies=${to}`);
    const rate = d.zcash[to];
    res.json({ amountZec: amount, currency: to.toUpperCase(), rate, value: +(amount * rate).toFixed(2), source: 'CoinGecko', at: new Date().toISOString() });
  } catch (e) {
    next(e);
  }
});

// 0.0005 ZEC: Zcash mainnet activity right now.
app.get('/v1/zec/network', pay(PRICES.network, { description: 'Zcash network activity' }), async (_req, res, next) => {
  try {
    const n = (await getJson('https://api.blockchair.com/zcash/stats')).data;
    res.json({
      chain: 'Zcash mainnet',
      bestBlock: n.best_block_height,
      bestBlockTime: n.best_block_time + ' UTC',
      blocks24h: n.blocks_24h,
      transactions24h: n.transactions_24h,
      mempoolTransactions: n.mempool_transactions,
      hashrate24hSolPerSec: Number(n.hashrate_24h),
      nodes: n.nodes,
      source: 'Blockchair',
      at: new Date().toISOString(),
    });
  } catch (e) {
    next(e);
  }
});

// 0.001 ZEC: daily closes for the last N days.
app.get('/v1/zec/history', (req, res, next) => {
  const days = Number(req.query.days ?? 30);
  if (!Number.isInteger(days) || days < 7 || days > 90) {
    res.status(400).json({ error: 'use ?days=<whole number from 7 to 90>' });
    return;
  }
  next();
}, pay(PRICES.history, { description: 'ZEC/USD daily history' }), async (req, res, next) => {
  try {
    const days = Number(req.query.days ?? 30);
    const chart = await getJson(`https://api.coingecko.com/api/v3/coins/zcash/market_chart?vs_currency=usd&days=${days}&interval=daily`);
    const closes = chart.prices.map(([t, p]: [number, number]) => ({ date: new Date(t).toISOString().slice(0, 10), usd: +p.toFixed(2) }));
    const usd = closes.map((c: { usd: number }) => c.usd);
    res.json({ asset: 'ZEC', days, closes, high: Math.max(...usd), low: Math.min(...usd), changePct: +(((usd[usd.length - 1] - usd[0]) / usd[0]) * 100).toFixed(2), source: 'CoinGecko', at: new Date().toISOString() });
  } catch (e) {
    next(e);
  }
});

app.use((err: Error, _req: express.Request, res: express.Response, _next: express.NextFunction) => {
  res.status(502).json({ error: `upstream data source failed: ${err.message}` });
});

const port = Number(process.env.PORT ?? 4300);
app.listen(port, '127.0.0.1', () => console.log(`Kage Market Data on http://127.0.0.1:${port}  (${Object.entries(PRICES).map(([k, v]) => `${k} ${v}`).join(', ')})`));
