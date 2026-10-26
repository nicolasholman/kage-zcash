"""Default listener, ledger-only cap race, server tests, and existing browser suite.

Run after acquiring `heavy-slot.sh acquire kage-defaults`; always releases the slot.
Uses an existing target/ build cache, never reads .env or a wallet, and never pays.
"""
from pathlib import Path
import http.client
import json
import os
import re
import secrets
import shutil
import signal
import sys
import subprocess
import tempfile
import time

ROOT = Path(__file__).resolve().parents[3]
OUT = ROOT / 'audit/exec/defaults'
SLOT = '/Volumes/Extreme SSD/Projects/grokbot/tools/heavy-slot.sh'
ENV = {k: v for k, v in os.environ.items() if not k.startswith(('X402Z_', 'ZCASH_'))}
ENV.update(CARGO_BUILD_JOBS='1', CARGO_TARGET_DIR=str((ROOT / 'target').resolve()))
BIN = Path(ENV['CARGO_TARGET_DIR']) / 'debug'
processes = []
stack_started = False
resume = "--remaining" in sys.argv
runs = json.loads((OUT / "runs.json").read_text()) if resume else []
tmp = Path(tempfile.mkdtemp(prefix='kage-defaults-', dir='/tmp'))
stack_root = tmp / 'stack'


def run(label, command, cwd=ROOT, env=ENV):
    print(f'{label}: starting', flush=True)
    with (OUT / f'{label}.log').open('w') as log:
        log.write('Command: ' + ' '.join(map(str, command)) + '\nCARGO_BUILD_JOBS=1\n')
        log.flush()
        p = subprocess.Popen(command, cwd=cwd, env=env, stdout=log, stderr=subprocess.STDOUT)
        processes.append(p)
        code = p.wait()
    runs.append(dict(label=label, command=list(map(str, command)), pid=p.pid, exitCode=code))
    (OUT / 'runs.json').write_text(json.dumps(runs, indent=2) + '\n')
    print(f'{label}: exit {code}', flush=True)
    if code:
        raise RuntimeError(f'{label} failed; see its log')


def free_ports(ports, log):
    for port in ports:
        r = subprocess.run(['lsof', '-nP', f'-iTCP:{port}', '-sTCP:LISTEN'], capture_output=True, text=True)
        log.write(f'Preflight lsof TCP {port}: exit {r.returncode}\n{r.stdout}{r.stderr}')
        log.flush()
        if r.returncode != 1 or r.stdout:
            raise RuntimeError(f'port {port} is occupied; no listener killed; see preflight log')


def secret_file(name):
    return os.fdopen(os.open(tmp / name, os.O_CREAT | os.O_EXCL | os.O_WRONLY, 0o600), 'w')


def start(name, env):
    log = secret_file(name + '.log')
    p = subprocess.Popen([BIN / ('x402z-' + name)], cwd=ROOT, env=env, stdout=log, stderr=subprocess.STDOUT)
    log.close()
    processes.append(p)
    return p


def stop(p):
    if p.poll() is None:
        p.terminate()
        try:
            p.wait(timeout=10)
        except subprocess.TimeoutExpired:
            p.kill()
            p.wait()


def interrupted(signum, frame):
    raise RuntimeError(f'interrupted by signal {signum}')


signal.signal(signal.SIGTERM, interrupted)
signal.signal(signal.SIGINT, interrupted)
try:
    if not resume:
        with (OUT / 'ports.log').open('a') as log:
            free_ports((4020, 4029, 4021), log)
        run('build', ['cargo', 'build', '-p', 'x402z-server', '-p', 'x402z-facilitator', '--bins', '--examples'])
        with (OUT / 'ports.log').open('a') as log:
            free_ports((4020, 4029, 4021), log)
        uivk = re.search(r'uivktest1[a-z0-9]{100,}', (ROOT / 'README.md').read_text()).group()
        ua = subprocess.check_output([BIN / 'examples/check_uivk', 'address', uivk], env=ENV, text=True).strip()
        with secret_file('receipt.env') as f:
            subprocess.run([BIN / 'x402z-facilitator', 'keygen'], env=ENV, stdout=f, stderr=subprocess.STDOUT, check=True)
        receipt = dict(line.split('=', 1) for line in (tmp / 'receipt.env').read_text().splitlines())
        token = secrets.token_hex(32)
        with secret_file('token.env') as f:
            f.write('X402Z_FACILITATOR_TOKEN=' + token + '\n')
        facilitator = start('facilitator', dict(ENV, **receipt, X402Z_FACILITATOR_TOKEN=token,
            X402Z_MERCHANT_UIVK=uivk, X402Z_FACILITATOR_BIND='127.0.0.1:4021',
            X402Z_FACILITATOR_DB=str(tmp / 'facilitator.sqlite')))
        server_env = dict(ENV, X402Z_MERCHANT_UA=ua, X402Z_RECEIPT_PUBKEY=receipt['X402Z_RECEIPT_PUBKEY'],
            X402Z_FACILITATOR_TOKEN=token, X402Z_FACILITATOR_URL='http://127.0.0.1:4021',
            X402Z_SERVER_DB=str(tmp / 'server.sqlite'))
        assert 'X402Z_BIND' not in server_env and 'X402Z_ADMIN_BIND' not in server_env
        server = start('server', server_env)
        # Wait for both real listeners. Facilitator startup probes testnet read-only.
        deadline = time.monotonic() + 120
        while True:
            if server.poll() is not None or facilitator.poll() is not None:
                raise RuntimeError('default stack exited; private logs suppressed')
            if 'listening' in (tmp / 'facilitator.log').read_text():
                try:
                    c = http.client.HTTPConnection('127.0.0.1', 4029, timeout=2)
                    c.request('GET', '/api/status', headers={'Host': 'localhost:4029'})
                    r = c.getresponse()
                    r.read()
                    c.close()
                    if r.status == 200:
                        break
                except OSError:
                    pass
            if time.monotonic() > deadline:
                raise RuntimeError('default stack readiness timed out')
            time.sleep(0.3)
        with (OUT / 'ports.log').open('a') as log:
            log.write(f'Default server PID {server.pid}; facilitator PID {facilitator.pid}\n')
            log.write('Child environment: X402Z_BIND absent; X402Z_ADMIN_BIND absent.\n')
            for row, port, path, expected in [('B-001', 4020, '/api/quote', 402), ('B-002', 4029, '/', 200)]:
                c = http.client.HTTPConnection('127.0.0.1', port, timeout=10)
                headers = {'Host': 'localhost:4029'} if port == 4029 else {}
                c.request('GET', path, headers=headers)
                r = c.getresponse()
                r.read()
                ct = r.getheader('Content-Type')
                log.write(f'{row}: GET http://127.0.0.1:{port}{path}' + (' Host: localhost:4029' if headers else '') + '\n')
                log.write(f'HTTP/1.1 {r.status} {r.reason}\nContent-Type: {ct}\n')
                log.flush()
                assert r.status == expected
                if row == 'B-002':
                    assert ct == 'text/html; charset=utf-8'
                c.close()
            # Stop default processes before tests or any subsequent use of 4020.
            stop(server)
            stop(facilitator)
            log.write('Stopped both default-stack PIDs before server tests and E2E.\n')
            free_ports((4020, 4029, 4021), log)
        print('B-001/B-002: PASS; default processes stopped', flush=True)
        run('server-tests', ['cargo', 'test', '-p', 'x402z-server', '--', '--test-threads=1'])
    # The existing browser stack requires the agent binary. This builds it but never runs pay.
    run('agent-build', ['cargo', 'build', '-p', 'x402z-agent', '--bin', 'x402z-agent'])
    probe = tmp / 'probe'
    probe.mkdir()
    manifest = '\n'.join(['[package]', 'name = "kage-b300-probe"', 'version = "0.0.0"', 'edition = "2021"',
        '[workspace]', '[[bin]]', 'name = "cap-race"', 'path = ' + json.dumps(str(OUT / 'cap-race.rs')),
        '[dependencies]', 'x402z-agent = { path = ' + json.dumps(str(ROOT / 'crates/x402z-agent')) + ' }',
        'spendguard = { path = ' + json.dumps(str(ROOT / 'crates/spendguard')) + ' }', 'rand = "0.10"',
        '[profile.dev.package."*"]', 'opt-level = 3', ''])
    (probe / 'Cargo.toml').write_text(manifest)
    shutil.copyfile(ROOT / 'Cargo.lock', probe / 'Cargo.lock')
    run('cap-race', ['cargo', 'run', '--offline', '--manifest-path', str(probe / 'Cargo.toml'), '--bin', 'cap-race', '--', str(tmp / 'ledger.json')])
    # Execute byte-identical existing E2E files from /tmp so even stack.sh's keygen
    # output and Playwright diagnostics stay under /tmp (receipt.env is mode 0600).
    stack_root.mkdir()
    shutil.copytree(ROOT / 'e2e', stack_root / 'e2e', ignore=shutil.ignore_patterns('.run', 'node_modules', 'test-results', 'playwright-report'))
    shutil.copyfile(ROOT / 'README.md', stack_root / 'README.md')
    (stack_root / 'target').symlink_to(Path(ENV['CARGO_TARGET_DIR']), target_is_directory=True)
    (stack_root / 'landing').symlink_to(ROOT / 'landing', target_is_directory=True)
    run('npm-ci', ['npm', 'ci', '--ignore-scripts', '--no-audit', '--no-fund'], cwd=stack_root / 'e2e')
    ports = (4020, 4021, 4030, 4039, 4040, 4041, 4042, 4050, 4120, 4130, 4140, 4141, 4142)
    with (OUT / 'e2e-preflight.log').open('w') as log:
        free_ports(ports, log)
    assert not (stack_root / 'e2e/.run/pids').exists(), 'refuse to stop or overwrite another stack'
    stack_started = True
    run('stack-start', ['bash', 'e2e/stack.sh'], cwd=stack_root)
    run('playwright', ['npm', 'run', 'test:e2e'], cwd=stack_root / 'e2e')
finally:
    try:
        if stack_started:
            run('stack-stop', ['bash', 'e2e/stack.sh', 'stop'], cwd=stack_root)
    finally:
        try:
            for p in reversed(processes):
                stop(p)
            shutil.rmtree(tmp)
        finally:
            subprocess.run([SLOT, 'release', 'kage-defaults'], check=True)
            (OUT / 'cleanup.log').write_text('Stopped only child PIDs (including E2E stack PIDs).\nDeleted /tmp scratch directory and throwaway secrets.\nReleased heavy slot kage-defaults.\n')
            print('Cleanup complete; heavy slot released', flush=True)
