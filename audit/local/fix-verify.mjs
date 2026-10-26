// Re-run the nine original FAILs against the real local stack. No response interception,
// console filtering, fabricated receipts, wallet access, or transaction broadcasts.
import { chromium, expect } from '../../e2e/node_modules/@playwright/test/index.mjs';
import fs from 'node:fs';
import { fileURLToPath } from 'node:url';
import assert from 'node:assert/strict';
const out = new URL('./', import.meta.url);
const results = [], events = [], disconnected = [];
const txid = 'ccb4214c6a1fdf183c086ae5b4f559d181ead4c1c61dc7e711fc4a08b95b0ed3';
const norm = s => s.replace(/\s+/g, ' ').trim();
const text = async (page, selector) => norm(await page.locator(selector).innerText());
let browser, server;
const problems = es => es.filter(e => e.type === 'console.error' || e.type === 'pageerror' || e.type === 'requestfailed' && e.error !== 'net::ERR_ABORTED' || e.type === 'response' && e.status >= 400 && !e.expected);
async function open(width, port, path = '/') {
  const context = await browser.newContext({ viewport: { width, height: width === 375 ? 812 : 800 }, reducedMotion: 'reduce' });
  const page = await context.newPage(), local = [];
  page.setDefaultTimeout(15000);
  const record = e => { const item = { width, port, at: new Date().toISOString(), ...e }; local.push(item); events.push(item); };
  page.on('console', m => record({ type: `console.${m.type()}`, text: m.text(), location: m.location() }));
  page.on('pageerror', e => record({ type: 'pageerror', error: e.message }));
  page.on('requestfailed', r => record({ type: 'requestfailed', url: r.url(), method: r.method(), error: r.failure()?.errorText }));
  page.on('response', r => record({ type: 'response', url: r.url(), method: r.request().method(), status: r.status(), expected: r.url().endsWith('/api/quote') && [402,429].includes(r.status()) || r.url().endsWith('/api/redeem') && [422,502].includes(r.status()) }));
  const response = await page.goto(`http://127.0.0.1:${port}${path}`);
  assert.equal(response.status(), 200);
  if(port !== 4050) await expect(page.locator('#facState')).not.toHaveText('checking…');
  await page.waitForTimeout(250);
  return { page, context, local, width };
}
async function check(id, view, run, from = view.local.length) {
  let detail, assertionError;
  try { detail = await run(); } catch(e) { assertionError = e.stack; }
  await view.page.waitForTimeout(200);
  const errors = problems(view.local.slice(from));
  const result = { id, width: view.width, status: assertionError || errors.length ? 'FAIL' : 'PASS', behaviorMatches: !assertionError, detail, assertionError, errors };
  results.push(result); console.log(JSON.stringify(result));
}
async function shot(view, name) {
  const path = new URL(`fix-${name}-${view.width}.jpg`, out);
  await view.page.screenshot({ path: fileURLToPath(path), type: 'jpeg', quality: 60 });
  assert.ok(fs.statSync(path).size <= 200000);
  return fileURLToPath(path).split('/audit/')[1];
}
async function challenge(view) {
  const wait = view.page.waitForResponse(r => r.url().endsWith('/api/quote'));
  await view.page.locator('#try').click();
  const r = await wait;
  assert.equal(r.status(),402);
  const body = await r.json();
  await expect(view.page.locator('.status402')).toContainText('402 Payment Required');
  await expect(view.page.locator('#challenge')).toContainText('zcash-shielded · zcash-testnet');
  return body.accepts[0].extra.memoNonce;
}
async function redeem(view) {
  await view.page.locator('#txid').fill(txid);
  const wait = view.page.waitForResponse(r => r.url().endsWith('/api/redeem'));
  await view.page.locator('#redeemForm button').click();
  return await wait;
}
async function unspent(port, nonce) {
  // Read only the challenge row; never open receipt/token env files.
  const { execFileSync } = await import('node:child_process');
  const name = port === 4120 ? 'server' : 'server-nofac';
  const row = execFileSync('sqlite3', [`e2e/.run/${name}.sqlite`, `SELECT spent_txid IS NULL FROM challenges WHERE nonce='${nonce}';`], { encoding:'utf8' }).trim();
  assert.equal(row,'1');
  return true;
}
try {
  server = await chromium.launchServer({ channel:'chrome', headless:true });
  console.log(`Chrome PID ${server.process().pid}; cleanup is in finally`);
  browser = await chromium.connect(server.wsEndpoint());
  // Both empty-state checks run before issuing any challenge on the merchant server.
  for(const width of [1280,375]) {
    let view = await open(width,4050,'/zec-shielded-x402/');
    await check('U-011', view, async()=> {
      const links = await view.page.locator('.toplinks a').evaluateAll(es=>es.map(e=>[e.textContent.trim(),e.getAttribute('href')]));
      assert.deepEqual(links,[['Spec','evidence.html'],['GitHub','https://github.com/nickthelegend/zec-shielded-x402']]);
      return { links, screenshot:await shot(view,'landing') };
    },0);
    await view.context.close();
    view = await open(width,4120);
    await check('U-062',view,async()=> {
      const sections = await view.page.locator('section').evaluateAll(es=>es.map(e=>[e.id,e.querySelector('h2')?.textContent]));
      assert.deepEqual(sections,[['pay','Try a paid request'],['overview','Totals'],['activity','Payment feed'],['spendguard','Spendguard'],['keys','Keys']]);
      await view.page.locator('.nav a[href="#keys"]').click();
      await expect(view.page.locator('#keys')).toBeInViewport();
      const sizes = await view.page.evaluate(()=>[document.documentElement.scrollWidth,document.documentElement.clientWidth]);
      assert.equal(sizes[0],sizes[1]);
      return { sections, sizes, screenshot:await shot(view,'keys') };
    },0);
    await check('U-067',view,async()=> {
      await expect(view.page.locator('#paidTotal')).toHaveText('0 TAZ');
      assert.equal(await text(view.page,'#paidTotal'),'0 TAZ');
      const counts = await Promise.all(['#paidCount','#chCount','#rejCount'].map(s=>text(view.page,s)));
      assert.deepEqual(counts,['0','0','0']);
      await view.page.locator('.nav a[href="#overview"]').click();
      return { paidTotal:await text(view.page,'#paidTotal'), counts, screenshot:await shot(view,'empty-totals') };
    },0);
    await view.context.close();
  }
  for(const width of [1280,375]) {
    let view = await open(width,4120), nonce;
    await check('U-071',view,async()=> { nonce=await challenge(view); return {nonce,screenshot:await shot(view,'challenge')}; });
    // U-101 is only happy-path load + challenge, with the required 402 allowed as HTTP.
    await check('U-101',view,async()=> 'Happy-path load and 402 request completed; all console diagnostics retained.',0);
    await check('U-081',view,async()=> {
      const r=await redeem(view); assert.equal(r.status(),422);
      const body=await r.json(); expect(body).toMatchObject({status:'rejected',reason:'invalid_payment'});
      await expect(view.page.locator('#redeemOut')).toHaveClass(/\bbad\b/);
      await expect(view.page.locator('#redeemOut')).toHaveText('Refused. no output in this transaction decrypts to the merchant viewing key with this memo nonce');
      return {response:body,unspent:await unspent(4120,nonce),screenshot:await shot(view,'refusal')};
    });
    await view.context.close();
    view=await open(width,4130);
    await check('U-093',view,async()=> {
      nonce=await challenge(view);
      const r=await redeem(view); assert.equal(r.status(),502);
      const body=await r.json(); expect(body).toMatchObject({status:'error',reason:'facilitator_error'});
      await expect(view.page.locator('#redeemOut')).toHaveClass(/\bbad\b/);
      await expect(view.page.locator('#redeemOut')).toHaveText(/^Facilitator unavailable\. facilitator unreachable at http:\/\/127\.0\.0\.1:4039/);
      await expect(view.page.locator('#redeemForm button')).toBeEnabled();
      return {response:body,unspent:await unspent(4130,nonce),screenshot:await shot(view,'facilitator-down')};
    });
    await view.context.close();
    view=await open(width,4142);
    await check('U-095',view,async()=> {
      let r;
      for(let i=0;i<2;i++) {
        const wait=view.page.waitForResponse(r=>r.url().endsWith('/api/quote'));
        await view.page.locator('#try').click(); r=await wait;
        if(r.status()===429)break;
      }
      assert.equal(r.status(),429); const body=await r.json();
      for(const want of ['429','Slow down.','too many payment challenges from this client']) await expect(view.page.locator('#challenge')).toContainText(want);
      await expect(view.page.locator('#challenge')).toContainText(/Retry in \d+ s/);
      for(const s of [1,2,3,4]) await expect(view.page.locator(`.step[data-s="${s}"]`)).not.toHaveClass(/done/);
      return {response:body,screenshot:await shot(view,'rate-limit')};
    });
    await view.context.close();
    // Load both views before stopping the real server for U-096.
    disconnected.push(await open(width,4120));
  }
  const pidLine=fs.readFileSync('e2e/.run/pids','utf8').split('\n').find(s=>s.startsWith('server '));
  const pid=Number(pidLine.split(' ')[1]); assert.ok(pid>1);
  process.kill(pid,'SIGTERM');
  console.log(`Stopped real merchant server PID ${pid} for U-096; stack.sh stop cleans remaining processes.`);
  await new Promise(resolve=>setTimeout(resolve,500));
  for(const view of disconnected) {
    await check('U-096',view,async()=> {
      await view.page.locator('#try').click();
      await expect(view.page.locator('#banners .banner')).toHaveAttribute('role','alert');
      await expect(view.page.locator('#banners')).toContainText('x402z-server unreachable:');
      return {fault:'real server stopped by recorded PID; no request interception',banner:await text(view.page,'#banners'),screenshot:await shot(view,'unreachable')};
    });
    await view.context.close();
  }
} finally {
  if(browser)await browser.close();
  if(server){const pid=server.process().pid;await server.close();console.log(`Chrome PID ${pid} closed`);}
  fs.writeFileSync(new URL('fix-results.json',out),JSON.stringify(results,null,2)+'\n');
  fs.writeFileSync(new URL('fix-events.json',out),JSON.stringify(events,null,2)+'\n');
}
