"""One foreground heavy slot, serial crates and real CLI/API probes, with PID/exit records.
Does not run live_testnet, send transactions, read .env, or access any existing wallet.
"""
from pathlib import Path
import os,subprocess,json,time
root=Path(__file__).resolve().parents[2];out=root/'audit/exec'
heavy=Path('/Volumes/Extreme SSD/Projects/grokbot/tools/heavy-slot.sh')
env={k:v for k,v in os.environ.items() if not k.startswith(('X402Z_','ZCASH_','KAGE_TEST_FILTER'))}
target=Path(os.environ.get('CARGO_TARGET_DIR',root/'target')).resolve()
env.update(CARGO_BUILD_JOBS='1',CARGO_TARGET_DIR=str(target),KAGE_BIN_DIR=str(target/'debug'))
summary=[]
def run(name,command,cwd=root):
 print(name,flush=True);start=time.monotonic()
 with (out/f'{name}-run.log').open('w') as log:
  log.write('Command: '+' '.join(map(str,command))+'\n');log.flush()
  child=subprocess.Popen(command,cwd=cwd,env=env,stdout=log,stderr=subprocess.STDOUT)
  try:code=child.wait()
  finally:
   if child.poll() is None:child.terminate();child.wait()
 summary.append({'name':name,'command':list(map(str,command)),'pid':child.pid,'exitCode':code,'seconds':round(time.monotonic()-start,2)})
 (out/'verification-summary.json').write_text(json.dumps(summary,indent=2)+'\n')
 print(f'{name}: PID {child.pid} exited {code}',flush=True)
 if code:raise RuntimeError(f'{name} failed; inspect its log')
with (out/'slot.log').open('w') as slot:
 subprocess.run([str(heavy),'acquire','kage-exec'],check=True,stdout=slot,stderr=subprocess.STDOUT)
 slot.write('acquired kage-exec for final verification\n');slot.flush()
 try:
  run('rust',['python3','audit/exec/run-rust.py'])
  run('build',['cargo','build','-p','x402z-server','-p','x402z-facilitator','--bins','--examples'])
  run('typescript',['node','--test','audit/exec/typescript.test.ts'])
  run('typescript-unit',['node','--test','test/unit.test.ts'],root/'packages/x402-zcash')
  run('static',['python3','audit/exec/static.py'])
  run('cli',['python3','audit/exec/cli.py'])
  run('server-live',['node','audit/exec/server-live.mjs'])
  run('mcp-probes',['python3','audit/exec/mcp.py'])
  (out/'live-pids.jsonl').unlink(missing_ok=True)
  run('typescript-live',['node','--import','./audit/exec/track-children.mjs','--test','packages/x402-zcash/test/live.test.ts'])
 finally:
  subprocess.run([str(heavy),'release','kage-exec'],check=True,stdout=slot,stderr=subprocess.STDOUT)
  slot.write('released kage-exec; each foreground child waited/reaped\n')
