"""Synchronize original expectations with audited dispositions and unfiltered re-run evidence."""
from pathlib import Path
from collections import Counter
import json, re
root=Path(__file__).resolve().parents[2]
pattern=re.compile(r'^\| ([BU]-\d{3}) \| (.*?) \| (.*?) \| (PASS|FAIL|UNTESTED|PENDING) \| (.*?) \| (.*?) \|$')
def rows(path):
 return {m[1]:m.groups() for line in path.read_text().splitlines() if (m:=pattern.match(line))}
original=rows(root/'AUDIT.md')
localpath=root/'audit/results-local.md'
local=rows(localpath)
prior_local=local.copy()
live=rows(root/'audit/results-live.md')
assert len(original)==len(local)==584
reruns=json.loads((root/'audit/local/fix-results.json').read_text())
changed={}
fixes={
 'U-011':'Changed the landing top navigation label to Spec; href unchanged.',
 'U-062':'Made Keys a named section with a Keys h2; retained its anchor and layout.',
 'U-067':'Added a real space before TAZ in initial markup and the live amount formatter; removed CSS-only unit spacing.',
}
for ident in {r['id'] for r in reruns}:
 checks=[r for r in reruns if r['id']==ident]
 assert sorted(r['width'] for r in checks)==[375,1280]
 assert all(r['behaviorMatches'] for r in checks),(ident,checks)
 status='PASS' if all(r['status']=='PASS' for r in checks) else 'FAIL'
 ev=f'`{ident}` re-run at 1280/375 px — [assertions](local/fix-results.json), [unfiltered console/network](local/fix-events.json), [method and limits](local/fix-summary.md)'
 if ident in fixes:
  assert status=='PASS'
  fix=fixes[ident]
 elif ident=='U-096':
  assert status=='FAIL'
  fix='Re-run with the real server stopped by PID: banner matches; Chrome emits ERR_CONNECTION_REFUSED and failed network requests. Zero console/failed-request rule cannot pass while exercising the required real outage. No diagnostics suppressed.'
 else:
  assert status=='FAIL'
  statuses={'U-071':'402','U-081':'422','U-093':'402 and 502','U-095':'429','U-101':'402'}[ident]
  fix=f'Re-run matches required UI and HTTP behavior; Chrome emits automatic resource errors for required HTTP {statuses}. Public API/statuses retained. FAIL remains under the unchanged zero-console rule; no diagnostics suppressed.'
 old=local[ident]
 changed[ident]=(old[0],old[1],old[2],status,ev,fix)
local.update(changed)
lines=[]
for line in localpath.read_text().splitlines():
 m=pattern.match(line)
 if m and m[1] in changed: line='| '+' | '.join(changed[m[1]])+' |'
 lines.append(line)
localcounts=Counter(r[3] for r in local.values())
uicounts=Counter(r[3] for k,r in local.items() if k.startswith('U-'))
# Keep the initial run narrative explicitly historical, and put the current disposition first.
idx=next(i for i,l in enumerate(lines) if l.startswith('| ID |'))
history='\n'.join(lines[:idx]).strip()
historical=re.search(r'<details><summary>Original audit narrative \(before fixes\)</summary>\n\n(.*?)\n\n</details>',history,re.S)
if historical: history=historical[1]
header=f'''Local audit updated on 2026-10-26 after the UI fixes. Current dispositions: **{localcounts['PASS']} PASS, {localcounts['FAIL']} FAIL, {localcounts['UNTESTED']} UNTESTED**. UI: **{uicounts['PASS']} PASS, {uicounts['FAIL']} FAIL, {uicounts['UNTESTED']} UNTESTED**.

U-011, U-062 and U-067 now PASS after re-running their unchanged expectations at 1280 px and 375 px. U-071/U-081/U-093/U-095/U-096/U-101 remain FAIL: their UI and required HTTP behavior match, but Chrome emits automatic resource/network errors. Nothing was filtered or suppressed. U-096 now uses an actual stopped server, with no request interception. [Re-run evidence and limits](local/fix-summary.md), [regressions](local/fix-test-summary.json).

Original audit narrative below is historical; current table and re-run evidence supersede the nine original FAIL dispositions.

<details><summary>Original audit narrative (before fixes)</summary>

{history}

</details>

'''
localpath.write_text(header+'\n'.join(lines[idx:])+'\n')
# Preserve all prior PASS statuses and copy their evidence; do not infer unexecuted items.
def links(s): return s.replace('](local/','](audit/local/').replace('](live/','](audit/live/')
newlines=[]
counts=Counter()
for line in (root/'AUDIT.md').read_text().splitlines():
 m=pattern.match(line)
 if not m:
  newlines.append(line);continue
 ident,component,expected=m[1],m[2],m[3]
 assert (component,expected)==local[ident][1:3],ident
 candidates=[('Local',local[ident])]
 if ident in live: candidates.append(('Live',live[ident]))
 statuses=[r[3] for _,r in candidates]
 status='FAIL' if 'FAIL' in statuses else 'PASS' if 'PASS' in statuses else 'UNTESTED'
 evidence='; '.join(f'{label} ({r[3]}): '+links(r[4]) for label,r in candidates)
 fixes_for_row=[f'{label}: {r[5]}' for label,r in candidates if r[5] not in ('','—','-') and (r[3]!='UNTESTED' or status=='UNTESTED')]
 fix='; '.join(fixes_for_row) or '—'
 newlines.append('| '+' | '.join([ident,component,expected,status,evidence,fix])+' |')
 counts[status]+=1
newlines[1]=f'Audit dispositions: {counts["PASS"]} PASS / {counts["FAIL"]} FAIL / {counts["UNTESTED"]} UNTESTED. Original expectations are unchanged. Local and live evidence are identified separately; no deployment was performed.'
(root/'AUDIT.md').write_text('\n'.join(newlines)+'\n')
after=rows(root/'AUDIT.md')
assert all(original[k][:3]==after[k][:3] for k in original)
assert all(local[k][3]=='PASS' for k,r in prior_local.items() if r[3]=='PASS')
(root/'audit/local/fix-disposition-counts.json').write_text(json.dumps({'all':dict(counts),'local':dict(localcounts),'uiLocal':dict(uicounts),'live':dict(Counter(r[3] for r in live.values()))},indent=2)+'\n')
print(json.dumps({'all':dict(counts),'local':dict(localcounts),'uiLocal':dict(uicounts)},indent=2))
