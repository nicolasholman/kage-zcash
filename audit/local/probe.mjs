// Supplemental local AUDIT.md checks. No wallet access, keys, or payment broadcasts.
// Run from the repository root: node audit/local/probe.mjs [initial|main]
import { chromium, expect } from '../../e2e/node_modules/@playwright/test/index.mjs';
import fs from 'node:fs';
import assert from 'node:assert/strict';
import { fileURLToPath } from 'node:url';

const mode = process.argv[2] || 'main';
const output = new URL('./', import.meta.url);
const records = [], events = [];
const github = 'https://github.com/nickthelegend/zec-shielded-x402';
const live = 'https://kage-api.201-18-198-162.sslip.io/api/quote';
const clone = `git clone ${github} && cd zec-shielded-x402 && cargo test -p x402z-server`;
const root = 'http://127.0.0.1:4050/zec-shielded-x402/';
let browserServer, browser;
const norm = s => s.replace(/\s+/g, ' ').trim();
async function check(id, viewport, fn) {
  const start = events.length;
  try {
    const detail = await fn();
    const errors = events.slice(start).filter(e => e.type === 'pageerror' || e.type === 'console.error' || (e.type === 'requestfailed' && e.error !== 'net::ERR_ABORTED') || (e.type === 'response' && e.status >= 400 && !e.expected));
    records.push({ id, viewport, status: errors.length ? 'FAIL' : 'PASS', detail: detail ?? 'Assertions matched', errors });
  } catch (e) {
    records.push({ id, viewport, status: 'FAIL', detail: e.message.slice(0, 3500) });
  }
  console.log(JSON.stringify(records.at(-1)));
}
async function open(width, url, reducedMotion = 'reduce') {
  const context = await browser.newContext({ viewport: { width, height: width === 375 ? 812 : 800 }, permissions: ['clipboard-read', 'clipboard-write'], reducedMotion });
  await context.addInitScript(() => {
    window.__auditMotion = [];
    document.addEventListener('DOMContentLoaded', () => {
      const started = performance.now();
      const rows = [...document.querySelectorAll('.shadow li')];
      for (const [index,row] of rows.entries()) new MutationObserver(() => {
        if(row.classList.contains('in')) window.__auditMotion.push({index,ms:performance.now()-started});
      }).observe(row,{attributes:true,attributeFilter:['class']});
    });
  });
  const page = await context.newPage();
  page.setDefaultTimeout(5000);
  const localEvents = [];
  const record = e => { const item = { viewport: width, page: url, ...e }; events.push(item); localEvents.push(item); };
  page.on('console', m => record({ type: `console.${m.type()}`, text: m.text(), url: m.location().url }));
  page.on('pageerror', e => record({ type: 'pageerror', error: e.message }));
  page.on('requestfailed', r => record({ type: 'requestfailed', method: r.method(), url: r.url(), error: r.failure()?.errorText }));
  page.on('response', r => record({ type: 'response', method: r.request().method(), url: r.url(), status: r.status(), expected: /\/api\/quote$/.test(r.url()) && [402,429].includes(r.status()) || /\/api\/redeem$/.test(r.url()) && [422,502].includes(r.status()) }));
  const response = await page.goto(url);
  await page.waitForTimeout(350);
  return { page, context, response, localEvents };
}
async function shot(page, name) {
  const path = fileURLToPath(new URL(`${name}.jpg`, output));
  await page.screenshot({ path, type: 'jpeg', quality: 65 });
  assert.ok(fs.statSync(path).size <= 200000, 'Screenshot exceeds 200 KB');
  return `audit/local/${name}.jpg`;
}
const text = async (page, sel) => norm(await page.locator(sel).innerText());
const texts = async (page, sel) => (await page.locator(sel).allInnerTexts()).map(norm);
async function noLog(page) {
  const controls = await page.locator('button,input,select,textarea,a,[role="button"]').evaluateAll(es => es.filter(e => /log/i.test(e.getAttribute('aria-label') || e.innerText || e.getAttribute('placeholder') || '')).map(e => e.outerHTML));
  assert.deepEqual(controls, []);
}
async function noOverflow(page) {
  const sizes = await page.evaluate(() => ({ scroll: document.documentElement.scrollWidth, client: document.documentElement.clientWidth }));
  assert.equal(sizes.scroll, sizes.client); return sizes;
}
function clean(es) {
  const problems = es.filter(e => e.type === 'console.error' || e.type === 'pageerror' || e.type === 'requestfailed' && e.error !== 'net::ERR_ABORTED' || e.type === 'response' && e.status >= 400 && !e.expected);
  assert.deepEqual(problems, []);
}
try {
  browserServer = await chromium.launchServer({ channel: 'chrome', headless: true });
  fs.writeFileSync(new URL(`browser-${mode}.pid`, output), `${browserServer.process().pid}\n`);
  browser = await chromium.connect(browserServer.wsEndpoint());
  console.log(`Chrome PID ${browserServer.process().pid}; ${browser.version()}`);
  for (const width of [1280,375]) {
    if (mode === 'initial') {
      const {page,context,localEvents} = await open(width,'http://127.0.0.1:4120/');
      await expect(page.locator('#facState')).toHaveText('reachable');
      await check('U-066',width,async()=>{ assert.equal(await text(page,'#challenge .ei'),'no response yet'); assert.equal(await text(page,'#challenge b'),'Send the request'); assert.equal(await page.locator('.step.done,.step.on').count(),0); });
      await check('U-067',width,async()=>{ assert.equal(await text(page,'#paidTotal'),'0 TAZ'); for(const s of ['#paidCount','#chCount','#rejCount']) assert.equal(await text(page,s),'0'); });
      await check('U-070',width,async()=>{ await expect(page.locator('#feed')).toBeEmpty(); await expect(page.locator('#feedEmpty')).toBeVisible(); assert.equal(await text(page,'#feedEmpty .ei'),'empty'); assert.equal(await text(page,'#feedEmpty b'),'No requests yet'); });
      await check('U-101',width,async()=>{clean(localEvents); return await shot(page,`dashboard-empty-${width}`);});
      await context.close(); continue;
    }
    let view = await open(width,root), {page,context} = view;
    await check('U-001',width,async()=>{assert.equal(view.response.status(),200); assert.equal(await page.title(),'Kage — shielded x402 for AI agents'); return 'Local half only; live host assigned separately';});
    await check('U-002',width,async()=>{assert.equal(await page.locator('h1').count(),1); assert.equal(await text(page,'h1'),'Agents pay per call. Nobody learns who paid.'); assert.equal(await text(page,'h1 mark'),'who paid');});
    await check('U-003',width,async()=>{assert.equal(await text(page,'.eyebrow'),'x402 v1 · zcash-testnet'); assert.equal(await text(page,'.sub'),'HTTP 402 settled in shielded Zcash: spendguard checks each payment, the merchant verifies with a viewing key alone.');});
    await check('U-004',width,async()=>{ const rows = await texts(page,'.shadow li'); assert.equal(rows.length,5); for(const [i,s] of ['GET /api/quote','402 Payment Required','pay shielded transaction, nonce in the encrypted memo','GET /api/quote + X-PAYMENT','200 OK · "payer": ""'].entries()) assert.ok(rows[i].includes(s),JSON.stringify(rows)); return rows; });
    await check('U-005',width,async()=>{assert.deepEqual(await texts(page,'.veil span'),['amount','memo','from','to']); await expect(page.locator('.veil')).toHaveAttribute('aria-label','On the public chain the amount, memo, sender and receiver are hidden');});
    await check('U-007',width,async()=>{assert.equal(await page.locator('.shadow li.in').count(),5); assert.deepEqual(await page.locator('.shadow li').evaluateAll(es=>es.map(e=>[getComputedStyle(e).animationName,getComputedStyle(e).transitionProperty])),Array(5).fill(['none','none']));});
    await check('U-008',width,async()=>{await expect(page.locator('.hero a.btn-ink')).toHaveAttribute('href','evidence.html#quickstart'); await expect(page.locator('.hero a.btn-ink')).toContainText('Quickstart');});
    await check('U-009',width,async()=>{await expect(page.locator('.hero a.btn-line')).toHaveAttribute('href',live); assert.equal(await text(page,'.hero a.btn-line'),'Try the live 402');});
    await check('U-010',width,async()=>{await expect(page.locator('a.mark')).toHaveAttribute('href','#top'); await expect(page.locator('a.mark')).toHaveAttribute('aria-label','Kage home'); assert.equal(await text(page,'a.mark'),'Kage 影'); await page.locator('a.mark').click(); assert.equal(new URL(page.url()).hash,'#top');});
    await check('U-011',width,async()=>{ assert.deepEqual(await page.locator('.toplinks a').evaluateAll(es=>es.map(e=>[e.textContent.trim(),e.getAttribute('href')])),[['Spec','evidence.html'],['GitHub',github]]); });
    await check('U-012',width,async()=>{assert.equal(await page.locator('section[aria-labelledby="h-how"] ol.steps li').count(),3); assert.deepEqual(await texts(page,'.steps h3'),['Ask','Pay','Retry']); return '01/02/03 are CSS counters on the three list items';});
    await check('U-013',width,async()=>{assert.deepEqual(await texts(page,'section[aria-labelledby="h-proof"] .facts b'),['10','139','0']); assert.deepEqual(await texts(page,'.facts span'),['PAYMENTS','TESTS','SPEND KEYS']);});
    await check('U-014',width,async()=>{await expect(page.locator('a.more')).toHaveAttribute('href','evidence.html#evidence'); await expect(page.locator('a.more')).toContainText('See the evidence');});
    await check('U-015',width,async()=>{assert.deepEqual(await page.locator('section[aria-labelledby="h-live"] a').evaluateAll(es=>es.map(e=>e.href)),[live,'https://kage-facilitator.201-18-198-162.sslip.io/health']);});
    await check('U-016',width,async()=>{assert.equal(await text(page,'.strip .setup h3'),'SETUP REQUIRED'); assert.equal(await text(page,'.strip .setup p'),'Paying it needs your own funded testnet wallet. Mainnet is not supported.');});
    await check('U-017',width,async()=>assert.equal(await text(page,'code#clone'),clone));
    await check('U-018',width,async()=>{const b=page.locator('button[data-copy="#clone"]'); await b.click(); await expect(b).toHaveText('Copied'); assert.equal(await page.evaluate(()=>navigator.clipboard.readText()),clone); await expect(b).toHaveText('Copy',{timeout:2500});});
    await check('U-019',width,async()=>{await expect(page.locator('.end a.btn')).toHaveAttribute('href',github); assert.equal(await text(page,'.end a.btn'),'View on GitHub');});
    await check('U-020',width,async()=>assert.deepEqual(await page.locator('footer nav a').evaluateAll(es=>es.map(e=>e.getAttribute('href'))),['evidence.html',`${github}/blob/main/TESTING.md`,github]));
    await check('U-021',width,()=>noLog(page));
    await page.evaluate(()=>scrollTo(0,0));
    await check(width===375?'U-023':'U-022',width,async()=>{await noOverflow(page); if(width===375){assert.equal(await page.locator('.topbar').evaluate(e=>e.getBoundingClientRect().height),60); await expect(page.locator('.toplinks a.hide-sm')).toBeHidden(); assert.equal(await page.locator('.hero .wrap').evaluate(e=>getComputedStyle(e).gridTemplateColumns.split(' ').length),1);} return await shot(page,`landing-${width}`);});
    await check('U-024',width,()=>clean(view.localEvents));
    await context.close();

    view=await open(width,root,'no-preference'); ({page,context}=view);
    await check('U-006',width,async()=>{ await expect(page.locator('.shadow li.in')).toHaveCount(5); const times=await page.evaluate(()=>window.__auditMotion); assert.deepEqual(times.map(t=>t.index),[0,1,2,3,4]); assert.ok(Math.abs(times[0].ms-250)<150); for(let i=1;i<5;i++)assert.ok(Math.abs(times[i].ms-times[i-1].ms-420)<150); return {times,toleranceMs:150}; });
    await context.close();

    view=await open(width,root+'evidence.html'); ({page,context}=view);
    await check('U-025',width,async()=>{assert.equal(view.response.status(),200); assert.equal(await page.title(),'Kage — spec and evidence'); return 'Local half only';});
    await check('U-026',width,async()=>{assert.equal(await page.locator('h1').count(),1); assert.equal(await text(page,'h1'),'The spec, the runs, the evidence.'); assert.equal(await text(page,'h1 mark'),'evidence');});
    await check('U-027',width,async()=>{await expect(page.locator('a.skip')).toHaveAttribute('href','#content'); await page.keyboard.press('Tab'); await expect(page.locator('a.skip')).toBeFocused(); assert.equal(await page.locator('a.skip').evaluate(e=>getComputedStyle(e).top),'10px'); await page.keyboard.press('Enter'); assert.equal(new URL(page.url()).hash,'#content');});
    await check('U-028',width,async()=>{await expect(page.locator('a.mark')).toHaveAttribute('href','./'); await expect(page.locator('a.mark')).toHaveAttribute('aria-label','Kage home'); assert.equal(await text(page,'a.mark'),'Kage 影');});
    await check('U-029',width,async()=>assert.deepEqual(await page.locator('.toplinks a').evaluateAll(es=>es.map(e=>[e.textContent.trim(),e.getAttribute('href')])),[['Quickstart','#quickstart'],['Evidence','#evidence'],['Spec',`${github}/blob/main/docs/spec-shielded-x402-agents.md`],['GitHub',github]]));
    await check('U-030',width,async()=>{assert.deepEqual(await texts(page,'.facts b'),['10','139','0']); return await texts(page,'.facts');});
    await check('U-034',width,async()=>{await expect(page.locator('.tabs')).toHaveAttribute('role','tablist'); assert.equal(await page.locator('.tabs button[role="tab"]').count(),5); return await texts(page,'.tabs button');});
    await check('U-038',width,async()=>{await page.getByRole('tab',{name:/402/}).click(); await expect(page.locator('#p1')).toBeVisible(); await expect(page.locator('#p1')).toContainText('100000'); await expect(page.locator('#p1')).toContainText('x402z-696c58afd509cc0f8753f3b0');});
    await check('U-039',width,async()=>{await page.getByRole('tab',{name:'X-PAYMENT'}).click(); await expect(page.locator('#p2')).toBeVisible(); await expect(page.locator('#p2')).toContainText('25a8b324bebee30220382113c6fb9cc3be1144b036117c34778bde3b1d8b689d'); await expect(page.locator('#p2')).toContainText('X-PAYMENT'); return await text(page,'#p2');});
    await check('U-041',width,async()=>{await page.getByRole('tab').nth(4).click(); await expect(page.locator('#p4')).toContainText('HTTP/1.1 200 OK'); await expect(page.locator('#p4')).toContainText('"success":true'); await expect(page.locator('#p4')).toContainText('"payer":""'); await expect(page.locator('#p4')).toContainText('100000');});
    await check('U-046',width,async()=>assert.deepEqual(await page.locator('#toc a').evaluateAll(es=>es.map(e=>e.getAttribute('href'))),['#overview','#quickstart','#the-402','#spendguard','#x-payment','#facilitator','#settlement','#zip321','#mcp','#tab','#leaks','#evidence','#tests']));
    await check('U-054',width,async()=>assert.deepEqual(await page.locator('footer nav a').evaluateAll(es=>es.map(e=>e.getAttribute('href'))),[github,`${github}/blob/main/docs/spec-shielded-x402-agents.md`,`${github}/blob/main/TESTING.md`,'#evidence']));
    await check('U-055',width,()=>noLog(page));
    await check(width===375?'U-057':'U-056',width,async()=>{await noOverflow(page); if(width===375){await expect(page.locator('#toc summary')).toBeVisible(); assert.equal(await page.locator('.tbl').first().evaluate(e=>getComputedStyle(e).overflowX),'auto');} await page.evaluate(()=>scrollTo(0,0)); return await shot(page,`evidence-${width}`);});
    await check('U-058',width,()=>clean(view.localEvents));
    await context.close();

    view=await open(width,'http://127.0.0.1:4120/'); ({page,context}=view);
    await expect(page.locator('#facState')).toHaveText('reachable');
    await check('U-061',width,async()=>{assert.equal(await text(page,'.crumb'),'x402z-server / merchant console'); assert.equal(await text(page,'h1'),'Merchant console'); assert.equal(await text(page,'.pill'),'GET /api/quote');});
    await check('U-062',width,async()=>assert.deepEqual(await page.locator('section').evaluateAll(es=>es.map(e=>[e.id,e.querySelector('h2').textContent])),[['pay','Try a paid request'],['overview','Totals'],['activity','Payment feed'],['spendguard','Spendguard'],['keys','Keys']]));
    await check('U-063',width,async()=>{assert.equal(await text(page,'#net'),'zcash-testnet'); assert.equal(await text(page,'#facState'),'reachable'); assert.equal(await page.locator('#facState').getAttribute('class'),'dot ok'); assert.equal(await text(page,'#price'),'0.001 TAZ');});
    await check('U-065',width,async()=>{await expect(page.locator('#banners')).toBeEmpty(); assert.equal(await page.locator('#banners').evaluate(e=>getComputedStyle(e).display),'none');});
    await check('U-068',width,async()=>{assert.equal(await text(page,'#spent .fade'),'—'); assert.equal(await text(page,'#capChip'),'Merchant view · payer ledger not shown'); assert.equal(await page.locator('#capBar').evaluate(e=>e.style.width),'0%');});
    await check('U-069',width,async()=>{await expect(page.locator('#agents')).toContainText('Payer ledger not shown'); await expect(page.locator('#agents')).toContainText('Spendguard runs on the agent, and a merchant never sees its spending.'); await expect(page.locator('#audit')).toBeEmpty(); await expect(page.locator('#decCounts')).toBeEmpty();});
    let response;
    await check('U-071',width,async()=>{ const waiting=page.waitForResponse(r=>r.url().endsWith('/api/quote')); await page.locator('#try').click(); response=await waiting; assert.equal(response.status(),402); await expect(page.locator('.status402')).toContainText('402 Payment Required'); await expect(page.locator('#challenge')).toContainText('zcash-shielded · zcash-testnet'); });
    await check('U-073',width,async()=>{await expect(page.locator('#ttl')).toHaveText(/^expires in 1[45]:\d\d$/); const before=await text(page,'#ttl'); await page.waitForTimeout(1200); assert.notEqual(await text(page,'#ttl'),before);});
    await check('U-076',width,async()=>{for(const i of [0,1])await expect(page.locator(`.step[data-s="${i}"]`)).toHaveClass('step done'); await expect(page.locator('.step[data-s="2"]')).toHaveClass('step on'); await expect(page.locator('.step[data-s="4"]')).not.toHaveClass(/done/);});
    let posts=0; page.on('request',r=>{if(r.url().endsWith('/api/redeem')) posts++;});
    await check('U-079',width,async()=>{await expect(page.locator('#txid')).toHaveAttribute('required',''); await page.locator('#redeemForm button').click(); assert.equal(await page.locator('#txid').evaluate(e=>e.validity.valueMissing),true); await page.waitForTimeout(200); assert.equal(posts,0);});
    await check('U-080',width,async()=>{await expect(page.locator('#txid')).toHaveAttribute('pattern','[0-9a-fA-F]{64}'); await page.locator('#txid').fill('not-a-txid'); await page.locator('#redeemForm button').click(); assert.equal(await page.locator('#txid').evaluate(e=>e.validity.patternMismatch),true); assert.equal(posts,0); await expect(page.locator('#redeemOut')).toBeEmpty(); return await shot(page,`invalid-txid-${width}`);});
    await check('U-081',width,async()=>{await page.locator('#txid').fill('ccb4214c6a1fdf183c086ae5b4f559d181ead4c1c61dc7e711fc4a08b95b0ed3');const waiting=page.waitForResponse(r=>r.url().endsWith('/api/redeem'));await page.locator('#redeemForm button').click();const r=await waiting;assert.equal(r.status(),422);expect(await r.json()).toMatchObject({status:'rejected',reason:'invalid_payment'});await expect(page.locator('#redeemOut')).toHaveClass(/\bbad\b/);await expect(page.locator('#redeemOut')).toHaveText('Refused. no output in this transaction decrypts to the merchant viewing key with this memo nonce');return await shot(page,`recorded-refusal-${width}`);});
    await check('U-090',width,()=>noLog(page));
    await check(width===375?'U-100':'U-099',width,async()=>{await noOverflow(page); if(width===1280){assert.equal(await page.locator('.side').evaluate(e=>e.getBoundingClientRect().width),232); assert.equal(await page.locator('.side').evaluate(e=>e.getBoundingClientRect().x),0);} else {assert.equal(await page.locator('.side').evaluate(e=>getComputedStyle(e).position),'sticky'); const b=await page.locator('#challenge').boundingBox(); assert.ok(b.x+b.width<=375); assert.equal(await page.locator('.nav').evaluate(e=>getComputedStyle(e).overflowX),'auto');} await page.evaluate(()=>scrollTo(0,0)); return await shot(page,`dashboard-card-${width}`);});
    await check('U-101',width,()=>clean(view.localEvents));
    await check('U-096',width,async()=>{await page.route('**/api/quote',r=>r.abort('connectionrefused')); await page.locator('#try').click(); await expect(page.locator('#banners .banner')).toHaveAttribute('role','alert'); await expect(page.locator('#banners')).toContainText('x402z-server unreachable:'); return 'Injected connection refusal; no server killed, no payment sent';});
    await context.close();

    for(const [port,id] of [[4130,'U-092'],[4141,'U-091'],[4142,'U-095']]) {
      view=await open(width,`http://127.0.0.1:${port}/`); ({page,context}=view);
      await page.waitForTimeout(400);
      await check(id,width,async()=>{
        if(port===4130){await expect(page.locator('#facState')).toHaveText('unreachable'); await expect(page.locator('#facState')).toHaveClass('dot bad'); await expect(page.locator('#banners .banner')).toHaveAttribute('role','alert'); await expect(page.locator('#banners')).toContainText('unreachable at http://127.0.0.1:4039');}
        if(port===4141){await expect(page.locator('#banners .banner')).toHaveAttribute('role','alert'); await expect(page.locator('#banners')).toContainText('X402Z_DEMO_AGENT_LEDGER unreadable'); assert.equal(await text(page,'#capChip'),'Setup required · agent ledger unreadable'); await expect(page.locator('#agents .empty.setup')).toContainText('Setup required'); await expect(page.locator('#agents .empty.setup')).toContainText('Agent ledger unreadable');}
        if(port===4142){for(let i=0;i<2;i++){const wait=page.waitForResponse(r=>r.url().endsWith('/api/quote')); await page.locator('#try').click(); const r=await wait;if(r.status()===429)break;} for(const t of ['429','Slow down.','too many payment challenges from this client'])await expect(page.locator('#challenge')).toContainText(t); await expect(page.locator('#challenge')).toContainText(/Retry in \d+ s/); for(const i of [1,2,3,4])await expect(page.locator(`.step[data-s="${i}"]`)).not.toHaveClass(/done/);}
        await noOverflow(page); return await shot(page,`state-${port}-${width}`);
      });
      if(port===4130)await check('U-093',width,async()=>{let waiting=page.waitForResponse(r=>r.url().endsWith('/api/quote'));await page.locator('#try').click();assert.equal((await waiting).status(),402);await page.locator('#txid').fill('ccb4214c6a1fdf183c086ae5b4f559d181ead4c1c61dc7e711fc4a08b95b0ed3');waiting=page.waitForResponse(r=>r.url().endsWith('/api/redeem'));await page.locator('#redeemForm button').click();assert.equal((await waiting).status(),502);await expect(page.locator('#redeemOut')).toHaveText(/^Facilitator unavailable\. facilitator unreachable at http:\/\/127\.0\.0\.1:4039/);await expect(page.locator('#redeemOut')).toHaveClass(/bad/);await expect(page.locator('#redeemForm button')).toBeEnabled();await expect(page.locator('#ttl')).toHaveText(/^expires in/);await expect(page.locator('.step[data-s="4"]')).not.toHaveClass(/done/);return await shot(page,`facilitator-down-redeem-${width}`);});
      await context.close();
    }
  }
} finally {
  if(browser)await browser.close();
  if(browserServer){ await browserServer.close(); console.log(`Chrome PID ${browserServer.process().pid} stopped`); }
  fs.writeFileSync(new URL(`probe-${mode}-results.json`,output),JSON.stringify(records,null,2)+'\n');
  fs.writeFileSync(new URL(`probe-${mode}-events.json`,output),JSON.stringify(events,null,2)+'\n');
}
