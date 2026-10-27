import { readFileSync } from 'node:fs';
import { test, expect, expectNoHorizontalScroll, isMobile } from './fixtures';

const URL_ = 'http://127.0.0.1:4050/zec-shielded-x402/';
// The long docs page (hero tape, spec, evidence, tests) moved off the landing to evidence.html.
const EVID = URL_ + 'evidence.html';
const README = readFileSync(new URL('../../README.md', import.meta.url), 'utf8');
// The ten verified txids, as the README's "Verified testnet runs" table lists them.
const README_TXIDS = [...README.matchAll(/^\| \[`([0-9a-f]{64})`\]\(https:\/\/testnet\.zecblock\.com\/tx\/\1\) \| (\d+) \|/gm)].map(m => ({ txid: m[1], block: m[2] }));
// The two payments examples/agent-chat made with the kage-x402 SDK on Oct 27 (to a merchant made with
// `kage wallet new`, so the demo UIVK of the table above doesn't see them).
const CHAT_TXIDS = ['401cad2c15c26dd26b2d3f17400318bbe73c39c35e04032037ed8ae33c1a77fa', '6c1229d115b82acbd0af4be834e6d5ba1c36703009758136713d0d50eb81d0d2'];
const tx = (id: string) => `https://testnet.zecblock.com/tx/${id}`;

test.describe('landing', () => {
  test('landing loads under the Pages path: the SDK headline, npm install with copy, npm and GitHub links, no horizontal scroll', async ({ page }) => {
    await page.goto(URL_);
    await expect(page).toHaveTitle(/Kage/);
    await expect(page.getByRole('heading', { level: 1 })).toHaveText('Private payments for AI agents.');
    await expect(page.locator('#npmi')).toHaveText('npm install kage-x402');
    const copy = page.locator('button[data-copy="#npmi"]');
    await copy.click();
    await expect(copy).toHaveClass(/\bdone\b/);
    expect(await page.evaluate(() => navigator.clipboard.readText())).toBe('npm install kage-x402');
    await expect(page.locator('a[href="https://www.npmjs.com/package/kage-x402"]').first()).toBeVisible();
    await expect(page.locator('.nav a[href="https://github.com/nickthelegend/zec-shielded-x402"]')).toBeVisible();
    await expectNoHorizontalScroll(page);
    await page.screenshot({ path: test.info().outputPath('landing-hero.png') });
    await page.goto(EVID);
    await expect(page).toHaveTitle(/Kage/);
    await expect(page.getByRole('heading', { level: 1 })).toHaveText('The spec, the runs, the evidence.');
    await expect(page.locator('.facts b').first()).toHaveText(String(README_TXIDS.length));
    await expectNoHorizontalScroll(page);
  });

  test('hero leads with the LLM-over-MCP run: its tool calls, txid f037d597 and payer ""', async ({ page }) => {
    await page.emulateMedia({ reducedMotion: 'reduce' });
    await page.goto(EVID);
    await expect(page.getByRole('tab', { name: /MCP/ })).toHaveAttribute('aria-selected', 'true');
    await expect(page.locator('#recDate')).toContainText('2026-10-22');
    await expect(page.locator('#recWhat')).toHaveText('LLM over MCP');
    await expect(page.locator('#footMcp')).toBeVisible();
    await expect(page.locator('#footMcp a')).toHaveAttribute('href', 'https://testnet.zecblock.com/tx/f037d5978fec7817340ff118d328995288f0a575a1850b083ca05f7d71d4e22a');
    await expect(page.locator('#footExchange')).toBeHidden();
    const calls = page.locator('#mcptape li');
    await expect(calls).toHaveCount(11);
    await expect(calls.nth(1)).toContainText('paid_fetch');
    await expect(calls.nth(1)).toContainText('"status":"settlement_pending"');
    await expect(calls.nth(1)).toContainText('f037d597');
    await expect(calls.nth(2)).toContainText('redeem_payment f037d597');
    await expect(calls.nth(2)).toContainText('"payer":""');
    await expect(calls.nth(2).locator('.c')).toHaveText('200');
    await expect(page.locator('#p0 a', { hasText: 'mcp-paid-run.jsonl' })).toHaveAttribute('href', 'https://github.com/nickthelegend/zec-shielded-x402/blob/main/docs/mcp-paid-run.jsonl');
    // the exchange tabs bring back their own run's header
    await page.getByRole('tab', { name: /402/ }).click();
    await expect(page.locator('#recDate')).toContainText('2026-10-21');
    await expect(page.locator('#recWhat')).toHaveText('GET /api/quote');
    await expect(page.locator('#footExchange')).toContainText('25a8b324');
    await expect(page.locator('#footMcp')).toBeHidden();
    await expectNoHorizontalScroll(page);
    await page.getByRole('tab', { name: /MCP/ }).click();
    await page.locator('.wire').screenshot({ path: test.info().outputPath('landing-mcp-tape.png') });
  });

  test('hero tabs MCP / 402 / X-PAYMENT / pending / 200: click, arrow keys, one panel at a time', async ({ page }) => {
    await page.emulateMedia({ reducedMotion: 'reduce' }); // no autoplay racing the clicks
    await page.goto(EVID);
    const tabs = page.getByRole('tab');
    await expect(tabs).toHaveCount(5);
    const panels = ['#p0', '#p1', '#p2', '#p3', '#p4'];
    for (let i = 0; i < 5; i++) {
      await tabs.nth(i).click();
      await expect(tabs.nth(i)).toHaveAttribute('aria-selected', 'true');
      for (let k = 0; k < 5; k++) await (k === i ? expect(page.locator(panels[k])).toBeVisible() : expect(page.locator(panels[k])).toBeHidden());
    }
    await expect(page.locator('#p4')).toContainText('HTTP/1.1 200 OK');
    await expect(page.locator('#p4')).toContainText('"payer":""');
    // Keyboard: ArrowRight wraps from 200 back to MCP, ArrowLeft goes back.
    await tabs.nth(4).focus();
    await page.keyboard.press('ArrowRight');
    await expect(tabs.nth(0)).toHaveAttribute('aria-selected', 'true');
    await expect(tabs.nth(0)).toBeFocused();
    await page.keyboard.press('ArrowLeft');
    await expect(tabs.nth(4)).toHaveAttribute('aria-selected', 'true');
    // The pending tape: ten settlement_pending retries, then the 200.
    await tabs.nth(3).click();
    await expect(page.locator('#tape li')).toHaveCount(11);
    await expect(page.locator('#tape li .c').filter({ hasText: '402' })).toHaveCount(10);
    await expect(page.locator('#tape li.win')).toContainText('"success":true');
    await page.screenshot({ path: test.info().outputPath('landing-tape.png') });
  });

  test('the recorded exchange autoplays once without reduced motion, and stays put with it', async ({ page }) => {
    await page.goto(EVID);
    await page.mouse.move(1, 1);
    await expect(page.getByRole('tab', { name: /402/ })).toHaveAttribute('aria-selected', 'true', { timeout: 6000 });
    await page.emulateMedia({ reducedMotion: 'reduce' });
    await page.reload();
    await page.waitForTimeout(5000);
    await expect(page.getByRole('tab', { name: /MCP/ })).toHaveAttribute('aria-selected', 'true');
    // Reduced motion also switches CSS animation off on the tape.
    await page.getByRole('tab', { name: /pending/ }).click();
    const anim = await page.locator('#tape li').first().evaluate(el => getComputedStyle(el).animationName);
    expect(anim).toBe('none');
  });

  test('copy buttons put the exact command on the clipboard', async ({ page }) => {
    await page.goto(EVID);
    const clone = page.locator('button[data-copy="#clone"]');
    await clone.click();
    await expect(clone).toHaveText('Copied');
    expect(await page.evaluate(() => navigator.clipboard.readText())).toBe(
      'git clone https://github.com/nickthelegend/zec-shielded-x402 && cd zec-shielded-x402 && cargo test -p x402z-server');
    await expect(clone).toHaveText('Copy', { timeout: 3000 });
    const qs = page.locator('button[data-copy="#qs"]');
    await qs.scrollIntoViewIfNeeded();
    await qs.click();
    await expect(qs).toHaveText('Copied');
    const text = await page.evaluate(() => navigator.clipboard.readText());
    expect(text.split('\n')).toEqual([
      'cargo test -p spendguard -p x402z-protocol -p x402z-server',
      'cargo test -p x402z-agent',
      'cargo test -p x402z-mcp',
      'cargo test -p x402z-facilitator',
      'cd packages/x402-zcash && npm test',
    ]);
  });

  test('quickstart test counts agree with the Tests table and the 139 headline', async ({ page }) => {
    await page.goto(EVID);
    const table = Object.fromEntries(await page.locator('#tests tbody tr').evaluateAll(rows =>
      rows.map(r => [r.querySelector('code')!.textContent, Number(r.querySelector('.num')!.textContent)])));
    const total = Object.values(table).reduce((a, b) => a + b, 0);
    expect(total).toBe(139);
    await expect(page.locator('.facts b').nth(1)).toHaveText(String(total));
    const qs = await page.locator('#qs').innerText();
    const nums = (crate: string) => (new RegExp(`-p ${crate}\\s+# ([\\d +]+)`).exec(qs)?.[1] || '').split('+').map(s => Number(s.trim()));
    expect(nums('x402z-server')).toEqual([table['spendguard'], table['x402z-protocol'], table['x402z-server']]);
    expect(nums('x402z-agent')[0]).toBe(table['x402z-agent']);
    expect(nums('x402z-mcp')[0]).toBe(table['x402z-mcp']);
    expect(nums('x402z-facilitator')[0]).toBe(table['x402z-facilitator']);
    expect(Number(/npm test\s+# (\d+)/.exec(qs)?.[1])).toBe(table['x402-zcash']);
  });

  test('evidence CTAs and top links jump to their sections', async ({ page }, info) => {
    await page.goto(EVID);
    await page.getByRole('link', { name: 'Live testnet evidence' }).click();
    await expect(page.locator('#h-ev')).toBeInViewport();
    await page.getByRole('link', { name: /^Quickstart/ }).first().click();
    await expect(page).toHaveURL(/#quickstart$/);
    await expect(page.locator('#h-quick')).toBeInViewport();
    const top = page.locator('.toplinks');
    if (!isMobile(info.project.name)) {
      await top.getByRole('link', { name: 'Evidence' }).click();
      await expect(page.locator('#h-ev')).toBeInViewport();
    } else {
      await expect(top.getByRole('link', { name: 'Evidence' })).toBeHidden();
    }
    await expect(top.getByRole('link', { name: 'Spec' })).toHaveAttribute('href', 'https://github.com/nickthelegend/zec-shielded-x402/blob/main/docs/spec-shielded-x402-agents.md');
    await expect(top.getByRole('link', { name: 'GitHub' })).toHaveAttribute('href', 'https://github.com/nickthelegend/zec-shielded-x402');
    // The brand mark goes back to the landing.
    await page.getByRole('link', { name: 'Kage home' }).click();
    await expect(page).toHaveURL(/\/zec-shielded-x402\/$/);
    await expect(page.locator('#h1')).toBeInViewport();
  });

  test('sidebar spec nav: every link lands its heading below the sticky bars and is highlighted', async ({ page }, info) => {
    await page.goto(EVID);
    const mobile = isMobile(info.project.name);
    const toc = page.locator('#toc');
    const ids = await toc.locator('a').evaluateAll(as => as.map(a => a.getAttribute('href')!.slice(1)));
    expect(ids).toHaveLength(13);
    for (const id of ids) {
      if (mobile) {
        await expect(toc).not.toHaveAttribute('open', '');
        await toc.locator('summary').click();
        await expect(toc).toHaveAttribute('open', '');
      }
      const link = toc.locator(`a[href="#${id}"]`);
      const label = (await link.textContent())!.trim();
      await link.click();
      await expect(page).toHaveURL(new RegExp(`#${id}$`));
      if (mobile) await expect(toc).not.toHaveAttribute('open', ''); // the drawer closes after a pick
      // The heading is visible, not tucked under the fixed top bar or the sticky "On this page" bar.
      await expect.poll(async () => page.evaluate(sec => {
        const h = document.querySelector(`#${sec} h2`)!.getBoundingClientRect();
        const bars = [document.querySelector('.topbar')!, ...(innerWidth <= 920 ? [document.querySelector('.side')!] : [])];
        const covered = Math.max(...bars.map(b => b.getBoundingClientRect().bottom));
        return h.top >= covered - 1 && h.top < innerHeight;
      }, id), { message: `#${id} heading not covered` }).toBe(true);
      // The picked section is the highlighted one, even near the end where it can't reach the top.
      await expect(link).toHaveClass(/\bon\b/);
      await expect(toc.locator('a.on')).toHaveCount(1);
      await expect(page.locator('#tocNow')).toHaveText(label);
    }
    await expectNoHorizontalScroll(page);
  });

  test('heading anchors and the skip link work', async ({ page }) => {
    await page.goto(EVID);
    await page.locator('#facilitator a.anchor').click();
    await expect(page).toHaveURL(/#facilitator$/);
    await page.goto(EVID);
    await page.keyboard.press('Tab');
    const skip = page.getByRole('link', { name: 'Skip to the spec' });
    await expect(skip).toBeFocused();
    await expect(skip).toBeInViewport();
    await page.keyboard.press('Enter');
    await expect(page).toHaveURL(/#content$/);
  });

  test('evidence table: ten explorer links, well-formed, matching the README and the label', async ({ page }) => {
    await page.goto(EVID + '#evidence');
    expect(README_TXIDS).toHaveLength(10);
    const rows = page.locator('#evidence tbody tr');
    await expect(rows).toHaveCount(10);
    for (let i = 0; i < 10; i++) {
      const a = rows.nth(i).locator('a');
      const href = (await a.getAttribute('href'))!;
      const m = /^https:\/\/testnet\.zecblock\.com\/tx\/([0-9a-f]{64})$/.exec(href);
      expect(m, `row ${i} href ${href}`).not.toBeNull();
      const txid = m![1];
      expect(txid).toBe(README_TXIDS[i].txid);
      // The label is the first 8 and last 4 hex of the same txid.
      await expect(a).toHaveText(`${txid.slice(0, 8)}…${txid.slice(-4)} ↗`);
      await expect(rows.nth(i).locator('td.num').first()).toHaveText(Number(README_TXIDS[i].block).toLocaleString('en-US'));
    }
    // Every other explorer link on the page is well-formed and names a txid from the table.
    const all = await page.locator('a[href*="zecblock"]').evaluateAll(as => as.map(a => a.getAttribute('href')!));
    for (const h of all) {
      const m = /^https:\/\/testnet\.zecblock\.com\/tx\/([0-9a-f]{64})$/.exec(h);
      expect(m, h).not.toBeNull();
      expect(README_TXIDS.map(t => t.txid)).toContain(m![1]);
    }
    await page.screenshot({ path: test.info().outputPath('landing-evidence.png') });
  });

  test('Kage Tab section and its transcript links', async ({ page }) => {
    await page.goto(EVID + '#tab');
    await expect(page.locator('#h-tab')).toBeInViewport();
    await expect(page.locator('#tab')).toContainText('RFC 9474 blind RSA');
    await expect(page.locator('#tab .term')).toContainText('nonce_already_used');
    await expect(page.locator('#tab a', { hasText: 'full transcript' })).toHaveAttribute('href', 'https://github.com/nickthelegend/zec-shielded-x402/blob/main/docs/tab-live-run.txt');
    await expect(page.locator('#mcp a', { hasText: 'full transcript' })).toHaveAttribute('href', 'https://github.com/nickthelegend/zec-shielded-x402/blob/main/docs/mcp-paid-run.txt');
    await expect(page.locator('#mcp a', { hasText: 'refusal transcript' })).toHaveAttribute('href', 'https://github.com/nickthelegend/zec-shielded-x402/blob/main/docs/mcp-live-run.txt');
    await expect(page.locator('#mcp .term')).toContainText('"success": true');
    await page.screenshot({ path: test.info().outputPath('landing-tab.png') });
  });

  test('no "log" control on the landing is broken (there is none) and footer links are well-formed', async ({ page }) => {
    await page.goto(EVID);
    await expect(page.getByRole('button', { name: /log/i })).toHaveCount(0);
    const foot = page.locator('footer nav a');
    await expect(foot).toHaveCount(4);
    await foot.filter({ hasText: 'Evidence' }).click();
    await expect(page).toHaveURL(/#evidence$/);
    for (const h of await foot.evaluateAll(as => as.map(a => a.getAttribute('href')!)))
      expect(h).toMatch(/^(#evidence|https:\/\/github\.com\/nickthelegend\/zec-shielded-x402(\/blob\/main\/(docs\/spec-shielded-x402-agents\.md|TESTING\.md))?)$/);
    await expectNoHorizontalScroll(page);
  });

  test('the install note links the hosted 402 over https', async ({ page }) => {
    await page.goto(EVID);
    const live = page.locator('.install-note a[href*="kage-api"]');
    await expect(live).toHaveCount(1);
    await expect(live).toHaveAttribute('href', 'https://kage-api.201-18-198-162.sslip.io/api/quote');
  });

  test('landing quickstart: agent.ts / server.ts / keys.sh tabs, arrow keys, the file tree, copy the open file', async ({ page }) => {
    await page.emulateMedia({ reducedMotion: 'reduce' });
    await page.goto(URL_);
    await page.getByRole('link', { name: /^Read the quickstart/ }).click();
    await expect(page).toHaveURL(/#quickstart$/);
    await expect(page.locator('#h-quick')).toBeInViewport();
    const tabs = page.getByRole('tab');
    await expect(tabs).toHaveText([/agent\.ts/, /server\.ts/, /keys\.sh/]);
    await expect(tabs.nth(0)).toHaveAttribute('aria-selected', 'true');
    await expect(page.locator('#pn-agent')).toContainText("dailyCap: '0.008 ZEC'");
    await expect(page.locator('#pn-agent .out')).toContainText('tx 401cad2c…77fa');
    await tabs.nth(1).click();
    await expect(page.locator('#pn-server')).toBeVisible();
    await expect(page.locator('#pn-agent')).toBeHidden();
    await expect(page.locator('#pn-server')).toContainText("pay('0.001 ZEC')");
    await page.keyboard.press('ArrowRight');
    await expect(tabs.nth(2)).toHaveAttribute('aria-selected', 'true');
    await expect(tabs.nth(2)).toBeFocused();
    await expect(page.locator('#pn-keys')).toContainText('kage wallet new --role merchant');
    await page.keyboard.press('ArrowRight'); // wraps to agent.ts
    await expect(tabs.nth(0)).toHaveAttribute('aria-selected', 'true');
    const copy = page.locator('button[data-copy-panel]');
    await copy.click();
    await expect(copy).toHaveClass(/\bdone\b/);
    const code = (await page.evaluate(() => navigator.clipboard.readText())).split('\n');
    expect(code).toHaveLength(13);
    expect(code[0]).toBe("import { createKage } from 'kage-x402/agent';");
    expect(code[12]).toBe("const res = await kage.fetch('https://api.example.com/v1/zec/price'); // 402 → policy → pay → 200");
    await expectNoHorizontalScroll(page);
    await page.locator('.editor').screenshot({ path: test.info().outputPath('landing-editor.png') });
  });

  test('landing reference: keys, policy, the seven error codes, facilitator endpoints, and a side nav that lands each one', async ({ page }) => {
    await page.goto(URL_);
    await expect(page.locator('#keys tbody tr')).toHaveCount(4);
    await expect(page.locator('#policy tbody tr td:first-child code')).toHaveText(['dailyCap', 'allowedHosts', 'approvalThreshold', 'allowInsecureHttp']);
    const codes = await page.locator('#errors tbody tr td:first-child').allInnerTexts();
    expect(codes.map(c => c.replace(/\s+/g, ' ').trim())).toEqual(['CAP_EXCEEDED', 'HOST_NOT_ALLOWED', 'APPROVAL_REQUIRED / APPROVAL_DENIED', 'INSUFFICIENT_FUNDS', 'PAYMENT_REJECTED', 'SETTLEMENT_TIMEOUT', 'SETUP_REQUIRED']);
    await expect(page.locator('#facilitator .ep-top code')).toHaveText(['/supported', '/verify', '/settle', '/health']);
    const side = page.locator('.side a');
    for (const id of ['keys', 'policy', 'errors', 'events', 'facilitator', 'production', 'more']) {
      const link = side.and(page.locator(`[href="#${id}"]`));
      await link.click();
      await expect(page).toHaveURL(new RegExp(`#${id}$`));
      await expect.poll(async () => page.evaluate(sec => {
        const h = document.querySelector(`#${sec} h2`)!.getBoundingClientRect();
        const bars = [document.querySelector('.nav')!, ...(innerWidth <= 860 ? [document.querySelector('.side')!] : [])];
        return h.top >= Math.max(...bars.map(b => b.getBoundingClientRect().bottom)) - 1 && h.top < innerHeight;
      }, id), { message: `#${id} heading not covered` }).toBe(true);
      await expect(link).toHaveClass(/\bon\b/);
    }
    await expectNoHorizontalScroll(page);
  });

  test('landing "See it run": the two Oct 27 txids on the explorer, the unedited session, the hosted 402', async ({ page }) => {
    await page.goto(URL_ + '#run');
    await expect(page.locator('#h-run')).toBeInViewport();
    for (const id of CHAT_TXIDS) await expect(page.locator(`a.tx[href="${tx(id)}"]`)).toContainText(id);
    const term = page.locator('#run .term pre');
    await expect(term).toContainText('paid 0.001 ZEC, shielded · tx 401cad2c…77fa');
    await expect(term).toContainText('Approve 0.005 ZEC to 127.0.0.1:4300?');
    await expect(term).toContainText("HOST_NOT_ALLOWED: spendguard refused the payment: host 'api.other-data.io' is not on the allowlist.");
    await expect(term).toContainText('CAP_EXCEEDED: spendguard refused the payment: daily cap exceeded: spent 600000 + requested 500000 > cap 800000 zat.');
    await expect(page.locator('#run a[href="https://kage-api.201-18-198-162.sslip.io/api/quote"]')).toHaveCount(1);
    // Every explorer link on the landing is well-formed and names a real Kage payment.
    for (const h of await page.locator('a[href*="zecblock"]').evaluateAll(as => as.map(a => a.getAttribute('href')!))) {
      const m = /^https:\/\/testnet\.zecblock\.com\/tx\/([0-9a-f]{64})$/.exec(h);
      expect(m, h).not.toBeNull();
      expect(CHAT_TXIDS).toContain(m![1]);
    }
    await expect(page.locator('.nav').getByRole('link', { name: 'Spec' }).or(page.locator('footer a[href="evidence.html"]')).first()).toHaveAttribute('href', 'evidence.html');
    await expectNoHorizontalScroll(page);
    await page.locator('#run').screenshot({ path: test.info().outputPath('landing-run.png') });
  });
});
