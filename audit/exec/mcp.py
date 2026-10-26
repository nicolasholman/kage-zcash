"""Probe the real MCP binary via stdio using isolated configuration. No wallet or chain calls."""
from pathlib import Path
import subprocess,os,json,tempfile,time
root=Path(__file__).resolve().parents[2];bin=Path(os.environ.get('KAGE_BIN_DIR',root/'target/debug'));out=root/'audit/exec'
log=(out/'mcp-probes.log').open('w');results=[]
def mark(n):
 results.append(n);log.write(f'AUDIT B-{n:03} PASS\n');log.flush()
def request(i,method,params=None):return {'jsonrpc':'2.0','id':i,'method':method,'params':params or {}}
def call(i,name,args={}):return request(i,'tools/call',{'name':name,'arguments':args})
def run(lines,env):
 text=''.join(x+'\n' if isinstance(x,str) else json.dumps(x)+'\n' for x in lines)
 p=subprocess.Popen([str(bin/'x402z-mcp')],env=env,cwd=env['TMPDIR'],stdin=subprocess.PIPE,stdout=subprocess.PIPE,stderr=subprocess.PIPE,text=True)
 try:stdout,stderr=p.communicate(text,timeout=20)
 finally:
  if p.poll() is None:p.terminate();p.wait()
 assert p.returncode==0
 log.write(f'MCP PID {p.pid} exited 0; {len(stdout.splitlines())} response lines\n')
 rs=[json.loads(l) for l in stdout.splitlines()]
 for r in rs:assert r['jsonrpc']=='2.0'
 return rs
def tool(r):
 v=r['result'];assert isinstance(v['isError'],bool);assert v['content'][0]['type']=='text';assert json.loads(v['content'][0]['text'])==v['structuredContent'];return v['structuredContent']
with tempfile.TemporaryDirectory(prefix='kage-exec-mcp-') as d:
 env={'PATH':os.environ['PATH'],'TMPDIR':d,'X402Z_AGENT_WALLET_DIR':d+'/unused-wallet','X402Z_AGENT_LEDGER':d+'/ledger.json','X402Z_AGENT_LEDGER_KEY':os.urandom(32).hex(),'X402Z_AGENT_DAILY_CAP_ZAT':'50000','X402Z_AGENT_ALLOWED_HOSTS':'127.0.0.1','X402Z_AGENT_NAME':'audit'}
 for n,version in [(346,'2025-06-18'),(347,'2025-03-26'),(348,'2024-11-05'),(349,'1999-01-01')]:
  full=run([request(1,'initialize',{'protocolVersion':version})],env)[0];assert full['id']==1;r=full['result'];assert r['serverInfo']['name']=='x402z-mcp';assert r['protocolVersion']==('2025-06-18' if n==349 else version);assert r['capabilities']['tools']['listChanged'] is False;mark(n)
 r=run([request(1,'no-such-method')],env)[0];assert r['error']=={'code':-32601,'message':'method not found: no-such-method'};mark(353)
 r=run([call(1,'unknown-tool')],env)[0];assert r['error']=={'code':-32602,'message':"unknown tool 'unknown-tool'"};mark(354)
 rs=run(['{',request(1,'ping')],env);assert len(rs)==2;byid={r['id']:r for r in rs};assert byid[None]['error']['code']==-32700;assert byid[None]['error']['message'].startswith('parse error:');assert byid[1]['result']=={};mark(355)
 rs=run(['',request(5,'ping')],env);assert rs==[{'jsonrpc':'2.0','id':5,'result':{}}];mark(356)
 valid={'url':'http://127.0.0.1:45999/x','txid':'ab'*32,'memoNonce':'n'}
 missing={k:v for k,v in env.items() if k!='X402Z_AGENT_LEDGER_KEY'}
 rs=run([call(i,n,valid) for i,n in enumerate(['paid_fetch','redeem_payment','tab_fetch','spend_status','ledger_tail'])],missing)
 for r in rs:v=tool(r);assert r['result']['isError'] is True, str((r['id'],v.get('error')));assert v['error']=='setup_required';assert 'X402Z_AGENT_LEDGER_KEY' in v['message']
 assert len(rs)==5;mark(358)
 rs=run([call(i,'redeem_payment',{k:v for k,v in valid.items() if k!=omit}) for i,omit in enumerate(valid)],env)
 for r in rs:assert r['result']['isError'] is True;assert tool(r)['error']=='invalid_arguments'
 mark(361)
 r=run([call(1,'redeem_payment',{**valid,'txid':'bad'})],env)[0];assert r['result']['isError'] is True;v=tool(r);assert v['error']=='invalid_arguments';assert '64 hex' in v['message'];mark(362)
 rs=run([call(i,'tab_fetch',{'url':valid['url'],'buy':n}) for i,n in enumerate([0,-1,1.5,101])],env)
 for r in rs:assert r['result']['isError'] is True;assert tool(r)=={'error':'invalid_arguments','message':'buy must be an integer from 1 to 100'}
 mark(363)
 r=run([call(1,'tab_fetch',{'url':valid['url']})],{**env,'X402Z_AGENT_TAB_SIZE':'bad'})[0];assert r['result']['isError'] is True;assert tool(r)=={'error':'setup_required','message':'X402Z_AGENT_TAB_SIZE must be a positive integer'};mark(364)
 rs=run([call(1,'spend_status'),call(2,'ledger_tail')],env);byid={r['id']:r for r in rs};v=tool(byid[1]);assert byid[1]['result']['isError'] is False
 for k,w in {'spentTodayZat':0,'dailyCapZat':50000,'remainingZat':50000,'network':'zcash-testnet','allowedHosts':['127.0.0.1'],'approvalThresholdZat':None,'utcDay':int(time.time())//86400}.items():assert v[k]==w
 mark(374);assert byid[2]['result']['isError'] is False;assert tool(byid[2])=={'total':0,'entries':[]};mark(375)
 # Real migration seals a synthetic, clearly labelled refusal history; no payment entries.
 entries=[{'ts':i,'agent':'audit','host':'synthetic-audit.local','amountZat':i,'decision':'denied','reason':'synthetic refusal fixture'} for i in range(15)]
 Path(d+'/ledger.json').write_text(json.dumps({'agents':[],'audit':entries}))
 p=subprocess.run([str(bin/'x402z-agent'),'seal-ledger'],env=env,cwd=d,capture_output=True);assert p.returncode==0
 rs=run([call(i,'ledger_tail',args) for i,args in enumerate([{}, {'n':1},{'n':100},{'n':0},{'n':101},{'n':1.5}])],env);byid={r['id']:r for r in rs}
 for i,n in [(0,10),(1,1),(2,100)]:v=tool(byid[i]);assert byid[i]['result']['isError'] is False;assert v['total']==15;assert v['entries']==entries[-n:]
 for i in [3,4,5]:assert byid[i]['result']['isError'] is True;assert tool(byid[i])=={'error':'invalid_arguments','message':'n must be an integer from 1 to 100'}
 mark(376);mark(373)
# Real local server for free fetch and own-payment replay (clearly labelled persisted fixture).
import socket,sqlite3,urllib.request,urllib.error
with tempfile.TemporaryDirectory(prefix='kage-exec-mcp-server-') as d:
 def free_port():
  with socket.socket() as sock:sock.bind(('127.0.0.1',0));port=sock.getsockname()[1]
  assert port>4142
  return port
 public,admin=free_port(),free_port();db=d+'/server.sqlite'
 env={'PATH':os.environ['PATH'],'TMPDIR':d,'X402Z_AGENT_WALLET_DIR':d+'/unused-wallet','X402Z_AGENT_LEDGER':d+'/ledger.json','X402Z_AGENT_LEDGER_KEY':os.urandom(32).hex(),'X402Z_AGENT_DAILY_CAP_ZAT':'50000','X402Z_AGENT_ALLOWED_HOSTS':'127.0.0.1','X402Z_AGENT_NAME':'audit'}
 server_env={'PATH':os.environ['PATH'],'X402Z_MERCHANT_UA':'utest1sampleaddressforschemaonly000000','X402Z_RECEIPT_PUBKEY':'77'*32,'X402Z_FACILITATOR_URL':'http://127.0.0.1:45999','X402Z_FACILITATOR_TOKEN':os.urandom(32).hex(),'X402Z_BIND':f'127.0.0.1:{public}','X402Z_ADMIN_BIND':f'127.0.0.1:{admin}','X402Z_TAB_MAX_TOKENS':'0','X402Z_CHALLENGE_RATE_PER_MIN':'0','X402Z_RESEND_WINDOW_SECS':'0','X402Z_SERVER_DB':db}
 with tempfile.TemporaryFile() as private:
  os.fchmod(private.fileno(),0o600);server=subprocess.Popen([str(bin/'x402z-server')],env=server_env,cwd=d,stdin=subprocess.DEVNULL,stdout=private,stderr=private)
  try:
   for _ in range(200):
    try:
     with urllib.request.urlopen(f'http://127.0.0.1:{admin}/api/status',timeout=1) as response:expected=json.load(response)
     break
    except OSError:time.sleep(.02)
   else:raise AssertionError('server readiness')
   r=run([call(1,'paid_fetch',{'url':f'http://127.0.0.1:{admin}/api/status'})],env)[0];v=tool(r);assert r['result']['isError'] is False;assert v['status']==200;assert v['paid'] is None;assert v['settlement'] is None;assert v['body']==expected;mark(371)
   url=f'http://127.0.0.1:{public}/api/quote'
   try:urllib.request.urlopen(url)
   except urllib.error.HTTPError as error:assert error.code==402;challenge=json.load(error)['accepts'][0]
   nonce=challenge['extra']['memoNonce'];txid='ab'*32
   with sqlite3.connect(db) as conn:conn.execute('UPDATE challenges SET spent_txid=?,spent_at=? WHERE nonce=?',(txid,int(time.time()),nonce))
   entry={'ts':int(time.time()),'agent':'audit','host':'127.0.0.1','amountZat':100000,'decision':'allowed','reason':'Explicit synthetic post-broadcast replay fixture','memoNonce':nonce,'txid':txid,'url':url}
   Path(d+'/ledger.json').write_text(json.dumps({'agents':[],'audit':[entry]}))
   sealed=subprocess.run([str(bin/'x402z-agent'),'seal-ledger'],env=env,cwd=d,capture_output=True);assert sealed.returncode==0
   r=run([call(1,'paid_fetch',{'url':url})],env)[0];v=tool(r);assert r['result']['isError'] is True;assert v['error']=='already_redeemed';assert 'Do NOT pay again' in v['message']
   rs=run([call(1,'ledger_tail'),call(2,'spend_status')],env);byid={r['id']:r for r in rs};entries=tool(byid[1]);assert entries['total']==1;assert entries['entries'][0]['settled'] is True;assert entries['entries'][0]['txid']==txid;assert tool(byid[2])['spentTodayZat']==100000;mark(370)
  finally:
   if server.poll() is None:server.terminate()
   server.wait(timeout=10);log.write(f'real server PID {server.pid} killed/reaped\n')
log.write(f'{len(results)} row probes passed; no wallet, seed or broadcast.\n');log.close()
