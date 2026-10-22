// Against the real Kage binaries and Zcash testnet, nothing mocked: x402z-facilitator holding only
// the demo merchant's published UIVK (a fresh receipt key and token per run), x402z-server, and
// this package's paywall middleware settling through that facilitator. Needs network
// (testnet.zec.rocks) and the binaries: cargo build -p x402z-server -p x402z-facilitator --bins --examples
import { after, before, test } from 'node:test';
import assert from 'node:assert/strict';
import { spawn, execFileSync, type ChildProcess } from 'node:child_process';
import { randomBytes } from 'node:crypto';
import { existsSync, mkdtempSync, readFileSync, rmSync } from 'node:fs';
import { createServer, type Server } from 'node:http';
import { createServer as netServer, type AddressInfo } from 'node:net';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { fetchWithPayment, memoryStore, parsePaymentRequired, paywall, xPaymentHeader, zip321Uri } from '../src/index.ts';

const ROOT = fileURLToPath(new URL('../../../', import.meta.url));
const BIN = process.env.KAGE_BIN_DIR ?? join(ROOT, 'target', 'debug');
// Real mined Kage payments to the demo merchant (README, "Verified testnet runs").
const PAID = { txid: 'ccb4214c6a1fdf183c086ae5b4f559d181ead4c1c61dc7e711fc4a08b95b0ed3', memoNonce: 'x402z-b7a15cc628bb7a06bf9a26cd' };
const OTHER = { txid: '25a8b324bebee30220382113c6fb9cc3be1144b036117c34778bde3b1d8b689d', memoNonce: 'x402z-696c58afd509cc0f8753f3b0' };

const procs: ChildProcess[] = [];
let dir = '';
let facilitator = '';
let server = '';
let ua = '';
let pubkey = '';
let token = '';

async function freePort(): Promise<number> {
  return new Promise(res => { const s = netServer().listen(0, '127.0.0.1', () => { const p = (s.address() as AddressInfo).port; s.close(() => res(p)); }); });
}

function start(bin: string, env: Record<string, string>, ready: RegExp): Promise<void> {
  const p = spawn(join(BIN, bin), [], { env: { PATH: process.env.PATH ?? '', ...env }, stdio: ['ignore', 'ignore', 'pipe'] });
  procs.push(p);
  return new Promise((res, rej) => {
    let log = '';
    const t = setTimeout(() => rej(new Error(`${bin} did not start: ${log}`)), 60_000);
    p.stderr!.on('data', d => { log += d; if (ready.test(log)) { clearTimeout(t); res(); } });
    p.on('exit', code => { clearTimeout(t); rej(new Error(`${bin} exited ${code}: ${log}`)); });
  });
}

before(async () => {
  for (const b of ['x402z-facilitator', 'x402z-server', join('examples', 'check_uivk')]) {
    if (!existsSync(join(BIN, b))) throw new Error(`${join(BIN, b)} missing; build first: cargo build -p x402z-server -p x402z-facilitator --bins --examples`);
  }
  dir = mkdtempSync(join(tmpdir(), 'kage-ts-'));
  const uivk = /uivktest1[a-z0-9]{100,}/.exec(readFileSync(join(ROOT, 'README.md'), 'utf8'))![0];
  ua = execFileSync(join(BIN, 'examples', 'check_uivk'), ['address', uivk], { encoding: 'utf8' }).trim();
  // a throwaway receipt key, kept in memory only
  const keys = Object.fromEntries(execFileSync(join(BIN, 'x402z-facilitator'), ['keygen'], { encoding: 'utf8' }).trim().split('\n').map(l => l.split('=')));
  pubkey = keys.X402Z_RECEIPT_PUBKEY;
  token = randomBytes(32).toString('hex');
  const fport = await freePort();
  facilitator = `http://127.0.0.1:${fport}`;
  await start('x402z-facilitator', { X402Z_MERCHANT_UIVK: uivk, X402Z_RECEIPT_SIGNING_KEY: keys.X402Z_RECEIPT_SIGNING_KEY, X402Z_FACILITATOR_TOKEN: token, X402Z_FACILITATOR_BIND: `127.0.0.1:${fport}`, X402Z_FACILITATOR_DB: join(dir, 'f.sqlite') }, /listening/);
  const sport = await freePort();
  server = `http://127.0.0.1:${sport}`;
  await start('x402z-server', { X402Z_MERCHANT_UA: ua, X402Z_RECEIPT_PUBKEY: pubkey, X402Z_FACILITATOR_URL: facilitator, X402Z_FACILITATOR_TOKEN: token, X402Z_BIND: `127.0.0.1:${sport}`, X402Z_ADMIN_BIND: 'off', X402Z_SERVER_DB: join(dir, 's.sqlite') }, /listening on/);
});

after(() => {
  for (const p of procs) p.kill();
  if (dir) rmSync(dir, { recursive: true, force: true });
});

test("x402z-server's 402 parses, its ZIP 321 URI is ours, and a mined payment for another nonce is refused", async () => {
  const r = await fetch(`${server}/api/quote`);
  assert.equal(r.status, 402);
  const c = parsePaymentRequired(await r.json());
  assert.equal(c.payTo, ua);
  assert.equal(c.amountZat, 100_000n);
  assert.equal(zip321Uri(c.payTo, c.amountZat, c.memoNonce), c.paymentUri);
  // a real mined payment, but its memo is another challenge's nonce: the facilitator trial-decrypts
  // it live with the UIVK and finds no output for this nonce
  const out = await fetchWithPayment(`${server}/api/quote`, { txid: PAID.txid, memoNonce: c.memoNonce });
  assert.equal(out.status, 402);
  assert.equal(out.settlement?.success, false);
  assert.equal(out.settlement?.errorReason, 'invalid_payment');
  assert.match(String((out.body as { error: string }).error), /no output in this transaction decrypts/);
  assert.notEqual(out.challenge?.memoNonce, c.memoNonce, 'a refusal comes with a fresh challenge');
});

test('the paywall middleware settles a real mined payment through the real facilitator, once', async () => {
  const store = memoryStore();
  // the challenge ccb4214c paid (its nonce and amount, to the demo merchant)
  store.add({ memoNonce: PAID.memoNonce, payTo: ua, amountZat: 100_000n, expiresAt: 4_000_000_000 });
  const opts = { facilitatorUrl: facilitator, facilitatorToken: token, receiptPubkeyHex: pubkey, payTo: ua, amountZat: 100_000n, store };
  const mw = paywall(opts);
  const app: Server = createServer((req, res) => mw(req, res, (err?: unknown) => {
    res.statusCode = err ? 500 : 200;
    res.setHeader('content-type', 'application/json');
    res.end(JSON.stringify(err ? { error: String(err) } : { paid: 'content' }));
  }));
  await new Promise<void>(r => app.listen(0, '127.0.0.1', r));
  const url = `http://127.0.0.1:${(app.address() as AddressInfo).port}/premium`;
  try {
    const first = await fetch(url);
    assert.equal(first.status, 402);
    const c = parsePaymentRequired(await first.json());
    assert.match(c.memoNonce, /^x402z-[0-9a-f]{24}$/);

    const paid = await fetchWithPayment(url, PAID);
    assert.equal(paid.status, 200, JSON.stringify(paid.body));
    assert.deepEqual(paid.settlement, { success: true, transaction: PAID.txid, network: 'zcash-testnet', payer: '' });
    assert.deepEqual(paid.body, { paid: 'content' });

    // the payer lost that 200 and presents the same payment again: the facilitator confirms it
    // again and the content is served again; nothing new is burned
    const resent = await fetchWithPayment(url, PAID);
    assert.equal(resent.status, 200, JSON.stringify(resent.body));
    assert.deepEqual(resent.settlement, { success: true, transaction: PAID.txid, network: 'zcash-testnet', payer: '' });
    assert.equal(store.get(PAID.memoNonce)?.spentTxid, PAID.txid);

    // once the resend window is over, the same payment is a replay
    store.get(PAID.memoNonce)!.spentAt! -= 601;
    const again = await fetchWithPayment(url, PAID);
    assert.equal(again.status, 402);
    assert.equal(again.settlement?.errorReason, 'nonce_already_used');

    // another real payment presented for this middleware's fresh challenge: wrong memo
    const wrong = await fetchWithPayment(url, { txid: OTHER.txid, memoNonce: c.memoNonce });
    assert.equal(wrong.status, 402);
    assert.equal(wrong.settlement?.errorReason, 'invalid_payment');

    const bad = await fetch(url, { headers: { 'X-PAYMENT': Buffer.from(JSON.stringify({ x402Version: 1, scheme: 'zcash-shielded', network: 'zcash-testnet', payload: { txid: 'zz', memoNonce: c.memoNonce } })).toString('base64') } });
    assert.equal(bad.status, 400);
    const unknown = await fetch(url, { headers: { 'X-PAYMENT': xPaymentHeader(OTHER.txid, 'x402z-never-issued') } });
    assert.equal(unknown.status, 402);

    // a middleware holding the wrong facilitator token is told so, nothing burned
    const store2 = memoryStore();
    store2.add({ memoNonce: OTHER.memoNonce, payTo: ua, amountZat: 100_000n, expiresAt: 4_000_000_000 });
    const mw2 = paywall({ ...opts, facilitatorToken: 'not-the-token', store: store2 });
    const app2 = createServer((req, res) => mw2(req, res, () => { res.end('x'); }));
    await new Promise<void>(r => app2.listen(0, '127.0.0.1', r));
    const r2 = await fetchWithPayment(`http://127.0.0.1:${(app2.address() as AddressInfo).port}/`, OTHER);
    app2.close();
    assert.equal(r2.status, 502);
    assert.match(String((r2.body as { error: string }).error), /Setup required/);
    assert.equal(store2.get(OTHER.memoNonce)?.spentTxid, undefined);
  } finally {
    app.close();
  }
});
