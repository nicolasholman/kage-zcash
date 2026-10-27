# Kage Market Data — a paid API in ~60 lines

Live Zcash market data sold per call for shielded ZEC with [`kage-x402`](../../packages/kage):

| Endpoint | Price | Returns |
|---|---|---|
| `GET /` | free | the catalog |
| `GET /v1/zec/price` | 0.001 ZEC | ZEC/USD, 24h change, volume, market cap (CoinGecko) |
| `GET /v1/zec/report` | 0.005 ZEC | 7-day open/close/high/low, trend, daily volatility, Zcash network activity (CoinGecko + Blockchair) |

The server holds no keys. Payments go to `KAGE_PAY_TO`; the facilitator verifies them with the
merchant's incoming viewing key and signs receipts the paywall checks.

```sh
kage wallet new --dir ../../.kage/merchant --role merchant   # address + UIVK; seed to a 0600 file
kage facilitator keygen                                      # receipt key pair
# facilitator env: X402Z_MERCHANT_UIVK, X402Z_RECEIPT_SIGNING_KEY, X402Z_FACILITATOR_TOKEN
kage facilitator                                             # refuses anything but a UIVK
cp .env.example .env    # KAGE_PAY_TO, KAGE_FACILITATOR_URL/TOKEN, KAGE_RECEIPT_PUBKEY
npm install && npm start                                     # http://127.0.0.1:4300
curl -i http://127.0.0.1:4300/v1/zec/price                   # 402 with the x402 offer
```
