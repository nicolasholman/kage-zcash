/**
 * ZEC amounts. Policies and prices are written as ZEC (`"0.001 ZEC"`, `"0.001"`) or as zatoshi
 * (`100_000n`); everything below this module works in zatoshi (1 ZEC = 100,000,000 zat).
 */

export const ZAT_PER_ZEC = 100_000_000n;

/** A ZEC amount: `"0.001 ZEC"`, `"0.001 TAZ"`, `"0.001"`, or zatoshi as a bigint. */
export type Zec = string | bigint;

const ZEC_RE = /^(\d+)(?:\.(\d{1,8}))?(?:\s*(?:ZEC|TAZ))?$/i;

/** Parse a {@link Zec} amount to zatoshi. Throws on anything that isn't a non-negative amount with at most 8 decimals. */
export function toZat(amount: Zec): bigint {
  if (typeof amount === 'bigint') {
    if (amount < 0n) throw new RangeError(`amount must not be negative, got ${amount}`);
    return amount;
  }
  const m = ZEC_RE.exec(amount.trim());
  if (!m) throw new RangeError(`not a ZEC amount: "${amount}" (write e.g. "0.001 ZEC", or zatoshi as a bigint)`);
  return BigInt(m[1]) * ZAT_PER_ZEC + BigInt((m[2] ?? '').padEnd(8, '0'));
}

/** Zatoshi as a ZEC string without trailing zeros, e.g. `100000n` -> `"0.001"`. */
export function formatZec(zat: bigint | number): string {
  const z = BigInt(zat);
  const whole = z / ZAT_PER_ZEC;
  const frac = (z % ZAT_PER_ZEC).toString().padStart(8, '0').replace(/0+$/, '');
  return frac ? `${whole}.${frac}` : `${whole}`;
}
