"""Run only the requested non-spending test targets, sequentially; retain logs and PIDs."""
import json
from pathlib import Path
import subprocess
import time

root = Path(__file__).resolve().parents[2]
out = root / 'audit/local'
runs = [
    ('live-testnet', ['cargo', 'test', '-p', 'x402z-facilitator', '--test', 'live_testnet', '--', '--test-threads=1'], root),
    ('mcp', ['cargo', 'test', '-p', 'x402z-mcp', '--', '--test-threads=1'], root),
    ('agent', ['cargo', 'test', '-p', 'x402z-agent', '--', '--test-threads=1'], root),
    ('protocol-spendguard-server', ['cargo', 'test', '-p', 'spendguard', '-p', 'x402z-protocol', '-p', 'x402z-server', '--', '--test-threads=1'], root),
    ('typescript-unit', ['node', '--test', 'test/unit.test.ts'], root / 'packages/x402-zcash'),
]
summary = []
for name, cmd, cwd in runs:
    started = time.monotonic()
    with (out / f'{name}.log').open('w') as log:
        log.write('Command: ' + ' '.join(cmd) + '\n')
        log.flush()
        child = subprocess.Popen(cmd, cwd=cwd, stdout=log, stderr=subprocess.STDOUT)
        print(f'{name}: started PID {child.pid}', flush=True)
        code = child.wait()
    summary.append({'name': name, 'command': cmd, 'pid': child.pid, 'exitCode': code, 'seconds': round(time.monotonic()-started,2)})
    (out / 'test-summary.json').write_text(json.dumps(summary, indent=2)+'\n')
    print(json.dumps(summary[-1]), flush=True)
