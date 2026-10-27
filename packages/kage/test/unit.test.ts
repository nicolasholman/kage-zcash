// Pure functions: the x402 v1 spec's own 402 example next to a zcash-shielded option, ZIP 321
// rendering (same strings as x402z-protocol's Rust tests), X-PAYMENT round trip, receipt check;
// and the paywall's limits (config check, bounded store, rate limit, facilitator timeout) over
// in-process HTTP, where no facilitator is ever reached.
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { generateKeyPairSync, sign } from 'node:crypto';
import { createServer, type Server } from 'node:http';
import { createServer as netServer, type AddressInfo } from 'node:net';
import { decodePaymentResponse, memoryStore, parsePaymentRequired, paywall, verifyReceipt, X402ZcashError, xPaymentHeader, zecDecimal, zip321Uri } from '../src/index.ts';

const UA = 'utest1sampleaddressforschemaonly000000';

const body = {
  x402Version: 1,
  error: 'X-PAYMENT header is required',
  accepts: [
    // the coinbase x402 v1 spec's example option (EVM `exact`) is skipped
    { scheme: 'exact', network: 'base-sepolia', maxAmountRequired: '10000', resource: 'https://api.example.com/premium-data', description: 'Access to premium market data', mimeType: 'application/json', payTo: '0x209693Bc6afc0C5328bA36FaF03C514EF312287C', maxTimeoutSeconds: 60, asset: '0x036CbD53842c5426634e7929541eC2318f3dCF7e', extra: { name: 'USDC', version: '2' } },
    { scheme: 'zcash-shielded', network: 'zcash-testnet', maxAmountRequired: '100000', resource: 'http://kage.test/api/quote', description: 'd', mimeType: 'application/json', payTo: UA, maxTimeoutSeconds: 900, asset: 'ZEC', extra: { memoNonce: 'x402z-1', expiresAt: 2_000_000_000, paymentUri: 'zcash:x' } },
  ],
};

test('picks and checks the zcash-shielded testnet option', () => {
  const c = parsePaymentRequired(body, 1_900_000_000);
  assert.deepEqual([c.payTo, c.amountZat, c.memoNonce, c.expiresAt], [UA, 100000n, 'x402z-1', 2_000_000_000]);
  assert.throws(() => parsePaymentRequired(body, 2_000_000_000), /expired/);
  assert.throws(() => parsePaymentRequired({ ...body, accepts: [body.accepts[0]] }), /no zcash-shielded/);
  assert.throws(() => parsePaymentRequired({ ...body, x402Version: 2 }), X402ZcashError);
  const bad = structuredClone(body);
  bad.accepts[1].extra!.memoNonce = 'has space';
  assert.throws(() => parsePaymentRequired(bad, 1), /memoNonce/);
});

test('ZIP 321: decimal ZEC without trailing zeros and a base64url memo without padding', () => {
  assert.equal(zecDecimal(100_000n), '0.001');
  assert.equal(zecDecimal(150_000_000n), '1.5');
  assert.equal(zecDecimal(100_000_000n), '1');
  assert.equal(zecDecimal(1n), '0.00000001');
  // the same URI x402z-server puts in extra.paymentUri for this challenge
  assert.equal(zip321Uri(UA, 100_000n, 'x402z-1'), `zcash:${UA}?amount=0.001&memo=eDQwMnotMQ`);
});

test('X-PAYMENT is base64 JSON with the payload x402z-server reads; bad shapes are refused', () => {
  const tx = 'AB'.repeat(32);
  const v = JSON.parse(Buffer.from(xPaymentHeader(tx, 'x402z-1'), 'base64').toString());
  assert.deepEqual(v, { x402Version: 1, scheme: 'zcash-shielded', network: 'zcash-testnet', payload: { txid: 'ab'.repeat(32), memoNonce: 'x402z-1' } });
  assert.throws(() => xPaymentHeader('zz', 'x402z-1'), /64 hex/);
  assert.throws(() => xPaymentHeader(tx, 'n'.repeat(65)), /memoNonce/);
  const r = Buffer.from(JSON.stringify({ success: true, transaction: tx, network: 'zcash-testnet', payer: '' })).toString('base64');
  assert.deepEqual(decodePaymentResponse(r), { success: true, transaction: tx, network: 'zcash-testnet', payer: '' });
});

test('receipts verify only under the facilitator key and only unchanged', () => {
  const { privateKey, publicKey } = generateKeyPairSync('ed25519');
  const pub = publicKey.export({ format: 'der', type: 'spki' }).subarray(12).toString('hex');
  const claims = Buffer.from(JSON.stringify({ memoNonce: 'x402z-1', amountZat: 100000, payTo: UA, txid: 'ab'.repeat(32), confirmations: 1, issuedAt: 1 }));
  const receipt = `${claims.toString('base64url')}.${sign(null, claims, privateKey).toString('base64url')}`;
  assert.equal(verifyReceipt(receipt, pub).amountZat, 100000);
  const other = generateKeyPairSync('ed25519').publicKey.export({ format: 'der', type: 'spki' }).subarray(12).toString('hex');
  assert.throws(() => verifyReceipt(receipt, other), /signature invalid/);
  const forged = `${Buffer.from(claims.toString().replace('100000', '999999')).toString('base64url')}.${receipt.split('.')[1]}`;
  assert.throws(() => verifyReceipt(forged, pub), /signature invalid/);
});

test('a paywall without its facilitator token or receipt key refuses to start', () => {
  assert.throws(() => paywall({ facilitatorUrl: 'http://x', facilitatorToken: '', receiptPubkeyHex: 'ab'.repeat(32), payTo: 'utest1x', amountZat: 1n }), /Setup required/);
  assert.throws(() => paywall({ facilitatorUrl: 'http://x', facilitatorToken: 't', receiptPubkeyHex: 'nope', payTo: 'utest1x', amountZat: 1n }), /Setup required/);
});

// --- paywall resource limits: in-process HTTP, no facilitator is reached ----------------------

async function serveMw(mw: ReturnType<typeof paywall>): Promise<{ url: string; close: () => void }> {
  const app: Server = createServer((req, res) => mw(req, res, () => { res.end('paid'); }));
  await new Promise<void>(r => app.listen(0, '127.0.0.1', r));
  return { url: `http://127.0.0.1:${(app.address() as AddressInfo).port}/premium`, close: () => app.close() };
}

const PUB = 'ab'.repeat(32);

test('the memory store forgets challenges whose redeem window is over: 10,000 unpaid 402s past TTL leave it bounded', () => {
  let t = 1_800_000_000;
  const store = memoryStore({ now: () => t });
  for (let i = 0; i < 10_000; i++) store.add({ memoNonce: `x402z-${i}`, payTo: UA, amountZat: 1n, expiresAt: t + 900 });
  assert.equal(store.size(), 10_000);
  assert.ok(store.markSpent('x402z-0', 'ab'.repeat(32)));
  t += 900 + 1200 + 86_400 + 1; // expiry + grace + redeem window
  store.add({ memoNonce: 'x402z-new', payTo: UA, amountZat: 1n, expiresAt: t + 900 });
  assert.equal(store.size(), 1, 'only the new challenge is left');
  assert.equal(store.get('x402z-0'), undefined, 'a pruned spent nonce is unknown, so it is still refused');
  // inside the window nothing is dropped
  store.add({ memoNonce: 'x402z-open', payTo: UA, amountZat: 1n, expiresAt: t + 900 });
  t += 60;
  store.add({ memoNonce: 'x402z-later', payTo: UA, amountZat: 1n, expiresAt: t + 900 });
  assert.equal(store.size(), 3);
});

test('a full store answers 503 and issues nothing; it never evicts an open challenge', async () => {
  const store = memoryStore({ maxEntries: 2 });
  const s = await serveMw(paywall({ facilitatorUrl: 'http://127.0.0.1:9', facilitatorToken: 't', receiptPubkeyHex: PUB, payTo: UA, amountZat: 1n, store, ratePerMin: 0 }));
  try {
    assert.equal((await fetch(s.url)).status, 402);
    assert.equal((await fetch(s.url)).status, 402);
    const full = await fetch(s.url);
    assert.equal(full.status, 503);
    assert.match((await full.json() as { error: string }).error, /challenge store full/);
    assert.equal(store.size(), 2);
  } finally {
    s.close();
  }
});

test('402s and X-PAYMENT attempts share a per-client budget: 30 a minute, then 429 with Retry-After', async () => {
  const store = memoryStore();
  const s = await serveMw(paywall({ facilitatorUrl: 'http://127.0.0.1:9', facilitatorToken: 't', receiptPubkeyHex: PUB, payTo: UA, amountZat: 1n, store }));
  try {
    for (let i = 0; i < 30; i++) assert.equal((await fetch(s.url)).status, 402, `request ${i}`);
    const r = await fetch(s.url, { headers: { 'X-PAYMENT': xPaymentHeader('ab'.repeat(32), 'x402z-never') } });
    assert.equal(r.status, 429);
    assert.ok(Number(r.headers.get('retry-after')) >= 1);
    assert.equal(store.size(), 30, 'the 429 issued no challenge');
  } finally {
    s.close();
  }
});

test('a facilitator that never answers is a 502 after facilitatorTimeoutMs, and nothing is burned', async () => {
  const hang = netServer(sock => { sock.on('data', () => {}); }); // accepts, reads, never answers
  await new Promise<void>(r => hang.listen(0, '127.0.0.1', r));
  const store = memoryStore();
  store.add({ memoNonce: 'x402z-hang', payTo: UA, amountZat: 1n, expiresAt: 4_000_000_000 });
  const facilitatorUrl = `http://127.0.0.1:${(hang.address() as AddressInfo).port}`;
  const s = await serveMw(paywall({ facilitatorUrl, facilitatorToken: 't', receiptPubkeyHex: PUB, payTo: UA, amountZat: 1n, store, facilitatorTimeoutMs: 300 }));
  try {
    const t0 = Date.now();
    const r = await fetch(s.url, { headers: { 'X-PAYMENT': xPaymentHeader('ab'.repeat(32), 'x402z-hang') } });
    assert.equal(r.status, 502);
    assert.ok(Date.now() - t0 < 5_000, 'gave up instead of waiting for the facilitator');
    assert.match((await r.json() as { error: string }).error, /facilitator unreachable/);
    assert.equal(store.get('x402z-hang')?.spentTxid, undefined);
  } finally {
    s.close();
    hang.close();
  }
});
