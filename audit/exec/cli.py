"""Sterile-environment CLI/config probes. Raw stderr stays in private /tmp files; log only assertions."""
from pathlib import Path
import os,subprocess,tempfile,json,socket,time,urllib.request,urllib.error
root=Path(__file__).resolve().parents[2];bins=Path(os.environ.get('KAGE_BIN_DIR',root/'target/debug'));log=(root/'audit/exec/cli.log').open('w');passes=[]
def mark(n,note):passes.append(n);log.write(f'AUDIT B-{n:03} PASS {note}\n');log.flush()
def run(binary,args,env,dir):
 with tempfile.TemporaryFile() as private:
  os.fchmod(private.fileno(),0o600)
  p=subprocess.Popen([str(bins/binary),*args],env=env,cwd=dir,stdin=subprocess.DEVNULL,stdout=private,stderr=private)
  try:p.wait(timeout=20)
  finally:
   if p.poll() is None:p.terminate();p.wait()
  private.seek(0);s=private.read().decode(errors='replace')
 log.write(f'{binary} PID {p.pid} exit={p.returncode}\n');return p.returncode,s
with tempfile.TemporaryDirectory(prefix='kage-exec-cli-') as d:
 base={'PATH':os.environ['PATH']}
 server={**base,'X402Z_MERCHANT_UA':'utest1sampleaddressforschemaonly000000','X402Z_RECEIPT_PUBKEY':'77'*32,'X402Z_FACILITATOR_URL':'http://127.0.0.1:45999','X402Z_FACILITATOR_TOKEN':os.urandom(32).hex(),'X402Z_BIND':'127.0.0.1:45050','X402Z_ADMIN_BIND':'off','X402Z_TAB_MAX_TOKENS':'0','X402Z_SERVER_DB':d+'/server.sqlite'}
 def check(n,changes,parts,code=2,env=None,binary='x402z-server',args=[]):
  e=dict(server if env is None else env)
  for k,v in changes.items():
   if v is None:e.pop(k,None)
   else:e[k]=v
  status,s=run(binary,args,e,d);assert status==code,f'B-{n} exit {status} != {code}'
  for part in parts:assert part in s,f'B-{n} missing expected diagnostic'
  return s
 for n,var,text in [(116,'X402Z_MERCHANT_UA','error: X402Z_MERCHANT_UA not configured'),(118,'X402Z_RECEIPT_PUBKEY','error: X402Z_RECEIPT_PUBKEY not configured'),(120,'X402Z_FACILITATOR_URL','error: X402Z_FACILITATOR_URL not configured'),(121,'X402Z_FACILITATOR_TOKEN','error: Setup required: X402Z_FACILITATOR_TOKEN not configured.')]:check(n,{var:None},[text]);mark(n,'exit 2 and named missing-setting diagnostic; process exited without listening')
 check(117,{'X402Z_MERCHANT_UA':'u1wrong'},['error: X402Z_MERCHANT_UA must be a testnet unified address (utest1...)']);mark(117,'exit 2 address diagnostic')
 for val in ['not-hex','ab','02'+'00'*31]:check(119,{'X402Z_RECEIPT_PUBKEY':val},['error: X402Z_RECEIPT_PUBKEY invalid:'])
 mark(119,'invalid hex, length and invalid compressed ed25519 point rejected with exit 2')
 for val in ['0','bad']:check(122,{'X402Z_PRICE_ZAT':val},['error: X402Z_PRICE_ZAT must be a positive integer (zatoshi)'])
 mark(122,'zero and malformed price exit 2')
 for n,var,val,text in [(123,'X402Z_TRUSTED_PROXIES','bad','error: X402Z_TRUSTED_PROXIES must be a comma-separated list of IP addresses'),(124,'X402Z_MIN_CONFIRMATIONS','bad','error: X402Z_MIN_CONFIRMATIONS must be a non-negative integer'),(127,'X402Z_CHALLENGE_RATE_PER_MIN','bad','error: X402Z_CHALLENGE_RATE_PER_MIN must be a non-negative integer (0 disables the limit)'),(128,'X402Z_SERVER_DB',d+'/missing/db.sqlite','cannot open X402Z_SERVER_DB')]:check(n,{var:val},[text]);mark(n,'exit 2 and exact diagnostic; no listener success')
 for var in ['X402Z_EXPIRY_GRACE_SECS','X402Z_REDEEM_WINDOW_SECS','X402Z_RESEND_WINDOW_SECS','X402Z_TAB_EPOCH_SECS','X402Z_TAB_GRACE_SECS']:check(126,{var:'bad'},[f'error: {var} must be a non-negative integer (seconds)'])
 mark(126,'all five invalid seconds settings exit 2')
 for value in ['api.example','ftp://api.example']:check(33,{'X402Z_PUBLIC_BASE_URL':value},['error: X402Z_PUBLIC_BASE_URL','must start with https:// or http://'])
 mark(33,'missing scheme and ftp exit 2 with both exact diagnostic fragments')
 for value in ['https://','https://api.example/path','https://api.example?x=1','https://api.example#x','https://api .example']:check(34,{'X402Z_PUBLIC_BASE_URL':value},['must be an origin like https://api.example (no path)'])
 mark(34,'empty authority, path, query, fragment and interior whitespace exit 2')
 check(17,{'X402Z_ADMIN_BIND':server['X402Z_BIND']},['error: X402Z_ADMIN_BIND must differ from X402Z_BIND: the dashboard, feed and status stay off the public port']);mark(17,'exit 2 with entire quoted error')
 with socket.socket() as held:
  held.bind(('127.0.0.1',0));held.listen();addr=f'127.0.0.1:{held.getsockname()[1]}';check(129,{'X402Z_BIND':addr},[f'error: cannot bind {addr}:'],1)
 mark(129,'actual loopback bind collision exits 1 and names occupied address')
 uivk=__import__('re').search(r'uivktest1[a-z0-9]{100,}',(root/'README.md').read_text())[0]
 fac={**base,'X402Z_MERCHANT_UIVK':uivk,'X402Z_RECEIPT_SIGNING_KEY':os.urandom(32).hex(),'X402Z_FACILITATOR_TOKEN':os.urandom(32).hex(),'X402Z_FACILITATOR_DB':d+'/fac.sqlite','ZCASH_LWD_URL':'http://127.0.0.1:45999'}
 for n,changes,args,parts,code in [(247,{},['unknown-cmd'],["error: unknown command 'unknown-cmd' (usage: x402z-facilitator [serve|keygen|derive-uivk])"],2),(248,{'X402Z_MERCHANT_UIVK':None},[],['error: X402Z_MERCHANT_UIVK not configured (see .env.example)'],2),(249,{'X402Z_MERCHANT_UIVK':'dummy','X402Z_MERCHANT_UFVK':'dummy'},[],['error: set only one of X402Z_MERCHANT_UIVK and X402Z_MERCHANT_UFVK (the UIVK is enough)'],2),(250,{'X402Z_MERCHANT_UIVK':'dummy'},[],['X402Z_MERCHANT_UIVK is not a testnet unified incoming viewing key (uivktest1...)'],2),(252,{'X402Z_FACILITATOR_TOKEN':None},[],['error: Setup required: X402Z_FACILITATOR_TOKEN not configured.'],2),(253,{'X402Z_FACILITATOR_TOKEN':'short'},[],['error: X402Z_FACILITATOR_TOKEN must be at least 32 characters'],2),(256,{},[],['error: cannot connect to lightwalletd:'],1),(259,{'X402Z_FACILITATOR_DB':d+'/absent/db.sqlite'},[],['cannot open X402Z_FACILITATOR_DB'],2)]:check(n,changes,parts,code,fac,'x402z-facilitator',args);mark(n,'sterile CLI exit and exact diagnostic')
 for val in [None,'invalid']:check(254,{'X402Z_RECEIPT_SIGNING_KEY':val},['error: X402Z_RECEIPT_SIGNING_KEY '+('not configured' if val is None else 'invalid:')],env=fac,binary='x402z-facilitator')
 mark(254,'absent and malformed signing key exit 2; no key output collected')
 for var in ['X402Z_MIN_CONFIRMATIONS','X402Z_EXPIRY_GRACE_SECS']:
  for v in ['-1','1.5']:check(258,{var:v},[f'error: {var} must be a non-negative integer'],env=fac,binary='x402z-facilitator')
 mark(258,'negative and noninteger confirmation/grace values exit 2')
 # Exercise the default-URL diagnostic while failing before any connection.
 s=check(255,{'ZCASH_LWD_URL':None,'X402Z_MERCHANT_UIVK':None},['ZCASH_LWD_URL not set; using https://testnet.zec.rocks:443'],env=fac,binary='x402z-facilitator');assert '.with_webpki_roots()' in (root/'crates/x402z-facilitator/src/lwd.rs').read_text();mark(255,'default URL diagnostic and TLS webpki-root source assertion')
log.write(f'{len(passes)} row probes passed. Raw command output was private and ephemeral; no seed or wallet loaded.\n');log.close()
