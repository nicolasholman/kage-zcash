"""Serial cargo audit probes. Temporarily include probe source; always restore production source.
Reuse an explicit existing target dir to avoid duplicating Zcash build dependencies.
"""
from pathlib import Path
import os, subprocess, json, signal, sys, time
root=Path(__file__).resolve().parents[2]
out=root/'audit/exec'
env={k:v for k,v in os.environ.items() if not k.startswith(('X402Z_','ZCASH_'))}
env['CARGO_BUILD_JOBS']='1'
target_dir=Path(os.environ.get('CARGO_TARGET_DIR',root/'target')).resolve()
env['CARGO_TARGET_DIR']=str(target_dir)
env['KAGE_BIN_DIR']=str(target_dir/'debug')
summary=json.loads((out/'rust-summary.json').read_text()) if len(sys.argv)>1 and (out/'rust-summary.json').exists() else []
runs=[
 ('spendguard',['cargo','test','-p','spendguard','--','--test-threads=1','--nocapture']),
 ('protocol',['cargo','test','-p','x402z-protocol','--','--test-threads=1','--nocapture']),
 ('server',['cargo','test','-p','x402z-server','--','--test-threads=1','--nocapture']),
 ('agent',['cargo','test','-p','x402z-agent','--','--test-threads=1','--nocapture']),
 ('mcp',['cargo','test','-p','x402z-mcp','--','--test-threads=1']),
 ('facilitator',['cargo','test','-p','x402z-facilitator','--lib','--bin','x402z-facilitator','--test','verify_real_tx','--test','zip321_uri','--','--test-threads=1','--nocapture'])]
for name,cmd in runs:
    if len(sys.argv)>1 and name not in sys.argv[1:]:continue
    if os.environ.get('KAGE_TEST_FILTER'):
        cmd=cmd[:cmd.index('--')]+[os.environ['KAGE_TEST_FILTER']]+cmd[cmd.index('--'):]
    source=None;original=None;extra=None;manifest=None;manifest_original=None;lock_original=None
    if name=='facilitator':
        manifest=root/'crates/x402z-facilitator/Cargo.toml';manifest_original=manifest.read_text();manifest.write_text(manifest_original+'\n# Temporary audit harness codec; removed after execution.\ntonic-prost = \"=0.14.6\"\nreqwest = { version = \"=0.12.28\", default-features = false, features = [\"json\", \"rustls-tls\"] }\n');lock_original=(root/'Cargo.lock').read_bytes()
    if name in ['server','facilitator']:
        source=root/f'crates/x402z-{name}/src/main.rs'
        original=source.read_text();at=original.rfind('}')
        source.write_text(original[:at]+f'\ninclude!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../audit/exec/{name}.rs"));\n'+original[at:])
    elif name in ['spendguard','protocol','agent']:
        crate='spendguard' if name=='spendguard' else f'x402z-{name}'
        extra=root/f'crates/{crate}/tests/audit_exec.rs';extra.parent.mkdir(exist_ok=True)
        if extra.exists():raise RuntimeError('Refusing to overwrite test file')
        extra.write_text((out/f'{name}.rs').read_text())
    try:
        with (out/f'{name}.log').open('w') as log:
            log.write('Command: '+' '.join(cmd)+'\nCARGO_BUILD_JOBS=1\n');log.flush()
            p=subprocess.Popen(cmd,cwd=root,env=env,stdout=log,stderr=subprocess.STDOUT)
            print(f'{name} PID {p.pid}',flush=True)
            try: code=p.wait()
            finally:
                if p.poll() is None:p.terminate();p.wait()
        summary=[r for r in summary if r['name']!=name]
        summary.append({'name':name,'command':cmd,'pid':p.pid,'exitCode':code})
        (out/'rust-summary.json').write_text(json.dumps(summary,indent=2)+'\n')
        print(f'{name} exit {code}',flush=True)
        if code:sys.exit(code)
    finally:
        if original is not None:source.write_text(original)
        if extra is not None:extra.unlink(missing_ok=True)
        if manifest_original is not None:manifest.write_text(manifest_original);(root/'Cargo.lock').write_bytes(lock_original)
