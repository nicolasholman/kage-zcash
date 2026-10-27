// A paid API in ~60 lines: live Zcash market data, priced per call, paid in shielded ZEC.
// The server holds no keys at all. Payments go to KAGE_PAY_TO; the Kage facilitator checks them
// with the merchant's incoming viewing key and signs a receipt this server verifies.
import express from 'express';
import { kagePaywall, kagePaywallEnv } from 'kage-x402';

const pay = kagePaywall(kagePaywallEnv()); // KAGE_PAY_TO, KAGE_FACILITATOR_URL, KAGE_FACILITATOR_TOKEN, KAGE_RECEIPT_PUBKEY
const app = express();

const PRICES = { price: '0.001 ZEC', report: '0.005 ZEC' } as const;

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

app.use((err: Error, _req: express.Request, res: express.Response, _next: express.NextFunction) => {
  res.status(502).json({ error: `upstream data source failed: ${err.message}` });
});

const port = Number(process.env.PORT ?? 4300);
app.listen(port, '127.0.0.1', () => console.log(`Kage Market Data on http://127.0.0.1:${port}  (price ${PRICES.price}, report ${PRICES.report})`));
