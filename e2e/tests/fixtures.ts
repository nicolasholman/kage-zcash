import { test as base, expect, type Page } from '@playwright/test';

type Allowed = { url: RegExp; status: number };

/**
 * Every test gets a `watch` that records console errors, failed requests and same-origin HTTP
 * errors, and asserts at the end that there were none. A test may `allow()` a specific status on a
 * specific URL when that status is the product's honest answer (the 402 itself, a 422 refusal, a
 * 502 when the facilitator is down, a 429). Nothing else is excused.
 */
export const test = base.extend<{ watch: { allow: (url: RegExp, status: number) => void } }>({
  watch: [
    async ({ page }, use, testInfo) => {
      const allowed: Allowed[] = [];
      const problems: string[] = [];
      const isAllowed = (url: string, status: number) => allowed.some(a => a.status === status && a.url.test(url));
      const sameOrigin = (url: string) => {
        try { return new URL(url).origin === new URL(page.url()).origin; } catch { return false; }
      };
      page.on('pageerror', e => problems.push(`pageerror: ${e.message}`));
      page.on('console', m => {
        if (m.type() !== 'error') return;
        const loc = m.location().url || '';
        const st = /status of (\d{3})/.exec(m.text());
        if (st && isAllowed(loc, Number(st[1]))) return;
        problems.push(`console.error: ${m.text()} @ ${loc}`);
      });
      page.on('requestfailed', r => {
        if (!sameOrigin(r.url())) return;
        const why = r.failure()?.errorText || '';
        // A navigation away aborts in-flight polls; that's not a product failure.
        if (why === 'net::ERR_ABORTED') return;
        problems.push(`requestfailed: ${r.method()} ${r.url()} ${why}`);
      });
      page.on('response', r => {
        if (!sameOrigin(r.url()) || r.status() < 400) return;
        if (isAllowed(r.url(), r.status())) return;
        problems.push(`HTTP ${r.status()}: ${r.request().method()} ${r.url()}`);
      });
      await use({ allow: (url, status) => allowed.push({ url, status }) });
      if (problems.length) await testInfo.attach('problems', { body: problems.join('\n') });
      expect(problems, 'console errors / failed same-origin requests').toEqual([]);
    },
    { auto: true },
  ],
});

export { expect };

/** No horizontal page scroll (the 375 px check). */
export async function expectNoHorizontalScroll(page: Page) {
  const { sw, cw } = await page.evaluate(() => ({ sw: document.documentElement.scrollWidth, cw: document.documentElement.clientWidth }));
  expect(sw, `page scrollWidth ${sw} > clientWidth ${cw}`).toBeLessThanOrEqual(cw);
}

/** The element's top is inside the viewport (an anchor jump landed). */
export async function expectInViewport(page: Page, selector: string) {
  await expect.poll(async () => page.evaluate(sel => {
    const r = document.querySelector(sel)!.getBoundingClientRect();
    return r.top >= -2 && r.top < innerHeight * 0.6;
  }, selector), { message: `${selector} scrolled into view` }).toBe(true);
}

export const isMobile = (name: string) => name === 'mobile';
