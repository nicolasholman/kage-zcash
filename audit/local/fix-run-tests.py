"""Required non-spending regressions, serially and at one compiler job."""
import json, os, subprocess, time
from pathlib import Path
root = Path(__file__).resolve().parents[2]
runs = [
 ('fast', ['cargo','test','-p','spendguard','-p','x402z-protocol','-p','x402z-server']),
 ('agent', ['cargo','test','-p','x402z-agent']),
 ('mcp', ['cargo','test','-p','x402z-mcp']),
 ('facilitator', ['cargo','test','-p','x402z-facilitator','--test','verify_real_tx','--test','zip321_uri']),
 ('typescript', ['node','--test','test/unit.test.ts']),
]
summary=[]
for name,cmd in runs:
 started=time.monotonic()
 cwd=root/'packages/x402-zcash' if name=='typescript' else root
 with (root/f'audit/local/fix-tests-{name}.log').open('w') as log:
  log.write('Command: '+' '.join(cmd)+'\n');log.flush()
  child=subprocess.Popen(cmd,cwd=cwd,env={**os.environ,'CARGO_BUILD_JOBS':'1'},stdout=log,stderr=subprocess.STDOUT)
  print(f'{name}: PID {child.pid}',flush=True)
  code=child.wait()
 item={'name':name,'command':cmd,'pid':child.pid,'exitCode':code,'seconds':round(time.monotonic()-started,2)}
 summary.append(item)
 (root/'audit/local/fix-test-summary.json').write_text(json.dumps(summary,indent=2)+'\n')
 print(json.dumps(item),flush=True)
 if code: raise SystemExit(code)
