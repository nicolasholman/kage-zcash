// The kage-x402 SDK against the real engine binaries (KAGE_ENGINE_DIR, default target/release) and
// a real paywall on 127.0.0.1. Policy refusals need no chain; the approval and wallet tests reach
// testnet lightwalletd (testnet.zec.rocks) to create and sync a fresh, empty wallet.
import { after, before, test } from 'node:test';
import assert from 'node:assert/strict';
import { existsSync, mkdtempSync, rmSync, statSync, readFileSync } from 'node:fs';
import { createServer, type Server } from 'node:http';
import type { AddressInfo } from 'node:net';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { createKage, KageError, type KageEvent } from '../src/agent.ts';
import { engineDir, KageSetupError } from '../src/engine.ts';
import { assertIncomingOnly, createWallet, keyKind } from '../src/keys.ts';
import { formatZec, toZat } from '../src/units.ts';
import { kagePaywall, kagePaywallEnv } from '../src/index.ts';

const ROOT = fileURLToPath(new URL('../../../', import.meta.url));
process.env.KAGE_ENGINE_DIR ??= join(ROOT, 'target', 'release');
const MERCHANT_UA =
  'utest100fnpp4zd4lp0vmu093gwx6p6ujv8lusunnkvrsuxyaca9avmea5fqgt0ktkvm0sf2wnn8fw7ad034fy9lvsrq0x04vn0jqq7dvnyua5zk9s30v2lcys8pk3mm7xnkdt7fyf65xe0vtnv7sswdflaltg9uesty00c6x2kvgc25ygcrzs';

let server: Server;
let base = '';
let tmp = '';
let wallet: { seed: string; birthday: number; dir: string };

before(async () => {
  assert.ok(existsSync(join(process.env.KAGE_ENGINE_DIR!, 'x402z-mcp')), `build the engine first: cargo build --release -p x402z-mcp -p x402z-agent -p x402z-facilitator`);
  tmp = mkdtempSync(join(tmpdir(), 'kage-sdk-'));
  // a real merchant: 0.001 ZEC for /v1/price, 0.005 ZEC for /v1/report (the facilitator is never reached here)
  const pay = kagePaywall({ payTo: MERCHANT_UA, facilitatorUrl: 'http://127.0.0.1:9', facilitatorToken: 't'.repeat(32), receiptPubkeyHex: '00'.repeat(32) });
  const price = pay('0.001 ZEC');
  const report = pay('0.005 ZEC');
  server = createServer((req, res) => {
    const mw = req.url === '/v1/report' ? report : price;
    mw(req, res, () => res.end('{"paid":true}'));
  });
  await new Promise<void>((r) => server.listen(0, '127.0.0.1', r));
  base = `http://127.0.0.1:${(server.address() as AddressInfo).port}`;
  const w = createWallet({ dir: join(tmp, 'agent') });
  const text = readFileSync(w.secretFile, 'utf8');
  wallet = { seed: /KAGE_SEED="([^"]+)"/.exec(text)![1], birthday: w.birthday, dir: join(tmp, 'agent') };
});

after(() => {
  server?.close();
  if (tmp) rmSync(tmp, { recursive: true, force: true });
});

test('ZEC amounts parse to zatoshi and back', () => {
  assert.equal(toZat('0.001 ZEC'), 100_000n);
  assert.equal(toZat('1'), 100_000_000n);
  assert.equal(toZat('0.00000001 TAZ'), 1n);
  assert.equal(toZat(5n), 5n);
  assert.throws(() => toZat('0.000000001'), RangeError);
  assert.throws(() => toZat('-1'), RangeError);
  assert.equal(formatZec(100_000n), '0.001');
  assert.equal(formatZec(250_000_000n), '2.5');
});

test('createWallet keeps the seed in a 0600 file and returns only the address and UIVK', () => {
  const file = join(wallet.dir, 'seed.env');
  assert.equal(statSync(file).mode & 0o777, 0o600);
  assert.equal(wallet.seed.split(' ').length, 24);
  assert.throws(() => createWallet({ dir: wallet.dir }), /refusing to overwrite|EEXIST/, 'never overwrites an existing seed file');
});

test('key kinds: only an incoming viewing key may go to a facilitator', () => {
  assert.equal(keyKind('uivktest1abc'), 'uivk');
  assert.equal(keyKind('uviewtest1abc'), 'ufvk');
  assert.equal(keyKind(MERCHANT_UA), 'address');
  assert.doesNotThrow(() => assertIncomingOnly('uivktest1abc'));
  assert.throws(() => assertIncomingOnly('uviewtest1abc'), /full viewing key/);
});

test('setup errors name what is missing', async () => {
  await assert.rejects(createKage({ wallet: { seed: '', dir: tmp }, policy: { dailyCap: '1', allowedHosts: [] } }), (e: Error) => e instanceof KageSetupError && /wallet.seed/.test(e.message));
  const saved = process.env.KAGE_ENGINE_DIR;
  process.env.KAGE_ENGINE_DIR = '/nonexistent-too';
  try {
    assert.throws(() => engineDir('/nonexistent'), (e: Error) => e instanceof KageSetupError && /Setup required: no Kage engine/.test(e.message));
  } finally {
    process.env.KAGE_ENGINE_DIR = saved;
  }
  assert.throws(() => kagePaywallEnv({}), /KAGE_PAY_TO not configured/);
});

test('a host outside allowedHosts is refused before any request; nothing is paid', async () => {
  const events: KageEvent[] = [];
  const kage = await createKage({ wallet, policy: { dailyCap: '0.01 ZEC', allowedHosts: ['api.example.com'] }, onEvent: (e) => e.type !== 'log' && events.push(e) });
  try {
    await assert.rejects(kage.fetch(`${base}/v1/price`), (e: KageError) => e.code === 'HOST_NOT_ALLOWED' && e.paid === false);
    assert.deepEqual(events.map((e) => e.type), ['request', 'refused']);
    const [last] = (await kage.ledger(1)).slice(-1);
    assert.equal(last.decision, 'denied');
    assert.equal(last.txid, undefined);
  } finally {
    kage.close();
  }
});

test('a price over the daily cap is refused with CAP_EXCEEDED', async () => {
  const kage = await createKage({ wallet, policy: { dailyCap: '0.0005 ZEC', allowedHosts: ['127.0.0.1'] } });
  try {
    await assert.rejects(kage.fetch(`${base}/v1/price`), (e: KageError) => e.code === 'CAP_EXCEEDED' && /No transaction was built or broadcast/.test(e.message));
    const s = await kage.status();
    assert.equal(s.dailyCapZat, 50_000n);
    assert.equal(s.spentTodayZat, 0n);
  } finally {
    kage.close();
  }
});

test('above the approval threshold: no handler refuses, a "no" refuses, a "yes" reaches the wallet', async () => {
  const asked: { amount: string; host: string }[] = [];
  let answer = false;
  const kage = await createKage({
    wallet,
    policy: { dailyCap: '0.01 ZEC', allowedHosts: ['127.0.0.1'], approvalThreshold: '0.002 ZEC' },
    onApproval: (r) => {
      asked.push({ amount: r.amount, host: r.host });
      return answer;
    },
  });
  try {
    await assert.rejects(kage.fetch(`${base}/v1/report`), (e: KageError) => e.code === 'APPROVAL_DENIED');
    assert.deepEqual(asked, [{ amount: '0.005', host: base.slice('http://'.length) }]);
    answer = true;
    // approved, so the policy lets it through; the fresh wallet is empty, so the engine stops there
    await assert.rejects(kage.fetch(`${base}/v1/report`), (e: KageError) => e.code === 'INSUFFICIENT_FUNDS' && e.paid === false);
    const entries = await kage.ledger(5);
    assert.ok(entries.some((e) => e.decision === 'needs_approval'));
    assert.ok(entries.some((e) => e.decision === 'allowed' && /approved by operator/.test(e.reason ?? '')), JSON.stringify(entries));
  } finally {
    kage.close();
  }
  // without an onApproval handler the same payment is refused outright
  const strict = await createKage({ wallet, policy: { dailyCap: '0.01 ZEC', allowedHosts: ['127.0.0.1'], approvalThreshold: '0.002 ZEC' } });
  try {
    await assert.rejects(strict.fetch(`${base}/v1/report`), (e: KageError) => e.code === 'APPROVAL_REQUIRED');
  } finally {
    strict.close();
  }
});

test('wallet() syncs the fresh wallet: address, UIVK, zero balance', async () => {
  const kage = await createKage({ wallet, policy: { dailyCap: 0n, allowedHosts: [] } });
  try {
    const w = await kage.wallet();
    assert.match(w.address, /^utest1/);
    assert.equal(keyKind(w.uivk), 'uivk');
    assert.equal(w.spendableZat, 0n);
    assert.ok(w.chainTip > 4_000_000);
  } finally {
    kage.close();
  }
});
