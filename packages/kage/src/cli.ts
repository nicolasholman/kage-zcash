#!/usr/bin/env node
/**
 * `kage`: set up and run the Kage pieces.
 *
 *   kage wallet new [--dir .kage/agent] [--role agent|merchant]   new testnet wallet; seed goes to a 0600 file
 *   kage wallet status [--dir .kage/agent]                        address, incoming viewing key, balance
 *   kage keys check <key>                                         is this a UIVK, a UFVK or an address?
 *   kage facilitator keygen                                       receipt signing key pair for a facilitator
 *   kage facilitator                                              run the facilitator (needs X402Z_MERCHANT_UIVK, ...)
 */
import { spawn } from 'node:child_process';
import { readFileSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { createKage, KageError } from './agent.ts';
import { enginePath, KageSetupError } from './engine.ts';
import { assertIncomingOnly, createWallet, keyKind, readSeedFile } from './keys.ts';
import { formatZec } from './units.ts';

const args = process.argv.slice(2);
const flag = (name: string, dflt: string) => {
  const i = args.indexOf(`--${name}`);
  return i >= 0 && args[i + 1] ? args[i + 1] : dflt;
};

async function main() {
  const [cmd, sub] = args;
  if (cmd === 'wallet' && sub === 'new') {
    const dir = flag('dir', '.kage/agent');
    const role = flag('role', 'agent');
    const w = createWallet({ dir });
    const cut = (k: string) => `${k.slice(0, 14)}…${k.slice(-6)}`;
    console.log(`New ${role} wallet · Zcash testnet · birthday ${w.birthday}`);
    console.log(`  address   ${cut(w.address)}   public: where payments go`);
    console.log(`  uivk      ${cut(w.uivk)}   incoming viewing key: sees payments in, can't spend`);
    console.log(`  seed      ${w.secretFile}   0600, never printed: the only key that can spend`);
    if (role === 'merchant') {
      const envFile = join(dir, 'merchant.env');
      writeFileSync(envFile, `# public address and incoming viewing key (no spending power)\nKAGE_PAY_TO=${w.address}\nX402Z_MERCHANT_UIVK=${w.uivk}\n`, { mode: 0o600 });
      console.log(`\n  ${envFile}  →  KAGE_PAY_TO for your API, X402Z_MERCHANT_UIVK for the facilitator`);
      console.log(`  Move ${w.secretFile} off the server. The facilitator never needs it.`);
    } else {
      console.log(`\n  Fund the address with testnet ZEC, then: kage wallet status --dir ${dir}`);
    }
    return;
  }
  if (cmd === 'wallet' && sub === 'status') {
    const dir = flag('dir', '.kage/agent');
    const { seed, birthday } = readSeedFile(join(dir, 'seed.env'));
    const kage = await createKage({ wallet: { seed, birthday, dir }, policy: { dailyCap: 0n, allowedHosts: [] } });
    try {
      const w = await kage.wallet();
      console.log(`address    ${w.address}`);
      console.log(`uivk       ${w.uivk}`);
      console.log(`spendable  ${formatZec(w.spendableZat)} ZEC  (total ${formatZec(w.totalZat)}, pending ${formatZec(w.pendingZat)})`);
      console.log(`chain tip  ${w.chainTip}`);
    } finally {
      kage.close();
    }
    return;
  }
  if (cmd === 'keys' && sub === 'check') {
    const key = args[2] ?? '';
    const kind = keyKind(key);
    const what = {
      uivk: 'incoming viewing key: sees payments received, cannot spend, cannot see payments sent. Safe for a facilitator.',
      ufvk: 'FULL viewing key: also reveals every payment this wallet sends. Do not give it to a facilitator; derive the UIVK.',
      address: 'unified address: public, where payments are sent.',
      unknown: 'not a Zcash unified key or address.',
    }[kind];
    console.log(`${kind}: ${what}`);
    process.exitCode = kind === 'unknown' ? 1 : 0;
    return;
  }
  if (cmd === 'facilitator') {
    const bin = enginePath('x402z-facilitator');
    if (sub !== 'keygen') assertIncomingOnly(process.env.X402Z_MERCHANT_UIVK ?? '');
    const child = spawn(bin, sub === 'keygen' ? ['keygen'] : [], { stdio: 'inherit' });
    child.on('exit', (code) => process.exit(code ?? 1));
    return;
  }
  console.log(readFileSync(new URL(import.meta.url)).toString().split('\n').slice(2, 9).map((l) => l.replace(/^ \*\s?/, '')).join('\n'));
  process.exitCode = cmd ? 2 : 0;
}

main().catch((e) => {
  const code = e instanceof KageError || e instanceof KageSetupError ? e.code : 'ERROR';
  console.error(`kage: ${code}: ${e.message}`);
  process.exit(2);
});
