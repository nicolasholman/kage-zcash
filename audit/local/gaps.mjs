// Remaining browser assertions, on the same installed Google Chrome channel as e2e.
import { chromium, expect } from '../../e2e/node_modules/@playwright/test/index.mjs';
import assert from 'node:assert/strict';
import fs from 'node:fs';
const results=[],events=[];
const server=await chromium.launchServer({channel:'chrome',headless:true});
fs.writeFileSync(new URL('browser-gaps.pid',import.meta.url),`${server.process().pid}\n`);
const browser=await chromium.connect(server.wsEndpoint());
const root='http://127.0.0.1:4050/zec-shielded-x402/evidence.html';
async function check(id,width,fn){try{const detail=await fn();results.push({id,viewport:width,status:'PASS',detail:detail??'Assertions matched'});}catch(e){results.push({id,viewport:width,status:'FAIL',detail:e.message.slice(0,2500)});}console.log(JSON.stringify(results.at(-1)));}
try{
  for(const width of [1280,375]){
    const context=await browser.newContext({viewport:{width,height:812},permissions:['clipboard-read','clipboard-write']});
    const page=await context.newPage();
    page.on('console',m=>events.push({viewport:width,type:`console.${m.type()}`,text:m.text(),url:m.location().url}));
    page.on('pageerror',e=>events.push({viewport:width,type:'pageerror',error:e.message}));
    page.on('requestfailed',r=>events.push({viewport:width,type:'requestfailed',url:r.url(),error:r.failure()?.errorText}));
    page.on('response',r=>events.push({viewport:width,type:'response',url:r.url(),status:r.status(),ms:Date.now()}));
    await check('U-043',width,async()=>{
      await page.goto(root);await page.mouse.move(1,1);
      const start=Date.now(), selections=[];
      for(let i=1;i<=4;i++){
        await expect(page.getByRole('tab').nth(i)).toHaveAttribute('aria-selected','true',{timeout:5500});
        selections.push({tab:i,elapsedMs:Date.now()-start});
      }
      for(const action of ['scroll','click','hover']){
        await page.goto(root);await page.mouse.move(1,1);
        if(action==='scroll')await page.mouse.wheel(0,100);
        if(action==='click')await page.locator('h1').click();
        if(action==='hover')await page.locator('.wire').hover();
        await page.waitForTimeout(4600);
        await expect(page.getByRole('tab').nth(0)).toHaveAttribute('aria-selected','true');
      }
      return {selections,stoppedBy:['scroll','click','hover']};
    });
    await check('U-033',width,async()=>{
      await page.emulateMedia({reducedMotion:'reduce'});await page.goto(root);
      const button=page.locator('button[data-copy="#qs"]');await button.click();await expect(button).toHaveText('Copied');
      assert.deepEqual((await page.evaluate(()=>navigator.clipboard.readText())).split('\n'),['cargo test -p spendguard -p x402z-protocol -p x402z-server','cargo test -p x402z-agent','cargo test -p x402z-mcp','cargo test -p x402z-facilitator','cd packages/x402-zcash && npm test']);
      await expect(button).toHaveText('Copy',{timeout:2500});
    });
    await page.goto('http://127.0.0.1:4120/');await expect(page.locator('#facState')).toHaveText('reachable');
    await check('U-064',width,async()=>{
      const s=await(await context.request.get('http://127.0.0.1:4120/api/status')).json();
      for(const[sel,value]of [['#ua',s.merchantUa],['#rk',s.receiptKey]]){await expect(page.locator(sel)).toHaveAttribute('title',value);await expect(page.locator(sel)).toHaveText(value.slice(0,12)+'…'+value.slice(-8));}
    });
    await check('U-089',width,async()=>{
      const start=Date.now();await page.waitForTimeout(3200);
      const polls=events.filter(e=>e.viewport===width&&e.type==='response'&&e.url.startsWith('http://127.0.0.1:4120/api/events?since=')&&e.ms>=start);
      assert.ok(polls.length>=2);assert.ok(Math.abs(polls[1].ms-polls[0].ms-1500)<250);
      const r=await context.request.get('http://127.0.0.1:4020/api/quote');assert.equal(r.status(),402);const nonce=(await r.json()).accepts[0].extra.memoNonce;
      await expect(page.locator('#feed li').filter({hasText:nonce})).toBeVisible({timeout:5000});
      return {pollIntervalsMs:polls.slice(1).map((p,i)=>p.ms-polls[i].ms),externalNonce:nonce,appearedWithinMs:Date.now()-start-3200};
    });
    await context.close();
  }
  assert.equal(events.filter(e=>e.type==='console.error'||e.type==='pageerror'||e.type==='requestfailed').length,0);
}finally{
  await browser.close();await server.close();
  console.log(`Chrome PID ${server.process().pid} stopped`);
  fs.writeFileSync(new URL('gaps-results.json',import.meta.url),JSON.stringify(results,null,2)+'\n');
  fs.writeFileSync(new URL('gaps-events.json',import.meta.url),JSON.stringify(events,null,2)+'\n');
}
