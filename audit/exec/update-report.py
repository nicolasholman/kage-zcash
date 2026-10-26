"""Promote only mapped UNTESTED rows with passed exact-assertion evidence.
Preserves every initial PASS/FAIL row verbatim and checks all 584 expectations.
"""
from pathlib import Path
from collections import Counter
import subprocess,re,json
root=Path(__file__).resolve().parents[2];out=root/'audit/exec'
base=subprocess.check_output(['git','show','4be20bf^:AUDIT.md'],cwd=root,text=True)
verification=json.loads((out/'verification-summary.json').read_text())
assert len(verification)==9 and all(r['exitCode']==0 for r in verification), 'final verification incomplete'
runs=json.loads((out/'rust-summary.json').read_text());assert len(runs)==6 and all(r['exitCode']==0 for r in runs)
covered={};mapped={}
for path in sorted(out.glob('map-*.md')):
 for line in path.read_text().splitlines():
  m=re.match(r'\| ([BU]-\d+) \| ([^|]+) \|',line)
  if not m:continue
  id,verdict=m.groups();verdict=verdict.strip();mapped[id]=verdict
  if verdict.startswith('COVERED:'):
   _,name,src=verdict.split(':',2);covered[id]=(name,src)
assert len(mapped)==356
proof={};rust_counts={}
for name in ['spendguard','protocol','server','agent','mcp','facilitator']:
 path=out/f'{name}.log';text=path.read_text();assert 'test result: FAILED' not in text
 rust_counts[name]=sum(map(int,re.findall(r'test result: ok\. (\d+) passed;',text)))
 current=None
 for line in text.splitlines():
  m=re.match(r'test (\S+) \.\.\.',line)
  if m:current=m.group(1)
  for id in re.findall(r'AUDIT ([BU]-\d+) PASS',line):
   assert current,id
   proof[id]=(current,f'audit/exec/{name}.log')
for id,(test,src) in covered.items():
 crate='server' if 'x402z-server' in src else 'agent' if 'x402z-agent' in src else 'facilitator'
 text=(out/f'{crate}.log').read_text()
 assert re.search(r'test (?:\S*::)?'+re.escape(test)+r' \.\.\. ok',text),id
 assert test in (root/src).read_text(),id
 proof[id]=(test,f'audit/exec/{crate}.log')
for log,source in [('cli.log','cli.py'),('static.log','static.py'),('server-live.log','server-live.mjs'),('mcp-probes.log','mcp.py')]:
 for id in re.findall(r'AUDIT ([BU]-\d+) PASS',(out/log).read_text()):proof[id]=(f'{source}: {id}',f'audit/exec/{log}')
ts_text=(out/'typescript-run.log').read_text()
assert re.search(r'ℹ fail 0\b',ts_text)
for line in ts_text.splitlines():
 if line.startswith('✔ '):
  name=line[2:].split(' (')[0]
  for id in re.findall(r'B-\d+',name):proof[id]=(name,'audit/exec/typescript-run.log')
live=(out/'typescript-live-run.log').read_text();assert 'ℹ pass 2' in live and 'ℹ fail 0' in live
proof['B-426']=("x402z-server's 402 parses, its ZIP 321 URI is ours, and a mined payment for another nonce is refused; the paywall middleware settles a real mined payment through the real facilitator, once",'audit/exec/typescript-live-run.log')
zip_test='librustzcash_parses_the_challenge_uri_back_to_the_challenge'
assert f'test {zip_test} ... ok' in (out/'facilitator.log').read_text()
zip_src=(root/'crates/x402z-facilitator/tests/zip321_uri.rs').read_text()
for snippet in ['123_456_789','(1,','100_000u64','req.payments().len(), 1','req.to_uri(), uri']:assert snippet in zip_src
proof['B-445']=(zip_test,'audit/exec/facilitator.log')
gets=(out/'live-get.log').read_text();assert gets.count('status=404 curl_exit=0')==3 and gets.count('result=PASS')==3
proof['B-470']=('B-470: one read-only curl GET per named URL; 404 on /, /api/events, /api/status','audit/exec/live-get.log')
blocked={
 'B-001':('Not executed: proving the default listener requires binding 4020, excluded by this run.','needs permission to bind the excluded default port 4020'),
 'B-002':('Not executed: proving the default admin listener requires binding 4029, excluded by this run.','needs permission to bind the excluded default port 4029'),
 'B-300':('Not executed: the map requires two real CLI pays, one successful; a fabricated broadcast outcome would not prove a funded payment. Sends are prohibited.','needs a funded Zcash testnet send'),
 'U-083':('No newly funded payment was sent for the active dashboard challenge.','needs a funded Zcash testnet send'),
 'U-084':('No unconfirmed payment was broadcast for the active dashboard challenge.','needs a funded Zcash testnet send'),
}
fixes={'B-034':'Fixed empty-authority validation to report the origin-only error (4be20bf).','B-076':'Expired direct receipts now carry the existing payment_expired settlement response (6a701d8).','B-358':'MCP redemption requires the ledger key before sending a payment presentation (3b2ec47).','B-324':'Validate the offered issue origin before authorizing or preparing payment (b652695).'}
rows=[];counts=Counter();upgraded=[];initial=[]
for line in base.splitlines():
 if not re.match(r'^\| [BU]-\d+ \|',line):continue
 c=line.split(' | ');id=c[0][2:];initial.append(c);status=c[3]
 if status=='UNTESTED':
  assert id in mapped
  if id in blocked:
   note,fix=blocked[id];evidence=f'Local (UNTESTED): {note} [execution limits](audit/exec/summary.md)'
  else:
   assert id in proof, f'No complete evidence for {id}'
   test,log=proof[id];status='PASS';evidence=f'Local (PASS): `{test}` — [passed execution]({log}); [probe source and method](audit/exec/summary.md)';fix=fixes.get(id,'—');upgraded.append(id)
  line=f'| {id} | {c[1]} | {c[2]} | {status} | {evidence} | {fix} |'
 counts[status]+=1;rows.append(line)
assert sum(counts.values())==584
assert counts=={'PASS':573,'FAIL':6,'UNTESTED':5},dict(counts)
text='Project: Kage\nAudit dispositions: 573 PASS / 6 FAIL / 5 UNTESTED. Original expectations are unchanged. Local and live evidence are identified separately; no deployment was performed.\n\n'
text+='| ID | component/flow | exact expected result | status | evidence | fix |\n| --- | --- | --- | --- | --- | --- |\n'+'\n'.join(rows)+'\n\n'
text+='Audit counts: 573 PASS / 6 FAIL / 5 UNTESTED (total 584 audit items).\n'
text+='Regression tests retain labelled loopback HTTP/lightwalletd fixtures and existing Sapling mock proof bytes; production fixes use the real cryptographic, policy and persistence logic. The six Chrome diagnostic FAILs remain unchanged; diagnostics and required HTTP status codes were not suppressed.\n'
text+='Origin was not pushed; all verification was completed locally with no deployment or transaction broadcast.\n'
final={l.split(' | ')[0][2:]:l for l in rows}
for old in initial:
 id=old[0][2:];new=final[id].split(' | ');assert old[2]==new[2],id
 if old[3] in ['PASS','FAIL']:
  old_line=next(l for l in base.splitlines() if l.startswith('| '+id+' |'))
  assert final[id]==old_line,id
(root/'AUDIT.md').write_text(text)
ts_counts={}
for name in ['typescript','typescript-unit','typescript-live']:
 t=(out/f'{name}-run.log').read_text();m=re.search(r'ℹ pass (\d+)',t);assert m and re.search(r'ℹ fail 0\b',t);ts_counts[name]=int(m[1])
summary={'audit':dict(counts),'total':584,'upgraded_rows':len(upgraded),'unchanged_initial_pass':222,'unchanged_initial_fail':6,'rust_passed':rust_counts,'rust_total':sum(rust_counts.values()),'typescript_passed':ts_counts,'typescript_total':sum(ts_counts.values()),'blocked':blocked,'root_fixes':fixes,'origin_pushed':False,'broadcasts':0}
(out/'counts.json').write_text(json.dumps(summary,indent=2)+'\n')
testing=(root/'TESTING.md').read_text();marker='Audit execution of the mapped rows (2026-10-26, task t10):'
if marker in testing:testing=testing[:testing.index(marker)].rstrip()+'\n'
testing+='\n'+marker+f' **{summary["rust_total"]} Rust tests and {summary["typescript_total"]} TypeScript tests passed** in the final commands: '+', '.join(f'{k} {v}' for k,v in rust_counts.items())+f'; TypeScript audit probes {ts_counts["typescript"]}, existing unit tests {ts_counts["typescript-unit"]}, and recorded-transaction live interoperability {ts_counts["typescript-live"]}. Crates ran serially with `CARGO_BUILD_JOBS=1` under the heavy slot. The existing read-only `live_testnet` log was reused; that suite was not rerun. No transaction was broadcast. [Commands, PIDs and exits](audit/exec/verification-summary.json), [row counts and limits](audit/exec/summary.md).\n'
(root/'TESTING.md').write_text(testing)
print(json.dumps(summary,indent=2))
