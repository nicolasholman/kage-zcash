/**
 * Keys. Kage uses three kinds of Zcash key, and each party holds only what it needs:
 *
 * | Key | Who holds it | What it can do |
 * |---|---|---|
 * | Seed phrase (spending key) | the agent, in a secret store | spend the agent's funds |
 * | UIVK, unified incoming viewing key | the merchant's facilitator | see payments *received* (amount, memo); can't spend, can't see what the wallet sends |
 * | UFVK, unified full viewing key | nobody in Kage | would also see outgoing payments; Kage never asks for it |
 *
 * A merchant's spending key never touches the server: the facilitator verifies every payment
 * with the UIVK alone. A viewing key can't be revoked once shared, so treat it as permanent read
 * access to incoming payments and move to a new account if it leaks.
 */
import { spawnSync } from 'node:child_process';
import { mkdirSync, writeFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { enginePath, KageSetupError } from './engine.ts';

export interface NewWallet {
  /** Shielded unified address (utest1…): where payments are sent. Public. */
  address: string;
  /** Incoming viewing key (uivktest1…): give this, and only this, to the facilitator. */
  uivk: string;
  /** Block height the wallet was created at. */
  birthday: number;
  /** The 0600 file holding the seed phrase. The phrase itself is never returned or printed. */
  secretFile: string;
}

/**
 * Create a fresh testnet wallet with the engine. The seed phrase goes straight into `secretFile`
 * (mode 0600, as `KAGE_SEED=` / `KAGE_BIRTHDAY=` lines); only the address and UIVK come back.
 */
export function createWallet(opts: { dir: string; secretFile?: string; enginePath?: string; lightwalletd?: string }): NewWallet {
  const walletDir = join(opts.dir, 'wallet');
  const secretFile = opts.secretFile ?? join(opts.dir, 'seed.env');
  mkdirSync(walletDir, { recursive: true, mode: 0o700 });
  mkdirSync(dirname(secretFile), { recursive: true, mode: 0o700 });
  const env: Record<string, string> = { PATH: process.env.PATH ?? '', HOME: process.env.HOME ?? '', X402Z_AGENT_WALLET_DIR: walletDir };
  if (opts.lightwalletd) env.ZCASH_LWD_URL = opts.lightwalletd;
  const r = spawnSync(enginePath('x402z-agent', opts.enginePath), ['new-wallet'], { env, encoding: 'utf8' });
  if (r.status !== 0) {
    // stderr only: stdout would hold the phrase if the engine got that far
    throw new KageSetupError(`x402z-agent new-wallet failed (exit ${r.status}): ${(r.stderr ?? '').trim().split('\n').slice(-2).join(' ')}`);
  }
  const get = (k: string) => new RegExp(`^${k}="?([^"\\n]+)"?$`, 'm').exec(r.stdout)?.[1];
  const phrase = get('X402Z_AGENT_SEED');
  const birthday = Number(get('X402Z_AGENT_BIRTHDAY'));
  const address = get('address');
  const uivk = get('X402Z_MERCHANT_UIVK');
  if (!phrase || !address || !uivk || !Number.isFinite(birthday)) throw new KageSetupError('x402z-agent new-wallet printed an unexpected format; nothing was saved');
  writeFileSync(secretFile, `# Kage wallet seed. Anyone with this file can spend the wallet. Keep it in a secret store.\nKAGE_SEED="${phrase}"\nKAGE_BIRTHDAY=${birthday}\n`, { mode: 0o600, flag: 'wx' });
  return { address, uivk, birthday, secretFile };
}

/** What a key string is, from its prefix (ZIP 316 revision 0 and 2). Never logs the key. */
export function keyKind(key: string): 'uivk' | 'ufvk' | 'address' | 'unknown' {
  const k = key.trim();
  if (/^(uivk|uivktest|uvi|uvitest)1/.test(k)) return 'uivk';
  if (/^(uview|uviewtest|uvf|uvftest)1/.test(k)) return 'ufvk';
  if (/^(u|utest)1/.test(k)) return 'address';
  return 'unknown';
}

/** Throw unless `key` is an incoming viewing key: a facilitator must never be handed a full viewing key or a seed. */
export function assertIncomingOnly(key: string): void {
  const kind = keyKind(key);
  if (kind !== 'uivk') {
    throw new KageSetupError(
      kind === 'ufvk'
        ? 'this is a full viewing key (UFVK): it also reveals outgoing payments. Give the facilitator the incoming viewing key (UIVK) only.'
        : 'not a unified incoming viewing key (expected uivktest1… on testnet)',
    );
  }
}
