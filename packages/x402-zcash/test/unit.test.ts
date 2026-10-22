// Pure functions: the x402 v1 spec's own 402 example next to a zcash-shielded option, ZIP 321
// rendering (same strings as x402z-protocol's Rust tests), X-PAYMENT round trip, receipt check.
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { generateKeyPairSync, sign } from 'node:crypto';
import { decodePaymentResponse, parsePaymentRequired, verifyReceipt, X402ZcashError, xPaymentHeader, zecDecimal, zip321Uri } from '../src/index.ts';

const UA = 'utest1sampleaddressforschemaonly000000';

const body = {
  x402Version: 1,
  error: 'X-PAYMENT header is required',
  accepts: [
    // the coinbase x402 v1 spec's example option (EVM `exact`) is skipped
    { scheme: 'exact', network: 'base-sepolia', maxAmountRequired: '10000', resource: 'https://api.example.com/premium-data', description: 'Access to premium market data', mimeType: 'application/json', payTo: '0x209693Bc6afc0C5328bA36FaF03C514EF312287C', maxTimeoutSeconds: 60, asset: '0x036CbD53842c5426634e7929541eC2318f3dCF7e', extra: { name: 'USDC', version: '2' } },
    { scheme: 'zcash-shielded', network: 'zcash-testnet', maxAmountRequired: '100000', resource: 'http://kage.test/api/quote', description: 'd', mimeType: 'application/json', payTo: UA, maxTimeoutSeconds: 900, asset: 'ZEC', extra: { memoNonce: 'x402z-1', expiresAt: 2_000_000_000, paymentUri: 'zcash:x' } },
  ],
};

test('picks and checks the zcash-shielded testnet option', () => {
  const c = parsePaymentRequired(body, 1_900_000_000);
  assert.deepEqual([c.payTo, c.amountZat, c.memoNonce, c.expiresAt], [UA, 100000n, 'x402z-1', 2_000_000_000]);
  assert.throws(() => parsePaymentRequired(body, 2_000_000_000), /expired/);
  assert.throws(() => parsePaymentRequired({ ...body, accepts: [body.accepts[0]] }), /no zcash-shielded/);
  assert.throws(() => parsePaymentRequired({ ...body, x402Version: 2 }), X402ZcashError);
  const bad = structuredClone(body);
  bad.accepts[1].extra!.memoNonce = 'has space';
  assert.throws(() => parsePaymentRequired(bad, 1), /memoNonce/);
});

test('ZIP 321: decimal ZEC without trailing zeros and a base64url memo without padding', () => {
  assert.equal(zecDecimal(100_000n), '0.001');
  assert.equal(zecDecimal(150_000_000n), '1.5');
  assert.equal(zecDecimal(100_000_000n), '1');
  assert.equal(zecDecimal(1n), '0.00000001');
  // the same URI x402z-server puts in extra.paymentUri for this challenge
  assert.equal(zip321Uri(UA, 100_000n, 'x402z-1'), `zcash:${UA}?amount=0.001&memo=eDQwMnotMQ`);
});

test('X-PAYMENT is base64 JSON with the payload x402z-server reads; bad shapes are refused', () => {
  const tx = 'AB'.repeat(32);
  const v = JSON.parse(Buffer.from(xPaymentHeader(tx, 'x402z-1'), 'base64').toString());
  assert.deepEqual(v, { x402Version: 1, scheme: 'zcash-shielded', network: 'zcash-testnet', payload: { txid: 'ab'.repeat(32), memoNonce: 'x402z-1' } });
  assert.throws(() => xPaymentHeader('zz', 'x402z-1'), /64 hex/);
  assert.throws(() => xPaymentHeader(tx, 'n'.repeat(65)), /memoNonce/);
  const r = Buffer.from(JSON.stringify({ success: true, transaction: tx, network: 'zcash-testnet', payer: '' })).toString('base64');
  assert.deepEqual(decodePaymentResponse(r), { success: true, transaction: tx, network: 'zcash-testnet', payer: '' });
});

test('receipts verify only under the facilitator key and only unchanged', () => {
  const { privateKey, publicKey } = generateKeyPairSync('ed25519');
  const pub = publicKey.export({ format: 'der', type: 'spki' }).subarray(12).toString('hex');
  const claims = Buffer.from(JSON.stringify({ memoNonce: 'x402z-1', amountZat: 100000, payTo: UA, txid: 'ab'.repeat(32), confirmations: 1, issuedAt: 1 }));
  const receipt = `${claims.toString('base64url')}.${sign(null, claims, privateKey).toString('base64url')}`;
  assert.equal(verifyReceipt(receipt, pub).amountZat, 100000);
  const other = generateKeyPairSync('ed25519').publicKey.export({ format: 'der', type: 'spki' }).subarray(12).toString('hex');
  assert.throws(() => verifyReceipt(receipt, other), /signature invalid/);
  const forged = `${Buffer.from(claims.toString().replace('100000', '999999')).toString('base64url')}.${receipt.split('.')[1]}`;
  assert.throws(() => verifyReceipt(forged, pub), /signature invalid/);
});
