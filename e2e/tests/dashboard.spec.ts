import { test, expect, expectNoHorizontalScroll, isMobile } from './fixtures';
import type { APIRequestContext, Page } from '@playwright/test';
import { createPrivateKey, randomBytes, sign } from 'node:crypto';
import { readFileSync } from 'node:fs';

const at = (port: number) => `http://127.0.0.1:${port}/`;
// Dashboards live on each x402z-server's admin port; PUBLIC_API is the same server's public port.
const SERVER = 4120, NO_FAC = 4130, DEMO_LEDGER = 4140, BAD_LEDGER = 4141, RATE_LIMITED = 4142;
const PUBLIC_API = 4020;
// A real mined Kage payment (block 4,524,586) to the published demo merchant. Its memo is a
// different, already-used nonce, so against a fresh challenge the honest answer is a refusal.
const MINED_TXID = 'ccb4214c6a1fdf183c086ae5b4f559d181ead4c1c61dc7e711fc4a08b95b0ed3';

/**
 * Another client pays challenge `nonce`: it presents an X-PAYMENT-RECEIPT signed with this run's
 * throwaway receipt key (stack.sh made it with `x402z-facilitator keygen`, 0600, never printed).
 * That is the server's real legacy receipt path (signature, nonce, amount and payTo checked, the
 * nonce burned). Returns the receipt's txid.
 */
async function anotherClientPays(request: APIRequestContext, port: number, nonce: string, payTo: string) {
  const env = readFileSync(new URL('../.run/receipt.env', import.meta.url), 'utf8');
  const seed = Buffer.from(/^X402Z_RECEIPT_SIGNING_KEY=([0-9a-f]{64})$/m.exec(env)![1], 'hex');
  const key = createPrivateKey({ key: Buffer.concat([Buffer.from('302e020100300506032b657004220420', 'hex'), seed]), format: 'der', type: 'pkcs8' });
  const txid = randomBytes(32).toString('hex');
  const now = Math.floor(Date.now() / 1000);
  const claims = Buffer.from(JSON.stringify({ memoNonce: nonce, amountZat: 100000, payTo, txid, confirmations: 1, issuedAt: now, minedAt: now }));
  const receipt = `${claims.toString('base64url')}.${sign(null, claims, key).toString('base64url')}`;
  const r = await request.get(at(port) + 'api/quote', { headers: { 'X-PAYMENT-RECEIPT': receipt } });
  expect(r.status(), await r.text()).toBe(200);
  return txid;
}

async function sendRequest(page: Page) {
  const quote = page.waitForResponse(r => r.url().endsWith('/api/quote'));
  await page.locator('#try').click();
  return quote;
}

test.describe('merchant dashboard', () => {
  // A 402 from /api/quote is the product's answer, not an error (Chrome logs it as one).
  test.beforeEach(({ watch }) => watch.allow(/\/api\/quote$/, 402));

  test('loads: facilitator reachable, price, merchant UA and receipt key from /api/status', async ({ page, request }) => {
    const s = await (await request.get(at(SERVER) + 'api/status')).json();
    await page.goto(at(SERVER));
    await expect(page).toHaveTitle('Kage — merchant console');
    await expect(page.locator('#facState')).toHaveText('reachable');
    await expect(page.locator('#facState')).toHaveClass(/\bok\b/);
    await expect(page.locator('#price')).toHaveText('0.001 TAZ');
    await expect(page.locator('#ua')).toHaveAttribute('title', s.merchantUa);
    await expect(page.locator('#ua')).toHaveText(`${s.merchantUa.slice(0, 12)}…${s.merchantUa.slice(-8)}`);
    await expect(page.locator('#rk')).toHaveAttribute('title', s.receiptKey);
    await expect(page.locator('#banners')).toBeEmpty();
    await expectNoHorizontalScroll(page);
    await page.screenshot({ path: test.info().outputPath('dash-loaded.png'), fullPage: true });
  });

  test('merchant view hides the payer ledger (spendguard panel and totals chip)', async ({ page }) => {
    await page.goto(at(SERVER));
    await expect(page.locator('#capChip')).toHaveText(/Merchant view\s*· payer ledger not shown/);
    await expect(page.locator('#agents')).toContainText('Payer ledger not shown');
    await expect(page.locator('#audit')).toBeEmpty();
    await expect(page.locator('#banners')).toBeEmpty();
  });

  test('Send request → live 402 card with ZIP 321 URI, QR, countdown and progress', async ({ page }) => {
    await page.goto(at(SERVER));
    const before = Number(await page.locator('#chCount').textContent());
    const r = await sendRequest(page);
    expect(r.status()).toBe(402);
    const body = await r.json();
    const a = body.accepts[0];
    const nonce: string = a.extra.memoNonce;
    expect(nonce).toMatch(/^x402z-[0-9a-f]{24}$/);
    const card = page.locator('#challenge');
    await expect(card.locator('.status402')).toContainText('402 Payment Required');
    await expect(card.locator('#ttl')).toHaveText(/^expires in 1[45]:\d\d$/);
    await expect(card).toContainText('zcash-shielded · zcash-testnet');
    await expect(card.locator('.kv')).toContainText('0.001 TAZ (100000 zat)');
    await expect(card.locator('.kv dd.m').nth(1)).toHaveText(nonce);
    // The QR is rendered by the server for this challenge, and loads.
    const qr = card.locator('img.qr');
    await expect(qr).toHaveAttribute('src', `/api/challenge/${nonce}/qr.svg`);
    await expect.poll(() => qr.evaluate((i: HTMLImageElement) => i.complete && i.naturalWidth)).toBeGreaterThan(0);
    // The ZIP 321 URI pays the 402's address and amount, with the nonce as the memo.
    const uri = await card.locator('.uri code').textContent();
    expect(uri).toBe(a.extra.paymentUri);
    const u = new URL(uri!);
    expect(u.protocol).toBe('zcash:');
    expect(u.pathname).toBe(a.payTo);
    expect(u.searchParams.get('amount')).toBe('0.001');
    expect(Buffer.from(u.searchParams.get('memo')!, 'base64url').toString()).toBe(nonce);
    await expect(card.getByRole('link', { name: 'Open in wallet' })).toHaveAttribute('href', uri!);
    // Progress: request → 402 → waiting for the shielded payment.
    await expect(page.locator('.step[data-s="2"]')).toHaveClass(/\bon\b/);
    await expect(page.locator('.step[data-s="1"]')).toHaveClass(/\bdone\b/);
    await expect(page.locator('.step[data-s="4"]')).not.toHaveClass(/\bdone\b/);
    // The feed and totals pick up the challenge.
    await expect(page.locator('#feed li').filter({ hasText: nonce })).toContainText('open');
    await expect(page.locator('#chCount')).toHaveText(String(before + 1));
    await expect(page.locator('#feedEmpty')).toBeHidden();
    await expectNoHorizontalScroll(page);
    await page.locator('#pay').screenshot({ path: test.info().outputPath('dash-402-card.png') });
  });

  test('402 card copy buttons copy payTo, amount, memo and the URI', async ({ page }) => {
    await page.goto(at(SERVER));
    const a = (await (await sendRequest(page)).json()).accepts[0];
    const btns = page.locator('#challenge button.copy');
    await expect(btns).toHaveCount(4);
    const want = [a.payTo, '0.001', a.extra.memoNonce, a.extra.paymentUri];
    for (let i = 0; i < 4; i++) {
      await btns.nth(i).click();
      await expect(btns.nth(i)).toHaveText('copied');
      expect(await page.evaluate(() => navigator.clipboard.readText())).toBe(want[i]);
      await expect(btns.nth(i)).toHaveText('copy', { timeout: 3000 });
    }
  });

  test('paste a non-txid: the browser blocks it and nothing is sent', async ({ page }) => {
    await page.goto(at(SERVER));
    await sendRequest(page);
    let posted = 0;
    page.on('request', r => { if (r.url().endsWith('/api/redeem')) posted++; });
    await page.locator('#txid').fill('not-a-txid');
    await page.getByRole('button', { name: 'Redeem' }).click();
    expect(await page.locator('#txid').evaluate((i: HTMLInputElement) => i.validity.patternMismatch)).toBe(true);
    await page.waitForTimeout(500);
    expect(posted).toBe(0);
    await expect(page.locator('#redeemOut')).toBeEmpty();
  });

  test('paste an already-mined txid against a fresh challenge: refused, live, and logged in the feed', async ({ page, watch }) => {
    watch.allow(/\/api\/quote$/, 402);
    watch.allow(/\/api\/redeem$/, 422);
    await page.goto(at(SERVER));
    const nonce = (await (await sendRequest(page)).json()).accepts[0].extra.memoNonce;
    await expect(page.locator('#chCount')).not.toHaveText('0');
    const rej = Number(await page.locator('#rejCount').textContent());
    await page.locator('#txid').fill(MINED_TXID);
    const res = page.waitForResponse(r => r.url().endsWith('/api/redeem'));
    await page.getByRole('button', { name: 'Redeem' }).click();
    const r = await res;
    expect(r.status()).toBe(422);
    expect(await r.json()).toMatchObject({ status: 'rejected', reason: 'invalid_payment' });
    const out = page.locator('#redeemOut');
    await expect(out).toHaveClass(/\bbad\b/);
    await expect(out).toHaveText('Refused. no output in this transaction decrypts to the merchant viewing key with this memo nonce');
    // The payment feed logs the refusal, with a well-formed explorer link to the pasted tx.
    const row = page.locator('#feed li').filter({ hasText: 'no output in this transaction decrypts' }).first();
    await expect(row.locator('.st')).toHaveText('refused');
    await expect(row.locator('a')).toHaveAttribute('href', `https://testnet.zecblock.com/tx/${MINED_TXID}`);
    await expect(page.locator('#rejCount')).toHaveText(String(rej + 1));
    // Nothing was burned: the challenge is still open, and the progress never reached 200.
    await expect(page.locator('#ttl')).toHaveText(/^expires in/);
    await expect(page.locator('.step[data-s="4"]')).not.toHaveClass(/\bdone\b/);
    await expect(page.locator('#feed li').filter({ hasText: nonce }).first()).toBeVisible();
    await page.locator('#pay').screenshot({ path: test.info().outputPath('dash-refused.png') });
  });

  test("another client's payment lands in the feed but never ticks the 402 card on screen", async ({ page, request }) => {
    await page.goto(at(SERVER));
    const mine = (await (await sendRequest(page)).json()).accepts[0];
    await expect(page.locator('.step[data-s="2"]')).toHaveClass(/\bon\b/);
    // Someone else (an agent on the public port) gets its own 402 and pays it.
    const theirs = (await (await request.get(at(PUBLIC_API) + 'api/quote')).json()).accepts[0];
    expect(theirs.extra.memoNonce).not.toBe(mine.extra.memoNonce);
    const txid = await anotherClientPays(request, PUBLIC_API, theirs.extra.memoNonce, theirs.payTo);
    const row = page.locator('#feed li').filter({ has: page.locator(`a[href$="${txid}"]`) });
    await expect(row.locator('.st')).toHaveText('paid');
    // My card still waits for my payment.
    await page.waitForTimeout(2000); // more than one feed poll
    await expect(page.locator('.step[data-s="4"]')).not.toHaveClass(/\bdone\b/);
    await expect(page.locator('.step[data-s="2"]')).toHaveClass(/\bon\b/);
    await expect(page.locator('#ttl')).toHaveText(/^expires in \d+:\d\d$/);
    // A fresh page loads that paid event with the history and claims nothing either.
    await page.reload();
    await expect(page.locator('#feed li').filter({ has: page.locator(`a[href$="${txid}"]`) })).toBeVisible();
    await page.waitForTimeout(2000);
    for (const s of [0, 1, 2, 3, 4]) await expect(page.locator(`.step[data-s="${s}"]`)).not.toHaveClass(/\bdone\b/);
  });

  test("a payment for the card's own challenge ticks 200 and stops the countdown", async ({ page, request }) => {
    await page.goto(at(SERVER));
    const mine = (await (await sendRequest(page)).json()).accepts[0];
    await expect(page.locator('#ttl')).toHaveText(/^expires in/);
    await anotherClientPays(request, PUBLIC_API, mine.extra.memoNonce, mine.payTo); // e.g. the agent paid this card's nonce
    await expect(page.locator('.step[data-s="4"]')).toHaveClass(/\bdone\b/);
    await expect(page.locator('#ttl')).toHaveText('paid');
  });

  test('sidebar nav, including "Payment feed", jumps to each section and highlights it', async ({ page }) => {
    await page.goto(at(SERVER));
    const nav = page.locator('.nav a');
    await expect(nav).toHaveText(['Try a 402', 'Totals', 'Payment feed', 'Spendguard', 'Keys']);
    for (const [name, id] of [['Payment feed', 'activity'], ['Totals', 'overview'], ['Spendguard', 'spendguard'], ['Keys', 'keys'], ['Try a 402', 'pay']]) {
      await nav.filter({ hasText: name }).click();
      await expect(page).toHaveURL(new RegExp(`#${id}$`));
      await expect(page.locator(`#${id}`)).toBeInViewport();
      await expect(nav.filter({ hasText: name })).toHaveClass(/\bon\b/);
    }
  });

  test('no button labelled "log" exists; the payment log (feed) polls and shows every event', async ({ page, request }) => {
    await page.goto(at(SERVER));
    await expect(page.getByRole('button', { name: /log/i })).toHaveCount(0);
    // A 402 issued by someone else (an agent hitting the API) shows up in the open dashboard.
    const r = await request.get(at(SERVER) + 'api/quote');
    expect(r.status()).toBe(402);
    const nonce = (await r.json()).accepts[0].extra.memoNonce;
    await expect(page.locator('#feed li').filter({ hasText: nonce })).toBeVisible({ timeout: 5000 });
    const events = await (await request.get(at(SERVER) + 'api/events?since=0')).json();
    await expect(page.locator('#feed li')).toHaveCount(events.length);
  });

  test('the public port serves the paid API only: no dashboard, feed or status', async ({ request }) => {
    for (const path of ['', 'api/events?since=0', 'api/status']) {
      expect((await request.get(at(PUBLIC_API) + path)).status(), `/${path} on the public port`).toBe(404);
      expect((await request.get(at(SERVER) + path)).status(), `/${path} on the admin port`).toBe(200);
    }
    expect((await request.get(at(PUBLIC_API) + 'api/quote')).status()).toBe(402);
  });

  test('empty state on a fresh server: no requests yet', async ({ page }) => {
    await page.goto(at(BAD_LEDGER)); // nobody sends requests to this one
    await expect(page.locator('#feedEmpty')).toBeVisible();
    await expect(page.locator('#feedEmpty')).toContainText('No requests yet');
    await expect(page.locator('#challenge')).toContainText('Send the request');
    await expect(page.locator('#chCount')).toHaveText('0');
  });

  test('Setup required: X402Z_DEMO_AGENT_LEDGER set but unreadable', async ({ page }) => {
    await page.goto(at(BAD_LEDGER));
    await expect(page.locator('#banners .banner')).toContainText('X402Z_DEMO_AGENT_LEDGER unreadable');
    await expect(page.locator('#capChip')).toHaveText(/Setup required\s*· agent ledger unreadable/);
    await expect(page.locator('#agents .empty.setup')).toContainText('Setup required');
    await expectNoHorizontalScroll(page);
    await page.screenshot({ path: test.info().outputPath('dash-setup-required.png') });
  });

  test('facilitator down: red state, and a pasted txid gets "Facilitator unavailable", nothing burned', async ({ page, watch }) => {
    watch.allow(/\/api\/quote$/, 402);
    watch.allow(/\/api\/redeem$/, 502);
    await page.goto(at(NO_FAC));
    await expect(page.locator('#facState')).toHaveText('unreachable');
    await expect(page.locator('#facState')).toHaveClass(/\bbad\b/);
    await expect(page.locator('#banners .banner')).toContainText('unreachable at http://127.0.0.1:4039');
    await sendRequest(page);
    await page.locator('#txid').fill(MINED_TXID);
    await page.getByRole('button', { name: 'Redeem' }).click();
    await expect(page.locator('#redeemOut')).toHaveText(/^Facilitator unavailable\. facilitator unreachable at http:\/\/127\.0\.0\.1:4039/);
    await expect(page.locator('#redeemForm button')).toBeEnabled();
    await expectNoHorizontalScroll(page);
    await page.screenshot({ path: test.info().outputPath('dash-facilitator-down.png') });
  });

  test('one-machine demo: the spendguard panel shows the real agent ledger and its denials', async ({ page }) => {
    await page.goto(at(DEMO_LEDGER));
    await expect(page.locator('#capChip')).toHaveText('1 agent');
    await expect(page.locator('#capNote')).toHaveText('of 0.0005 TAZ combined daily cap');
    const agent = page.locator('#agents .agent');
    await expect(agent.locator('.name')).toHaveText('e2e-agent');
    await expect(agent.locator('.host').first()).toHaveText('api.example.org');
    await expect(page.locator('#decCounts')).toHaveText('2 denied');
    await expect(page.locator('#audit li')).toHaveCount(2);
    await expect(page.locator('#audit')).toContainText('daily cap exceeded: spent 0 + requested 100000 > cap 50000 zat');
    await expect(page.locator('#audit')).toContainText("host '127.0.0.1' is not on the allowlist");
    await page.locator('#spendguard').scrollIntoViewIfNeeded();
    await expectNoHorizontalScroll(page);
    await page.locator('#spendguard').screenshot({ path: test.info().outputPath('dash-spendguard.png') });
  });

  test('DNS rebinding: under another host name the admin console answers 403; localhost still works', async ({ page, watch }) => {
    // Chrome resolves *.localhost to 127.0.0.1, so this is a page on a foreign name reaching the
    // loopback admin port, as a rebinding attack's page would.
    watch.allow(/kage-rebind\.localhost:4120/, 403);
    for (const path of ['', 'api/status', 'api/events?since=0']) {
      const r = await page.goto(`http://kage-rebind.localhost:${SERVER}/${path}`);
      expect(r?.status(), path).toBe(403);
      await expect(page.locator('body')).toContainText('DNS rebinding protection');
      await expect(page.locator('body')).not.toContainText('merchantUa');
    }
    await page.goto(`http://localhost:${SERVER}/`);
    await expect(page.locator('#facState')).toHaveText('reachable');
    await expectNoHorizontalScroll(page);
  });

  test('rate limited: a 429 shows "slow down" and never claims a 200', async ({ page, watch }) => {
    watch.allow(/\/api\/quote$/, 402);
    watch.allow(/\/api\/quote$/, 429);
    await page.goto(at(RATE_LIMITED));
    // The bucket holds one challenge per minute: at most two clicks reach the 429.
    let r = await sendRequest(page);
    if (r.status() !== 429) r = await sendRequest(page);
    expect(r.status()).toBe(429);
    const card = page.locator('#challenge');
    await expect(card).toContainText('429');
    await expect(card).toContainText(/too many payment challenges from this client/);
    await expect(card).toContainText(/Retry in \d+ s/);
    for (const s of [1, 2, 3, 4]) await expect(page.locator(`.step[data-s="${s}"]`)).not.toHaveClass(/\bdone\b/);
    await page.locator('#pay').screenshot({ path: test.info().outputPath('dash-429.png') });
  });

  test('reduced motion: new feed rows do not animate', async ({ page }) => {
    await page.emulateMedia({ reducedMotion: 'reduce' });
    await page.goto(at(SERVER));
    await sendRequest(page);
    const anim = await page.locator('#feed li.new').first().evaluate(el => getComputedStyle(el).animationName);
    expect(anim).toBe('none');
  });

  test('mobile layout: nav scrolls sideways inside its bar, the page does not', async ({ page }, info) => {
    test.skip(!isMobile(info.project.name), 'mobile only');
    await page.goto(at(SERVER));
    await sendRequest(page);
    await expect(page.locator('#challenge .uri')).toBeVisible();
    await expectNoHorizontalScroll(page);
    const box = await page.locator('#challenge').boundingBox();
    expect(box!.x + box!.width).toBeLessThanOrEqual(375);
    await page.screenshot({ path: test.info().outputPath('dash-mobile.png'), fullPage: true });
  });
});
