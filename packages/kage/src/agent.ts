/**
 * Kage for agents: `fetch` that pays HTTP 402s with shielded Zcash, inside a spend policy.
 *
 * ```ts
 * import { createKage } from 'kage-x402/agent';
 *
 * const kage = await createKage({
 *   wallet: { seed: process.env.KAGE_AGENT_SEED!, birthday: 4_540_000, dir: '.kage/agent' },
 *   policy: {
 *     dailyCap: '0.05 ZEC',
 *     allowedHosts: ['api.example.com'],
 *     approvalThreshold: '0.01 ZEC',
 *   },
 *   onApproval: async (p) => confirm(`Pay ${p.amount} ZEC to ${p.host}?`),
 * });
 * const res = await kage.fetch('https://api.example.com/v1/report'); // 402 -> policy -> pay -> 200
 * ```
 *
 * The policy is enforced by the engine before any transaction exists: a refused payment builds
 * nothing and broadcasts nothing. Payments at or above `approvalThreshold` are paid only after
 * `onApproval` says yes; the approval goes to the engine over a secret only this process holds,
 * so an LLM driving the agent cannot approve its own payments.
 */
import { randomBytes } from 'node:crypto';
import { mkdirSync } from 'node:fs';
import { join } from 'node:path';
import { EngineProcess, enginePath, KageSetupError } from './engine.ts';
import { formatZec, toZat, type Zec } from './units.ts';

export { KageSetupError } from './engine.ts';

export interface KagePolicy {
  /** Most the agent may spend per UTC day, counting every broadcast payment. */
  dailyCap: Zec;
  /** Hosts the agent may pay (`api.example.com`, `*.example.com`, `127.0.0.1:4300`). Empty means none. */
  allowedHosts: string[];
  /** Payments at or above this need `onApproval`. Omit to never ask. */
  approvalThreshold?: Zec;
  /** Allow plain http to non-loopback hosts (default false: https only, or http to loopback). */
  allowInsecureHttp?: boolean;
}

export interface KageWallet {
  /** 24-word BIP 39 phrase of the agent's own testnet wallet. Load it from a secret store, never hard-code it. */
  seed: string;
  /** Block height the wallet was created at (scanning starts here). */
  birthday?: number;
  /** Directory for the wallet database, the sealed ledger and Kage Tab tokens. */
  dir: string;
}

export interface ApprovalRequest {
  url: string;
  host: string;
  amountZat: bigint;
  /** `amountZat` as ZEC, e.g. "0.02". */
  amount: string;
  thresholdZat: bigint;
}

export type KageEvent =
  | { type: 'request'; url: string }
  | { type: 'approval_requested'; request: ApprovalRequest }
  | { type: 'approved'; request: ApprovalRequest }
  | { type: 'refused'; url: string; code: KageErrorCode; message: string }
  | { type: 'broadcast'; url: string; txid: string; amountZat: bigint; memoNonce: string; explorer: string }
  | { type: 'waiting'; url: string; txid: string; elapsedMs: number }
  | { type: 'settled'; url: string; txid: string; amountZat: bigint; explorer: string; status: number }
  | { type: 'log'; line: string };

export interface KageOptions {
  wallet: KageWallet;
  policy: KagePolicy;
  /** Asked for every payment at or above `policy.approvalThreshold`. Without it those are refused. */
  onApproval?: (req: ApprovalRequest) => boolean | Promise<boolean>;
  /** Every step of every payment, for logs and UIs. */
  onEvent?: (e: KageEvent) => void;
  /** Name in the ledger (default "agent-1"). */
  name?: string;
  /** Key that seals the ledger at rest (64 hex chars). Default: derived from the seed. */
  ledgerKey?: string;
  /** lightwalletd (default https://testnet.zec.rocks:443). */
  lightwalletd?: string;
  /** Directory with the engine binaries (default: $KAGE_ENGINE_DIR or the platform package). */
  enginePath?: string;
  /** How often to present a broadcast payment again while it waits for its block (default 15 s). */
  pollMs?: number;
  /** Give up waiting for settlement after this long (default 15 min); the payment stays resumable. */
  settleTimeoutMs?: number;
}

export type KageErrorCode =
  | 'CAP_EXCEEDED'
  | 'HOST_NOT_ALLOWED'
  | 'POLICY_DENIED'
  | 'APPROVAL_REQUIRED'
  | 'APPROVAL_DENIED'
  | 'APPROVAL_INVALID'
  | 'INSUFFICIENT_FUNDS'
  | 'WALLET'
  | 'PAYMENT_REJECTED'
  | 'ALREADY_REDEEMED'
  | 'SETTLEMENT_TIMEOUT'
  | 'SETUP_REQUIRED'
  | 'INVALID_ARGUMENT'
  | 'NETWORK';

/** A refused or failed payment. `paid` is false unless a transaction was broadcast. */
export class KageError extends Error {
  readonly code: KageErrorCode;
  readonly paid: boolean;
  readonly details: Record<string, unknown>;
  constructor(code: KageErrorCode, message: string, details: Record<string, unknown> = {}, paid = false) {
    super(message);
    this.name = 'KageError';
    this.code = code;
    this.details = details;
    this.paid = paid;
  }
}

export interface Payment {
  txid: string;
  amountZat: bigint;
  amount: string;
  memoNonce: string;
  explorer: string;
}

export interface KageResponse {
  status: number;
  /** Parsed JSON when the body is JSON, else the text. */
  body: unknown;
  /** Set when this call paid (or resumed a payment); absent for a free resource. */
  payment?: Payment;
  /** The server's X-PAYMENT-RESPONSE. */
  settlement?: Record<string, unknown>;
}

export interface SpendStatus {
  spentTodayZat: bigint;
  dailyCapZat: bigint;
  remainingZat: bigint;
  approvalThresholdZat: bigint | null;
  allowedHosts: string[];
  network: string;
}

export interface WalletStatus {
  address: string;
  /** Incoming viewing key: sees payments this wallet receives, can't spend or see what it sends. */
  uivk: string;
  spendableZat: bigint;
  totalZat: bigint;
  pendingZat: bigint;
  chainTip: number;
}

export interface LedgerEntry {
  ts: number;
  host: string;
  amountZat: number;
  decision: 'allowed' | 'denied' | 'needs_approval';
  reason?: string;
  memoNonce?: string;
  txid?: string;
  url?: string;
  settled?: boolean;
}

function codeFor(kind: string, message: string): KageErrorCode {
  switch (kind) {
    case 'spendguard_refused':
      if (message.includes('daily cap')) return 'CAP_EXCEEDED';
      if (message.includes('allowlist')) return 'HOST_NOT_ALLOWED';
      return 'POLICY_DENIED';
    case 'approval_required':
      return 'APPROVAL_REQUIRED';
    case 'approval_invalid':
      return 'APPROVAL_INVALID';
    case 'wallet':
      return message.includes('insufficient') ? 'INSUFFICIENT_FUNDS' : 'WALLET';
    case 'payment_rejected':
      return 'PAYMENT_REJECTED';
    case 'already_redeemed':
      return 'ALREADY_REDEEMED';
    case 'setup_required':
      return 'SETUP_REQUIRED';
    case 'invalid_arguments':
      return 'INVALID_ARGUMENT';
    default:
      return 'NETWORK';
  }
}

const big = (v: unknown): bigint => BigInt((v as number | string | undefined) ?? 0);

function hostOf(url: string): string {
  try {
    return new URL(url).host;
  } catch {
    return url;
  }
}

export class Kage {
  private engine: EngineProcess;
  private secret: string;
  private opts: KageOptions;

  /** @internal use {@link createKage} */
  constructor(engine: EngineProcess, secret: string, opts: KageOptions) {
    this.engine = engine;
    this.secret = secret;
    this.opts = opts;
  }

  private emit(e: KageEvent) {
    try {
      this.opts.onEvent?.(e);
    } catch {
      /* a listener's bug must not break a payment */
    }
  }

  private fail(url: string, kind: string, v: Record<string, unknown>, paid = false): never {
    const message = String(v.message ?? kind);
    const code = codeFor(kind, message);
    this.emit({ type: 'refused', url, code, message });
    throw new KageError(code, message, v, paid);
  }

  /**
   * GET `url`. On an x402 402 it checks the policy, asks `onApproval` when needed, pays from the
   * agent's wallet (memo = the challenge nonce), waits for the block and returns the paid response.
   * Throws {@link KageError} for every refusal; `error.paid` says whether money moved.
   */
  async fetch(url: string, opts: { wait?: boolean } = {}): Promise<KageResponse> {
    this.emit({ type: 'request', url });
    let r = await this.engine.call('paid_fetch', { url });
    if (r.isError && r.value.error === 'approval_required') {
      const request: ApprovalRequest = {
        url,
        host: String(r.value.host ?? hostOf(url)),
        amountZat: big(r.value.amountZat),
        amount: formatZec(big(r.value.amountZat)),
        thresholdZat: big(r.value.thresholdZat),
      };
      this.emit({ type: 'approval_requested', request });
      const ok = this.opts.onApproval ? await this.opts.onApproval(request) : false;
      if (!ok) {
        const message = this.opts.onApproval
          ? `the operator declined ${request.amount} ZEC to ${request.host}; nothing was paid`
          : `${request.amount} ZEC to ${request.host} needs approval (threshold ${formatZec(request.thresholdZat)} ZEC) and no onApproval handler is set; nothing was paid`;
        const code: KageErrorCode = this.opts.onApproval ? 'APPROVAL_DENIED' : 'APPROVAL_REQUIRED';
        this.emit({ type: 'refused', url, code, message });
        throw new KageError(code, message, { ...request });
      }
      this.emit({ type: 'approved', request });
      r = await this.engine.call('paid_fetch', { url, approval: { secret: this.secret, maxZat: Number(request.amountZat) } });
    }
    if (r.isError) this.fail(url, String(r.value.error), r.value);
    return this.settle(url, r.value, opts.wait ?? true);
  }

  private async settle(url: string, v: Record<string, unknown>, wait: boolean): Promise<KageResponse> {
    const started = Date.now();
    const pollMs = this.opts.pollMs ?? 15_000;
    const timeoutMs = this.opts.settleTimeoutMs ?? 15 * 60_000;
    if (v.status === 'settlement_pending') {
      const txid = String(v.txid);
      const memoNonce = String(v.memoNonce);
      const amountZat = big(v.amountZat);
      const explorer = String(v.explorer);
      this.emit({ type: 'broadcast', url, txid, amountZat, memoNonce, explorer });
      if (!wait) return { status: 402, body: v, payment: { txid, amountZat, amount: formatZec(amountZat), memoNonce, explorer } };
      while (v.status === 'settlement_pending') {
        if (Date.now() - started > timeoutMs) {
          throw new KageError('SETTLEMENT_TIMEOUT', `tx ${txid} is broadcast but not settled after ${Math.round(timeoutMs / 1000)} s; call kage.fetch(url) again later to present it (it never pays twice)`, { txid, memoNonce }, true);
        }
        await new Promise((res) => setTimeout(res, pollMs));
        this.emit({ type: 'waiting', url, txid, elapsedMs: Date.now() - started });
        const r = await this.engine.call('redeem_payment', { url, txid, memoNonce });
        if (r.isError) this.fail(url, String(r.value.error), r.value, true);
        v = r.value;
      }
    }
    const paid = v.paid as { txid: string; amountZat: number; memoNonce: string; explorer: string } | null | undefined;
    const payment = paid
      ? { txid: paid.txid, amountZat: big(paid.amountZat), amount: formatZec(big(paid.amountZat)), memoNonce: paid.memoNonce, explorer: paid.explorer }
      : undefined;
    if (payment) this.emit({ type: 'settled', url, txid: payment.txid, amountZat: payment.amountZat, explorer: payment.explorer, status: Number(v.status) });
    return { status: Number(v.status), body: v.body, payment, settlement: (v.settlement as Record<string, unknown>) ?? undefined };
  }

  /** Today's spend against the policy (UTC day, broadcast payments only). */
  async status(): Promise<SpendStatus> {
    const r = await this.engine.call('spend_status');
    if (r.isError) this.fail('', String(r.value.error), r.value);
    const v = r.value;
    return {
      spentTodayZat: big(v.spentTodayZat),
      dailyCapZat: big(v.dailyCapZat),
      remainingZat: big(v.remainingZat),
      approvalThresholdZat: v.approvalThresholdZat == null ? null : big(v.approvalThresholdZat),
      allowedHosts: (v.allowedHosts as string[]) ?? [],
      network: String(v.network),
    };
  }

  /** Sync the wallet; its address, incoming viewing key and balance. */
  async wallet(): Promise<WalletStatus> {
    const r = await this.engine.call('wallet_status');
    if (r.isError) this.fail('', String(r.value.error), r.value);
    const v = r.value;
    return {
      address: String(v.address),
      uivk: String(v.uivk),
      spendableZat: big(v.spendableZat),
      totalZat: big(v.totalZat),
      pendingZat: big(v.pendingZat),
      chainTip: Number(v.chainTip),
    };
  }

  /** The last `n` policy decisions (allowed, denied, needs_approval), newest last. */
  async ledger(n = 10): Promise<LedgerEntry[]> {
    const r = await this.engine.call('ledger_tail', { n });
    if (r.isError) this.fail('', String(r.value.error), r.value);
    return (r.value.entries as LedgerEntry[]) ?? [];
  }

  /** Stop the engine. Payments in flight stay in the ledger and resume on the next `fetch` of the same URL. */
  close() {
    this.engine.close();
  }
}

/** Start the Kage engine with this wallet and policy. */
export async function createKage(opts: KageOptions): Promise<Kage> {
  if (!opts?.wallet?.seed?.trim()) throw new KageSetupError('Setup required: wallet.seed is empty (load the agent seed from your secret store, e.g. process.env.KAGE_AGENT_SEED)');
  if (!opts.wallet.dir) throw new KageSetupError('Setup required: wallet.dir is empty');
  if (!opts.policy) throw new KageSetupError('Setup required: policy is missing (dailyCap, allowedHosts)');
  const cap = toZat(opts.policy.dailyCap);
  const threshold = opts.policy.approvalThreshold == null ? undefined : toZat(opts.policy.approvalThreshold);
  mkdirSync(opts.wallet.dir, { recursive: true, mode: 0o700 });
  const secret = randomBytes(32).toString('hex');
  const env: Record<string, string> = {
    X402Z_AGENT_SEED: opts.wallet.seed.trim(),
    X402Z_AGENT_WALLET_DIR: join(opts.wallet.dir, 'wallet'),
    X402Z_AGENT_LEDGER: join(opts.wallet.dir, 'ledger.json'),
    X402Z_AGENT_DAILY_CAP_ZAT: cap.toString(),
    X402Z_AGENT_ALLOWED_HOSTS: opts.policy.allowedHosts.join(','),
    X402Z_AGENT_NAME: opts.name ?? 'agent-1',
    X402Z_MCP_APPROVAL_SECRET: secret,
  };
  if (opts.wallet.birthday != null) env.X402Z_AGENT_BIRTHDAY = String(opts.wallet.birthday);
  if (threshold != null) env.X402Z_AGENT_APPROVAL_ZAT = threshold.toString();
  if (opts.policy.allowInsecureHttp) env.X402Z_AGENT_ALLOW_HTTP = '1';
  if (opts.ledgerKey) env.X402Z_AGENT_LEDGER_KEY = opts.ledgerKey;
  if (opts.lightwalletd) env.ZCASH_LWD_URL = opts.lightwalletd;
  const engine = new EngineProcess(enginePath('x402z-mcp', opts.enginePath), env, (line) => opts.onEvent?.({ type: 'log', line }));
  await engine.request('initialize', { protocolVersion: '2025-06-18', capabilities: {}, clientInfo: { name: 'kage-x402', version: '0.2.0' } });
  engine.notify('notifications/initialized');
  return new Kage(engine, secret, opts);
}
