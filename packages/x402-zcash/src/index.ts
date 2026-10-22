/**
 * x402 v1 over Zcash shielded payments (Kage), for TypeScript.
 *
 * Client side:
 * - {@link parsePaymentRequired}: the `zcash-shielded` / `zcash-testnet` entry of an x402 v1 402 body;
 * - {@link zip321Uri}: the ZIP 321 request any Zcash wallet can pay (memo = the challenge nonce);
 * - {@link xPaymentHeader} / {@link fetchWithPayment}: present `{txid, memoNonce}` with `X-PAYMENT`;
 * - {@link decodePaymentResponse}: read `X-PAYMENT-RESPONSE`.
 *
 * Server side: {@link paywall}, a Node / Express middleware that answers 402 with a fresh challenge
 * and settles an `X-PAYMENT` through the Kage facilitator's `/settle` (bearer token), accepting
 * only an ed25519 receipt from the facilitator's key, each nonce once.
 *
 * Only erasable TypeScript syntax is used, so Node >= 22.18 runs the sources directly.
 */
import { createPublicKey, randomBytes, verify as edVerify } from 'node:crypto';
import type { IncomingMessage, ServerResponse } from 'node:http';

export const X402_VERSION = 1;
export const SCHEME = 'zcash-shielded';
export const NETWORK = 'zcash-testnet';
export const PAYMENT_HEADER = 'X-PAYMENT';
export const PAYMENT_RESPONSE_HEADER = 'X-PAYMENT-RESPONSE';
const NONCE_RE = /^[A-Za-z0-9_-]{1,64}$/;
const TXID_RE = /^[0-9a-f]{64}$/;

/** One `accepts` entry of an x402 v1 402 body. */
export interface PaymentRequirements {
  scheme: string;
  network: string;
  maxAmountRequired: string;
  resource: string;
  description: string;
  mimeType?: string;
  payTo: string;
  maxTimeoutSeconds: number;
  asset: string;
  extra?: { memoNonce?: string; expiresAt?: number; paymentUri?: string; tab?: unknown; [k: string]: unknown };
}

/** The payable Zcash challenge of a 402. */
export interface ZcashChallenge {
  payTo: string;
  amountZat: bigint;
  memoNonce: string;
  expiresAt: number;
  resource: string;
  /** ZIP 321 URI the server put in `extra.paymentUri`, if any. */
  paymentUri?: string;
  requirements: PaymentRequirements;
}

/** `X-PAYMENT-RESPONSE` (and the facilitator's settlement fields). */
export interface SettlementResponse {
  success: boolean;
  errorReason?: string;
  transaction: string;
  network: string;
  payer: string;
}

export class X402ZcashError extends Error {}

/** Pick and check the `zcash-shielded` testnet option of an x402 v1 402 body. */
export function parsePaymentRequired(body: unknown, nowUnix = Math.floor(Date.now() / 1000)): ZcashChallenge {
  const b = body as { x402Version?: unknown; accepts?: unknown };
  if (!b || b.x402Version !== X402_VERSION || !Array.isArray(b.accepts)) {
    throw new X402ZcashError('not an x402 v1 PaymentRequiredResponse ({x402Version: 1, accepts: [...]})');
  }
  const r = (b.accepts as PaymentRequirements[]).find(a => a && a.scheme === SCHEME && a.network === NETWORK);
  if (!r) throw new X402ZcashError(`402 offers no ${SCHEME} / ${NETWORK} option`);
  const memoNonce = r.extra?.memoNonce;
  const expiresAt = r.extra?.expiresAt;
  if (typeof memoNonce !== 'string' || !NONCE_RE.test(memoNonce)) throw new X402ZcashError('extra.memoNonce missing or malformed');
  if (typeof expiresAt !== 'number') throw new X402ZcashError('extra.expiresAt missing');
  if (!/^[0-9]+$/.test(r.maxAmountRequired)) throw new X402ZcashError(`maxAmountRequired '${r.maxAmountRequired}' is not a zatoshi integer`);
  if (!r.payTo.startsWith('utest1')) throw new X402ZcashError('payTo is not a testnet unified address');
  if (expiresAt <= nowUnix) throw new X402ZcashError(`challenge expired at ${expiresAt}`);
  const amountZat = BigInt(r.maxAmountRequired);
  if (amountZat === 0n) throw new X402ZcashError('amount is zero');
  return { payTo: r.payTo, amountZat, memoNonce, expiresAt, resource: r.resource, paymentUri: r.extra?.paymentUri, requirements: r };
}

/** Zatoshi as a ZIP 321 `amount`: decimal ZEC, at most 8 decimals, no trailing zeros. */
export function zecDecimal(zat: bigint): string {
  const whole = zat / 100_000_000n;
  const frac = zat % 100_000_000n;
  if (frac === 0n) return whole.toString();
  return `${whole}.${frac.toString().padStart(8, '0').replace(/0+$/, '')}`;
}

/** `zcash:<payTo>?amount=<ZEC>&memo=<base64url(memo), no padding>` (ZIP 321). */
export function zip321Uri(payTo: string, amountZat: bigint, memoText: string): string {
  return `zcash:${payTo}?amount=${zecDecimal(amountZat)}&memo=${Buffer.from(memoText, 'utf8').toString('base64url')}`;
}

/** Base64 `{x402Version: 1, scheme, network, payload: {txid, memoNonce}}`. */
export function xPaymentHeader(txid: string, memoNonce: string): string {
  const t = txid.trim().toLowerCase();
  if (!TXID_RE.test(t)) throw new X402ZcashError('txid must be 64 hex characters');
  if (!NONCE_RE.test(memoNonce)) throw new X402ZcashError('memoNonce must be 1-64 of [A-Za-z0-9_-]');
  const p = { x402Version: X402_VERSION, scheme: SCHEME, network: NETWORK, payload: { txid: t, memoNonce } };
  return Buffer.from(JSON.stringify(p)).toString('base64');
}

function decodeB64Json<T>(h: string): T {
  const s = h.trim();
  const raw = Buffer.from(s.replace(/-/g, '+').replace(/_/g, '/'), 'base64').toString('utf8');
  return JSON.parse(raw) as T;
}

/** Decode `X-PAYMENT-RESPONSE`. */
export function decodePaymentResponse(header: string): SettlementResponse {
  return decodeB64Json<SettlementResponse>(header);
}

/** GET `url` presenting a payment already made; returns status, settlement and the parsed body. */
export async function fetchWithPayment(
  url: string,
  payment: { txid: string; memoNonce: string },
  init: RequestInit = {},
): Promise<{ status: number; settlement?: SettlementResponse; body: unknown; challenge?: ZcashChallenge }> {
  if (!['GET', undefined].includes(init.method)) throw new X402ZcashError('fetchWithPayment only does GET');
  const headers = new Headers(init.headers);
  headers.set(PAYMENT_HEADER, xPaymentHeader(payment.txid, payment.memoNonce));
  const r = await fetch(url, { ...init, headers, redirect: 'manual' });
  if (r.status >= 300 && r.status < 400) {
    throw new X402ZcashError(`${url} answered ${r.status} redirecting to ${r.headers.get('location')}; not following a redirect with X-PAYMENT`);
  }
  const h = r.headers.get(PAYMENT_RESPONSE_HEADER);
  const text = await r.text();
  let body: unknown = text;
  try { body = JSON.parse(text); } catch { /* not JSON */ }
  let challenge: ZcashChallenge | undefined;
  if (r.status === 402) {
    try { challenge = parsePaymentRequired(body); } catch { /* a 402 without a payable option */ }
  }
  return { status: r.status, settlement: h ? decodePaymentResponse(h) : undefined, body, challenge };
}

// ---------------------------------------------------------------------------------------------
// Paywall middleware
// ---------------------------------------------------------------------------------------------

export interface StoredChallenge {
  memoNonce: string;
  payTo: string;
  amountZat: bigint;
  expiresAt: number;
  /** Txid the nonce was redeemed with, once spent. */
  spentTxid?: string;
}

/** Where issued challenges live. The default keeps them in memory (one process). */
export interface ChallengeStore {
  add(c: StoredChallenge): void;
  get(nonce: string): StoredChallenge | undefined;
  /** Burn `nonce` for `txid`; false if it was already spent. */
  markSpent(nonce: string, txid: string): boolean;
}

export function memoryStore(): ChallengeStore {
  const m = new Map<string, StoredChallenge>();
  return {
    add: c => { m.set(c.memoNonce, { ...c }); },
    get: n => m.get(n),
    markSpent: (n, txid) => {
      const c = m.get(n);
      if (!c || c.spentTxid) return false;
      c.spentTxid = txid;
      return true;
    },
  };
}

export interface PaywallOptions {
  /** Kage facilitator base URL, e.g. http://127.0.0.1:4021. */
  facilitatorUrl: string;
  /** X402Z_FACILITATOR_TOKEN of that facilitator. */
  facilitatorToken: string;
  /** Hex ed25519 public key of the facilitator's receipt key (X402Z_RECEIPT_PUBKEY). */
  receiptPubkeyHex: string;
  /** Merchant shielded unified address (utest1...). */
  payTo: string;
  amountZat: bigint;
  description?: string;
  mimeType?: string;
  ttlSecs?: number;
  /** A payment mined up to this long after expiresAt still counts (default 1200 s, as the facilitator). */
  graceSecs?: number;
  minConfirmations?: number;
  store?: ChallengeStore;
}

type Req = IncomingMessage & { protocol?: string };
type Next = (err?: unknown) => void;

interface ReceiptClaims {
  memoNonce: string;
  amountZat: number;
  payTo: string;
  txid: string;
  confirmations: number;
  issuedAt: number;
  minedAt?: number;
}

/** Check an `X-PAYMENT-RECEIPT`-format receipt (`base64url(claims).base64url(sig)`) with the facilitator key. */
export function verifyReceipt(receipt: string, pubkeyHex: string): ReceiptClaims {
  const [c, s] = receipt.trim().split('.');
  if (!c || !s) throw new X402ZcashError('receipt malformed');
  const claims = Buffer.from(c, 'base64url');
  const key = createPublicKey({ key: Buffer.concat([Buffer.from('302a300506032b6570032100', 'hex'), Buffer.from(pubkeyHex, 'hex')]), format: 'der', type: 'spki' });
  if (!edVerify(null, claims, key, Buffer.from(s, 'base64url'))) throw new X402ZcashError('receipt signature invalid');
  return JSON.parse(claims.toString('utf8')) as ReceiptClaims;
}

/**
 * Express / Node middleware: no `X-PAYMENT` => 402 with a fresh `zcash-shielded` challenge;
 * `X-PAYMENT` => settle it through the facilitator, then `next()` with `X-PAYMENT-RESPONSE` set.
 * Pending payments get the same challenge back with `settlement_pending`; refusals a fresh one.
 */
export function paywall(opts: PaywallOptions) {
  if (!opts.facilitatorToken) throw new X402ZcashError('Setup required: facilitatorToken (X402Z_FACILITATOR_TOKEN) not configured');
  if (!/^[0-9a-f]{64}$/i.test(opts.receiptPubkeyHex)) throw new X402ZcashError('Setup required: receiptPubkeyHex must be 64 hex (X402Z_RECEIPT_PUBKEY)');
  if (!opts.payTo.startsWith('utest1')) throw new X402ZcashError('payTo must be a testnet unified address (utest1...)');
  const store = opts.store ?? memoryStore();
  const ttl = opts.ttlSecs ?? 900;
  const grace = opts.graceSecs ?? 1200;
  const minConf = opts.minConfirmations ?? 1;
  const settleUrl = `${opts.facilitatorUrl.replace(/\/$/, '')}/settle`;

  const resourceOf = (req: Req) => `http://${req.headers.host ?? 'localhost'}${req.url ?? '/'}`;
  const requirementsOf = (c: StoredChallenge, resource: string): PaymentRequirements => ({
    scheme: SCHEME, network: NETWORK, maxAmountRequired: c.amountZat.toString(), resource,
    description: opts.description ?? 'paid resource', mimeType: opts.mimeType ?? 'application/json',
    payTo: c.payTo, maxTimeoutSeconds: ttl, asset: 'ZEC',
    extra: { memoNonce: c.memoNonce, expiresAt: c.expiresAt, paymentUri: zip321Uri(c.payTo, c.amountZat, c.memoNonce) },
  });
  const send = (res: ServerResponse, status: number, body: unknown, settlement?: SettlementResponse) => {
    if (settlement) res.setHeader(PAYMENT_RESPONSE_HEADER, Buffer.from(JSON.stringify(settlement)).toString('base64'));
    res.statusCode = status;
    res.setHeader('content-type', 'application/json');
    res.end(JSON.stringify(body));
  };
  const failed = (reason: string): SettlementResponse => ({ success: false, errorReason: reason, transaction: '', network: NETWORK, payer: '' });
  const fresh = (): StoredChallenge => {
    const c = { memoNonce: `x402z-${randomBytes(12).toString('hex')}`, payTo: opts.payTo, amountZat: opts.amountZat, expiresAt: Math.floor(Date.now() / 1000) + ttl };
    store.add(c);
    return c;
  };
  const challenge = (req: Req, res: ServerResponse, c: StoredChallenge, error: string, settlement?: SettlementResponse) =>
    send(res, 402, { x402Version: X402_VERSION, accepts: [requirementsOf(c, resourceOf(req))], error }, settlement);

  return async function x402ZcashPaywall(req: Req, res: ServerResponse, next: Next): Promise<void> {
    try {
      const header = req.headers['x-payment'];
      if (typeof header !== 'string') return challenge(req, res, fresh(), `${PAYMENT_HEADER} header is required`);
      let p: { x402Version?: number; scheme?: string; network?: string; payload?: { txid?: string; memoNonce?: string } };
      try { p = decodeB64Json(header); } catch { return challenge(req, res, fresh(), `${PAYMENT_HEADER} is not base64 JSON`, failed('invalid_payload')); }
      if (p.x402Version !== X402_VERSION || p.scheme !== SCHEME || p.network !== NETWORK) {
        return challenge(req, res, fresh(), `${PAYMENT_HEADER} must be x402Version 1, ${SCHEME}, ${NETWORK}`, failed('invalid_scheme'));
      }
      const txid = String(p.payload?.txid ?? '').trim().toLowerCase();
      const nonce = String(p.payload?.memoNonce ?? '').trim();
      if (!TXID_RE.test(txid) || !NONCE_RE.test(nonce)) return send(res, 400, { error: 'txid must be 64 hex and memoNonce 1-64 of [A-Za-z0-9_-]' }, failed('invalid_payload'));
      const c = store.get(nonce);
      if (!c) return challenge(req, res, fresh(), 'memo nonce was never issued here', failed('unknown_challenge'));
      if (c.spentTxid) return challenge(req, res, fresh(), 'receipt already used', failed('nonce_already_used'));
      const reqs = requirementsOf(c, resourceOf(req));
      let s: { success?: boolean; errorReason?: string; status?: string; receipt?: string; error?: string };
      let code: number;
      try {
        const r = await fetch(settleUrl, {
          method: 'POST',
          headers: { 'content-type': 'application/json', authorization: `Bearer ${opts.facilitatorToken}` },
          body: JSON.stringify({ x402Version: X402_VERSION, paymentPayload: { x402Version: X402_VERSION, scheme: SCHEME, network: NETWORK, payload: { txid, memoNonce: nonce } }, paymentRequirements: reqs }),
        });
        code = r.status;
        s = await r.json() as typeof s;
      } catch (e) {
        return send(res, 502, { error: `facilitator unreachable at ${settleUrl}: ${(e as Error).message}` }, failed('facilitator_error'));
      }
      if (code === 401) return send(res, 502, { error: 'Setup required: the facilitator refused facilitatorToken (401)' }, failed('facilitator_error'));
      if (s.status === 'pending' || s.status === 'not_found') return challenge(req, res, c, s.error ?? 'payment not confirmed yet', failed('settlement_pending'));
      if (s.status === 'rejected') return challenge(req, res, fresh(), s.error ?? 'payment refused', failed(s.errorReason ?? 'invalid_payment'));
      if (s.status !== 'confirmed' || !s.receipt) return send(res, 502, { error: `facilitator: ${s.error ?? 'no receipt'}` }, failed('facilitator_error'));
      let claims: ReceiptClaims;
      try { claims = verifyReceipt(s.receipt, opts.receiptPubkeyHex); } catch (e) {
        return challenge(req, res, fresh(), (e as Error).message, failed('invalid_payment'));
      }
      if (claims.memoNonce !== nonce || claims.txid.toLowerCase() !== txid || claims.payTo !== c.payTo || BigInt(claims.amountZat) < c.amountZat) {
        return challenge(req, res, fresh(), 'receipt does not match the payment that was presented', failed('invalid_payment'));
      }
      if (claims.confirmations < minConf) return challenge(req, res, c, `payment has ${claims.confirmations} confirmation(s), need ${minConf}`, failed('settlement_pending'));
      const paidAt = claims.minedAt ?? Math.floor(Date.now() / 1000);
      if (paidAt > c.expiresAt + grace) return challenge(req, res, fresh(), `payment landed at ${paidAt}, after the challenge expired`, failed('payment_expired'));
      if (!store.markSpent(nonce, txid)) return challenge(req, res, fresh(), 'receipt already used', failed('nonce_already_used'));
      res.setHeader(PAYMENT_RESPONSE_HEADER, Buffer.from(JSON.stringify({ success: true, transaction: txid, network: NETWORK, payer: '' })).toString('base64'));
      next();
    } catch (e) {
      next(e);
    }
  };
}
