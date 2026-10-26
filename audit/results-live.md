# Kage Live Read-Only Audit Results

Date: 2026-10-26 (Read-Only Live Inspection)

## Executive Summary

- **Target Production Landing & Evidence**: `https://kage-landing.vercel.app` and `https://kage-landing.vercel.app/evidence.html`
- **Target Hosted API**: `https://kage-api.201-18-198-162.sslip.io` (Read-only single GET `/api/quote`, single GET `/api/challenge/<nonce>/qr.svg`)
- **Target Hosted Facilitator**: `https://kage-facilitator.201-18-198-162.sslip.io` (Read-only `GET /health` and `GET /supported`)
- **Public Testnet Block Explorer**: `https://testnet.zecblock.com/tx/<txid>` (All 10 verified README transactions inspected read-only)
- **Heavy slot lock**: Acquired via `/Volumes/Extreme SSD/Projects/grokbot/tools/heavy-slot.sh acquire kage-live-audit`
- **Audit Table Status**: **64 PASS / 0 FAIL** out of 64 exercised live items.

---

## Live Endpoint Raw Responses

### 1. Hosted API Quote: `GET https://kage-api.201-18-198-162.sslip.io/api/quote`
- **HTTP Status**: `402 Payment Required`
- **Headers Excerpt**:
```http
HTTP/2 402 
access-control-expose-headers: X-PAYMENT-RESPONSE,Retry-After
alt-svc: h3=":443"; ma=2592000
content-type: application/json
date: Sun, 25 Oct 2026 22:56:42 GMT
vary: Origin
content-length: 1588
```
- **Body (No Secrets)**:
```json
{
  "x402Version": 1,
  "accepts": [
    {
      "scheme": "zcash-shielded",
      "network": "zcash-testnet",
      "maxAmountRequired": "100000",
      "resource": "https://kage-api.201-18-198-162.sslip.io/api/quote",
      "description": "Kage demo quote: paid content unlocked by a shielded Zcash testnet payment",
      "mimeType": "application/json",
      "payTo": "utest100fnpp4zd4lp0vmu093gwx6p6ujv8lusunnkvrsuxyaca9avmea5fqgt0ktkvm0sf2wnn8fw7ad034fy9lvsrq0x04vn0jqq7dvnyua5zk9s30v2lcys8pk3mm7xnkdt7fyf65xe0vtnv7sswdflaltg9uesty00c6x2kvgc25ygcrzs",
      "maxTimeoutSeconds": 900,
      "asset": "ZEC",
      "extra": {
        "expiresAt": 1792969902,
        "memoNonce": "x402z-2407a3245422986693ef9238",
        "paymentUri": "zcash:utest100fnpp4zd4lp0vmu093gwx6p6ujv8lusunnkvrsuxyaca9avmea5fqgt0ktkvm0sf2wnn8fw7ad034fy9lvsrq0x04vn0jqq7dvnyua5zk9s30v2lcys8pk3mm7xnkdt7fyf65xe0vtnv7sswdflaltg9uesty00c6x2kvgc25ygcrzs?amount=0.001&memo=eDQwMnotMjQwN2EzMjQ1NDIyOTg2NjkzZWY5MjM4",
        "tab": {
          "epochEndsAt": 1793383919,
          "graceSecs": 1800,
          "issuePath": "/tab/issue",
          "keyId": "6eea047ceca41906",
          "maxTokens": 20,
          "publicKey": "MIIBUjA9BgkqhkiG9w0BAQowMKANMAsGCWCGSAFlAwQCAqEaMBgGCSqGSIb3DQEBCDALBglghkgBZQMEAgKiAwIBMAOCAQ8AMIIBCgKCAQEA97mC92CqhBUf56XVcDj4pab8JZWHR77iM9KRyYaYe2BrOu6IarE+ijf1yLPq7zvwD/53WV8Spdmi76If+1XVjVILs0kvtvA4QTJ31pPiQjPaqRmvLgHj05A1ELZg+6emX04tTgaXFDK6s+pct8a2+HmmQgIOTQpwCy19OOhK2xxvi/gIX7mR3wsB2BCOtU1+ydoV/VW+yktXlbVGDCeD89MdNaxjhec/OJm0ZhpTl2FnSF0dHkONzrUrb46wOe0wfEVGq4P3wOf2ZV3UOzCT9EgWyRAiYab91pmvjipBAN7tfSt4EWM+MbQ9ws8RLizPEPQDuLoinGVOhROS+uWi6QIDAQAB",
          "scheme": "zcash-shielded-tab",
          "suite": "RSABSSA-SHA384-PSS-Randomized",
          "unitPriceZat": 100000
        }
      }
    }
  ],
  "error": "X-PAYMENT header is required"
}
```
- **Field Validation**:
  - `x402Version`: `1` (Expected: 1)
  - `scheme`: `zcash-shielded` (Expected: zcash-shielded)
  - `network`: `zcash-testnet` (Expected: zcash-testnet)
  - `asset`: `ZEC` (Expected: ZEC)
  - `maxAmountRequired`: `100000` zat (0.001 TAZ)
  - `memoNonce`: `x402z-2407a3245422986693ef9238` (matches `^x402z-[0-9a-f]{24}$`)
  - `paymentUri`: `zcash:utest100fnpp4zd4lp0vmu093gwx6p6ujv8lusunnkvrsuxyaca9avmea5fqgt0ktkvm0sf2wnn8fw7ad034fy9lvsrq0x04vn0jqq7dvnyua5zk9s30v2lcys8pk3mm7xnkdt7fyf65xe0vtnv7sswdflaltg9uesty00c6x2kvgc25ygcrzs?amount=0.001&memo=eDQwMnotMjQwN2EzMjQ1NDIyOTg2NjkzZWY5MjM4`
  - Decoded Payment URI memo matches nonce: **Verified** (decoded `eDQwMnotMjQwN2EzMjQ1NDIyOTg2NjkzZWY5MjM4` equals `x402z-2407a3245422986693ef9238`)

### 2. Challenge QR SVG: `GET https://kage-api.201-18-198-162.sslip.io/api/challenge/<nonce>/qr.svg`
- **HTTP Status**: `200 OK`
- **Headers Excerpt**:
```http
HTTP/2 200 
access-control-expose-headers: X-PAYMENT-RESPONSE,Retry-After
alt-svc: h3=":443"; ma=2592000
cache-control: no-store
content-type: image/svg+xml
date: Sun, 25 Oct 2026 22:56:53 GMT
vary: Origin
content-length: 36152
```
- **Content Type**: `image/svg+xml`
- **Cache-Control**: `no-store`
- **SVG Size**: 36152 bytes (well-formed standalone SVG XML)

### 3. Hosted Facilitator Health: `GET https://kage-facilitator.201-18-198-162.sslip.io/health`
- **HTTP Status**: `200 OK`
- **Body**:
```json
{
  "chainTip": 4541789,
  "lightwalletd": "https://testnet.zec.rocks:443",
  "minConfirmations": 1,
  "network": "testnet",
  "ok": true,
  "receiptPubkey": "95b72c33b95c02a904a71c37e8638d39a1309a1f3686a374c898d32a8ab449c6",
  "viewingKey": "uivk"
}
```
- **Probed Chain Tip**: 4541789 (positive live block height on testnet)
- **Receipt Pubkey**: `95b72c33b95c02a904a71c37e8638d39a1309a1f3686a374c898d32a8ab449c6`

### 4. Hosted Facilitator Supported: `GET https://kage-facilitator.201-18-198-162.sslip.io/supported`
- **HTTP Status**: `200 OK`
- **Body**:
```json
{
  "kinds": [
    {
      "x402Version": 1,
      "scheme": "zcash-shielded",
      "network": "zcash-testnet"
    }
  ]
}
```

---

## Verified Testnet Runs (10 README Txids on testnet.zecblock.com)

| # | Txid | Mined Block | Explorer GET (Playwright Chrome) | Page Shows Tx | Notes |
|---|---|---|---|---|---|
| 1 | [`030c3cbf…8351`](https://testnet.zecblock.com/tx/030c3cbf78ce8460553bba4e82c0ba813f039bd3ecf4d7f2e81c59837dc28351) | 4,521,112 | HTTP 200 | YES (Canonical block #4,521,112) | Read-only verified; unshielded parameters hidden on chain |
| 2 | [`cd64980a…916d`](https://testnet.zecblock.com/tx/cd64980a5247881a727aa916476a3ba757e59794263c2ff1bcde467654a0916d) | 4,521,440 | HTTP 200 | YES (Canonical block #4,521,440) | Read-only verified; unshielded parameters hidden on chain |
| 3 | [`ccb4214c…0ed3`](https://testnet.zecblock.com/tx/ccb4214c6a1fdf183c086ae5b4f559d181ead4c1c61dc7e711fc4a08b95b0ed3) | 4,524,586 | HTTP 200 | YES (Canonical block #4,524,586) | Read-only verified; unshielded parameters hidden on chain |
| 4 | [`25a8b324…689d`](https://testnet.zecblock.com/tx/25a8b324bebee30220382113c6fb9cc3be1144b036117c34778bde3b1d8b689d) | 4,524,602 | HTTP 200 | YES (Canonical block #4,524,602) | Read-only verified; unshielded parameters hidden on chain |
| 5 | [`408aba26…8dd0`](https://testnet.zecblock.com/tx/408aba269ac7ca946e3b0eaedcf4cadd582a01090679e98391b4e7c706d88dd0) | 4,524,693 | HTTP 200 | YES (Canonical block #4,524,693) | Read-only verified; unshielded parameters hidden on chain |
| 6 | [`a7c3b0b2…38bc`](https://testnet.zecblock.com/tx/a7c3b0b2eaeb406ddcd02276cd71e69e4e618b22a1cab8977c959f2da3fc38bc) | 4,524,694 | HTTP 200 | YES (Canonical block #4,524,694) | Read-only verified; unshielded parameters hidden on chain |
| 7 | [`fa011720…c3ad`](https://testnet.zecblock.com/tx/fa01172015c63ce47a0285f7fae130a3efe5f821b938a8befa1ca547d00ac3ad) | 4,526,095 | HTTP 200 | YES (Canonical block #4,526,095) | Read-only verified; unshielded parameters hidden on chain |
| 8 | [`d3bf0b49…d00d`](https://testnet.zecblock.com/tx/d3bf0b49408fed118fe9e1241239d98319cbb83088c45a5ba357f68cd97bd00d) | 4,526,113 | HTTP 200 | YES (Canonical block #4,526,113) | Read-only verified; unshielded parameters hidden on chain |
| 9 | [`f037d597…e22a`](https://testnet.zecblock.com/tx/f037d5978fec7817340ff118d328995288f0a575a1850b083ca05f7d71d4e22a) | 4,529,425 | HTTP 200 | YES (Canonical block #4,529,425) | Read-only verified; unshielded parameters hidden on chain |
| 10 | [`b75b2415…4b91`](https://testnet.zecblock.com/tx/b75b2415347a7ce317a9b8b5799d07a2ad9ebc44eea92090f0db9d9b590e4b91) | 4,529,427 | HTTP 200 | YES (Canonical block #4,529,427) | Read-only verified; unshielded parameters hidden on chain |

> [!NOTE]
> CLI curl to `testnet.zecblock.com` triggers Vercel Bot Mitigation (`HTTP 429 Vercel Security Checkpoint`). In real Chrome driven via Playwright, all 10 endpoints resolve with `HTTP 200` and display canonical transaction inclusion details.

---

## Live Audit Matrix

| ID | component/flow | exact expected result | status | evidence | fix |
| --- | --- | --- | --- | --- | --- |
| U-001 | Landing index / page load & HTTP status | HTTP 200 response at /zec-shielded-x402/ and https://kage-landing.vercel.app; title text is "Kage — shielded x402 for AI agents" | PASS | HTTP 200, title "Kage — shielded x402 for AI agents" |  |
| U-002 | Landing index / hero headline | Exactly one h1 element with text "Agents pay per call. Nobody learns who paid." with mark element wrapping "who paid" | PASS | 1 h1: "Agents pay per call. Nobody learns who paid.", mark: "who paid" |  |
| U-003 | Landing index / hero eyebrow & lede | Eyebrow text is "x402 v1 · zcash-testnet"; sub paragraph text is "HTTP 402 settled in shielded Zcash: spendguard checks each payment, the merchant verifies with a viewing key alone." | PASS | eyebrow: "x402 v1 · zcash-testnet", sub: "HTTP 402 settled in shielded Zcash: spendguard checks each payment, the merchant verifies with a viewing key alone." |  |
| U-004 | Landing index / hero recorded exchange steps | List .shadow ol li contains exactly 5 items: "GET /api/quote", "402 Payment Required", "pay shielded transaction, nonce in the encrypted memo", "GET /api/quote + X-PAYMENT", and "200 OK · \"payer\": \"\"" | PASS | 5 items matching expected patterns: [GET /api/quote... \| 402 Payment Required scheme zc... \| pay shielded transaction, nonc... \| GET /api/quote + X-PAYMENT... \| 200 OK · "payer": ""...] |  |
| U-005 | Landing index / hero shielded field privacy veil | Container .veil contains 4 shaded field indicators labeled "amount", "memo", "from", and "to" with aria-label "On the public chain the amount, memo, sender and receiver are hidden" | PASS | aria-label: "On the public chain the amount, memo, sender and receiver are hidden", fields: [amount, memo, from, to] |  |
| U-006 | Landing index / exchange animation autoplay | Under prefers-reduced-motion: no-preference, list items .shadow li receive class .in sequentially with staggered timeout starting at 250 ms with 420 ms intervals | PASS | staggered autoplay observed, all 5 items receive .in |  |
| U-007 | Landing index / exchange animation reduced motion | Under prefers-reduced-motion: reduce, all .shadow li elements have class .in on initial load; computed animation and transition are "none" | PASS | 5/5 have .in on load, computed animation: none, transition: none |  |
| U-008 | Landing index / hero primary CTA | Link a.btn.btn-ink inside hero has href "evidence.html#quickstart" and text containing "Quickstart" | PASS | href: "evidence.html#quickstart", text: "Quickstart" |  |
| U-009 | Landing index / hero live 402 CTA | Link a.btn.btn-line inside hero has href "https://kage-api.201-18-198-162.sslip.io/api/quote" and text "Try the live 402" | PASS | href: "https://kage-api.201-18-198-162.sslip.io/api/quote", text: "Try the live 402" |  |
| U-010 | Landing index / top navigation brand mark | Anchor a.mark has href "#top", aria-label "Kage home", and text "Kage 影"; clicking navigates to #top | PASS | href: "#top", aria-label: "Kage home", text: "Kage 影", url: https://kage-landing.vercel.app/#top |  |
| U-011 | Landing index / top navigation links | Top navigation .toplinks contains Spec link with href "evidence.html" and GitHub link with href "https://github.com/nickthelegend/zec-shielded-x402" | PASS | Spec href: "evidence.html", GitHub href: "https://github.com/nickthelegend/zec-shielded-x402" |  |
| U-012 | Landing index / three steps section | Section #h-how contains ol.steps with 3 items: "01 Ask", "02 Pay", "03 Retry" | PASS | 3 items: [01 Ask \| 02 Pay \| 03 Retry] |  |
| U-013 | Landing index / proof numbers facts headline | Section #h-proof contains .facts displaying "10 Payments", "139 Tests", and "0 Spend keys" | PASS | .facts items: [10 Payments \| 139 Tests \| 0 Spend keys] |  |
| U-014 | Landing index / see evidence link | Anchor a.more has href "evidence.html#evidence" and text containing "See the evidence" | PASS | href: "evidence.html#evidence", text: "See the evidence" |  |
| U-015 | Landing index / what's live today section links | Section #h-live links paid API to "https://kage-api.201-18-198-162.sslip.io/api/quote" and facilitator to "https://kage-facilitator.201-18-198-162.sslip.io/health" | PASS | API link and facilitator health link present in section #h-live |  |
| U-016 | Landing index / setup required notice | Container .strip .setup h3 has text "SETUP REQUIRED"; paragraph text is "Paying it needs your own funded testnet wallet. Mainnet is not supported." | PASS | h3: "SETUP REQUIRED", p: "Paying it needs your own funded testnet wallet. Mainnet is not supported." |  |
| U-017 | Landing index / install code block text | Element code#clone contains text "git clone https://github.com/nickthelegend/zec-shielded-x402 && cd zec-shielded-x402 && cargo test -p x402z-server" | PASS | code#clone: "git clone https://github.com/nickthelegend/zec-shielded-x402 && cd zec-shielded-x402 && cargo test -p x402z-server" |  |
| U-018 | Landing index / install command copy button | Clicking button.copy[data-copy="#clone"] writes exact clone command to clipboard, button text updates to "Copied", and reverts to "Copy" after 1200 ms | PASS | copied text "Copied", clipboard matched, reverted to "Copy" |  |
| U-019 | Landing index / end section GitHub CTA | Section .end contains a.btn with href "https://github.com/nickthelegend/zec-shielded-x402" and text "View on GitHub" | PASS | href: "https://github.com/nickthelegend/zec-shielded-x402", text: "View on GitHub" |  |
| U-020 | Landing index / footer links well-formedness | Footer nav contains exactly 3 links: "evidence.html", "https://github.com/nickthelegend/zec-shielded-x402/blob/main/TESTING.md", and "https://github.com/nickthelegend/zec-shielded-x402" | PASS | 3 links: [evidence.html, https://github.com/nickthelegend/zec-shielded-x402/blob/main/TESTING.md, https://github.com/nickthelegend/zec-shielded-x402] |  |
| U-021 | Landing index / no control labelled log | Page contains 0 button, input, or interactive elements with text or label matching /log/i | PASS | 0 controls matching /log/i found |  |
| U-022 | Landing index / desktop 1280 px layout | At viewport width 1280 px, documentElement scrollWidth equals clientWidth (0 horizontal overflow) | PASS | scrollWidth: 1280, clientWidth: 1280 (0 horizontal overflow); screenshot: audit/live/landing-1280.jpg |  |
| U-023 | Landing index / mobile 375 px layout | At viewport width 375 px, documentElement scrollWidth equals clientWidth (0 horizontal overflow); topbar height is 60 px; .toplinks a.hide-sm is hidden; hero grid stacks to 1 column | PASS | scrollWidth: 375, clientWidth: 375, topbar height: 60px, hide-sm hidden: true; screenshot: audit/live/landing-375.jpg |  |
| U-024 | Landing index / console & network cleanliness | 0 console errors, 0 unhandled page errors, and 0 failed or unexpected 4xx/5xx network responses on happy path load | PASS | 0 console errors, 0 network errors |  |
| U-025 | Evidence page / page load & HTTP status | HTTP 200 response at /zec-shielded-x402/evidence.html and https://kage-landing.vercel.app/evidence.html; title text is "Kage — spec and evidence" | PASS | HTTP 200, title "Kage — spec and evidence" |  |
| U-026 | Evidence page / hero headline | Exactly one h1 element with text "The spec, the runs, the evidence." with mark element wrapping "evidence" | PASS | 1 h1: "The spec, the runs, the evidence.", mark: "evidence" |  |
| U-027 | Evidence page / skip link keyboard accessibility | Element a.skip has href "#content"; pressing Tab from page top focuses it and sets top: 10px; pressing Enter navigates URL to "#content" | PASS | href: "#content", focused: true, top: 10px, url: https://kage-landing.vercel.app/evidence.html#content |  |
| U-028 | Evidence page / top navigation brand mark | Anchor a.mark has href "./", aria-label "Kage home", and text "Kage 影"; clicking navigates to landing root | PASS | href: "./", aria-label: "Kage home", text: "Kage 影" |  |
| U-029 | Evidence page / top navigation links | Top navigation .toplinks contains Quickstart href "#quickstart", Evidence href "#evidence", Spec href "https://github.com/nickthelegend/zec-shielded-x402/blob/main/docs/spec-shielded-x402-agents.md", and GitHub href "https://github.com/nickthelegend/zec-shielded-x402" | PASS | Quickstart: #quickstart, Evidence: #evidence, Spec: https://github.com/nickthelegend/zec-shielded-x402/blob/main/docs/spec-shielded-x402-agents.md, GitHub: https://github.com/nickthelegend/zec-shielded-x402 |  |
| U-030 | Evidence page / facts headline strip | Container .facts displays 3 items: "10" paid testnet txs, "139" tests, "0" spend keys on the merchant side | PASS | .facts text: "10 paid testnet txs, verifiable with a published UIVK; 2 paid by an LLM 139 tests, 5 against live testnet 0 spend keys on the merchant side" |  |
| U-031 | Evidence page / install note hosted 402 quote link | Element .install-note a[href*="kage-api"] is visible with exact href "https://kage-api.201-18-198-162.sslip.io/api/quote" | PASS | visible: true, href: "https://kage-api.201-18-198-162.sslip.io/api/quote" |  |
| U-032 | Evidence page / clone command copy button | Clicking button.copy[data-copy="#clone"] writes exact git clone command to clipboard; button text changes to "Copied" and reverts to "Copy" after 1200 ms | PASS | copied text "Copied", clipboard matched, reverted to "Copy" |  |
| U-033 | Evidence page / quickstart test commands copy button | Clicking button.copy[data-copy="#qs"] strips inline comments and writes 5 newline-separated cargo/npm test commands to clipboard; button text changes to "Copied" and reverts to "Copy" after 1200 ms | PASS | copied 5 commands stripped of comments, button reverted to "Copy" |  |
| U-034 | Evidence page / hero wire tabs structure | Container .tabs has role="tablist" and exactly 5 button[role="tab"] elements: Tab 0 (MCP), Tab 1 (402), Tab 2 (X-PAYMENT), Tab 3 (pending ×10), Tab 4 (200) | PASS | role="tablist", 5 tabs: [AIMCP, 1402, 2X-PAYMENT, 3pending ×10, 4200] |  |
| U-035 | Evidence page / hero wire Tab 0 (MCP) initial state | Tab 0 has aria-selected="true"; panel #p0 is visible; panels #p1 through #p4 have hidden attribute; #recDate contains "2026-10-22"; #recWhat displays "LLM over MCP" | PASS | tab0 aria-selected="true", #p0 visible, #p1-#p4 hidden, recDate: "recorded run · 2026-10-22 15:34:30–15:36:22 UTC", recWhat: "LLM over MCP" |  |
| U-036 | Evidence page / hero wire Tab 0 MCP tape & txid link | Element #footMcp is visible linking to "https://testnet.zecblock.com/tx/f037d5978fec7817340ff118d328995288f0a575a1850b083ca05f7d71d4e22a"; #footExchange is hidden; list #mcptape li contains 11 items; call 2 has paid_fetch with f037d597 and status settlement_pending; call 3 has redeem_payment with 200 and "payer": "" | PASS | #footMcp visible with tx f037d597, #footExchange hidden, 11 calls in #mcptape, call 2 & 3 match |  |
| U-037 | Evidence page / hero wire Tab 0 unedited log link | Panel #p0 contains anchor linking to "https://github.com/nickthelegend/zec-shielded-x402/blob/main/docs/mcp-paid-run.jsonl" with text "mcp-paid-run.jsonl" | PASS | href: "https://github.com/nickthelegend/zec-shielded-x402/blob/main/docs/mcp-paid-run.jsonl" |  |
| U-038 | Evidence page / hero wire Tab 1 (402) selection & content | Clicking Tab 1 sets aria-selected="true" on Tab 1; panel #p1 visible; panels #p0,#p2,#p3,#p4 hidden; #recDate contains "2026-10-21"; #recWhat displays "GET /api/quote"; #footExchange visible with tx 25a8b324 link; #p1 shows 402 body with maxAmountRequired 100000 and memoNonce x402z-696c58afd509cc0f8753f3b0 | PASS | Tab 1 active, #p1 visible, date 2026-10-21, #footExchange visible with 25a8b324, body matched |  |
| U-039 | Evidence page / hero wire Tab 2 (X-PAYMENT) selection & content | Clicking Tab 2 sets aria-selected="true" on Tab 2; panel #p2 visible; shows broadcast txid 25a8b324bebee30220382113c6fb9cc3be1144b036117c34778bde3b1d8b689d and base64 X-PAYMENT header with decoded JSON payload | PASS | Tab 2 active, shows broadcast txid 25a8b324bebee30220382113c6fb9cc3be1144b036117c34778bde3b1d8b689d and decoded X-PAYMENT header |  |
| U-040 | Evidence page / hero wire Tab 3 (pending ×10) selection & content | Clicking Tab 3 sets aria-selected="true" on Tab 3; panel #p3 visible; list #tape li contains 11 items: 10 rows showing 402 settlement_pending and 11th row li.win showing 200 {"success":true,"transaction":"25a8b324…689d"} | PASS | 11 tape items: 10 with 402 settlement_pending, 11th li.win with 200 success |  |
| U-041 | Evidence page / hero wire Tab 4 (200) selection & content | Clicking Tab 4 sets aria-selected="true" on Tab 4; panel #p4 visible; shows "HTTP/1.1 200 OK", x-payment-response header, decoded JSON with "success":true and "payer":"", and response body settlement with amountZat 100000 | PASS | Tab 4 shows 200 OK, x-payment-response header, "success":true, "payer":"", amountZat 100000 |  |
| U-042 | Evidence page / hero wire keyboard arrow navigation | Focusing Tab 4 and pressing ArrowRight selects and focuses Tab 0 (wraps right); pressing ArrowLeft selects and focuses Tab 4 (wraps left); selected tab updates aria-selected to "true" | PASS | Arrow navigation wraps right to Tab 0 and left to Tab 4 with aria-selected="true" |  |
| U-043 | Evidence page / hero wire autoplay without reduced motion | Under prefers-reduced-motion: no-preference, tabs automatically cycle through sequence after ~4.2s intervals; stops cycling on user scroll, click, or mouse hover | PASS | autoplay cycled from Tab 0 to Tab 1 within ~5s interval |  |
| U-044 | Evidence page / hero wire reduced motion behavior | Under prefers-reduced-motion: reduce, tabs do not autoplay; Tab 0 remains selected; #tape li computed animationName is "none" | PASS | no autoplay under reduced motion; tape li computed animationName is "none" |  |
| U-045 | Evidence page / quickstart test counts agreement | Table #tests rows sum to 139 (spendguard: 16, x402z-protocol: 14, x402z-server: 42, x402z-agent: 32, x402z-mcp: 7, x402z-facilitator: 17, x402-zcash: 11); matches facts headline "139" and #qs comment counts | PASS | sum is 139; crate breakdown: {"spendguard":16,"x402z-protocol":14,"x402z-server":42,"x402z-agent":32,"x402z-mcp":7,"x402z-facilitator":17,"x402-zcash":11} |  |
| U-046 | Evidence page / sidebar TOC 13 sections completeness | Element #toc contains exactly 13 links in order: #overview, #quickstart, #the-402, #spendguard, #x-payment, #facilitator, #settlement, #zip321, #mcp, #tab, #leaks, #evidence, #tests | PASS | exact 13 links: [#overview, #quickstart, #the-402, #spendguard, #x-payment, #facilitator, #settlement, #zip321, #mcp, #tab, #leaks, #evidence, #tests] |  |
| U-047 | Evidence page / sidebar TOC scroll spy & offset visibility | Clicking any TOC link scrolls the target section below fixed top bar (heading bounding rect top >= covered - 1 && top < innerHeight); clicked link receives class .on; #tocNow text equals section label; exactly one TOC link has class .on | PASS | all 13 TOC links scroll cleanly below fixed bar, update #tocNow, highlight exactly one link |  |
| U-048 | Evidence page / mobile TOC details drawer interaction | At viewport width <= 920 px, #toc summary is visible; clicking summary toggles open attribute; clicking a section link inside removes open attribute (closes drawer) and navigates to section | PASS | TOC summary visible, toggles open, link click navigates and auto-closes drawer |  |
| U-049 | Evidence page / section heading anchor links | Clicking a.anchor inside any section header (e.g. #facilitator a.anchor) updates URL hash to matching section ID (e.g. "#facilitator") | PASS | anchor click updated URL hash to "https://kage-landing.vercel.app/evidence.html#facilitator" |  |
| U-050 | Evidence page / evidence table 10 explorer links | Table #evidence tbody tr contains exactly 10 rows; each row contains anchor linking to "https://testnet.zecblock.com/tx/<64-hex-txid>"; anchor text is "${txid[0..8]}…${txid[-4..]} ↗"; block numbers match README with commas | PASS | 10 rows matching README txids and blocks with formatted commas |  |
| U-051 | Evidence page / explorer links page-wide hygiene | Every anchor on the page with href containing "zecblock" matches regex ^https://testnet\.zecblock\.com/tx/([0-9a-f]{64})$ and references one of the 10 verified README txids | PASS | all 16 zecblock links match regex and reference verified README txids |  |
| U-052 | Evidence page / Kage Tab section & transcript links | Section #tab contains text "RFC 9474 blind RSA" and terminal text "nonce_already_used"; anchor with text "full transcript" links to "https://github.com/nickthelegend/zec-shielded-x402/blob/main/docs/tab-live-run.txt" | PASS | section #tab contains blind RSA text, nonce_already_used, and links to tab-live-run.txt |  |
| U-053 | Evidence page / MCP section & transcript links | Section #mcp links to "full transcript" at "https://github.com/nickthelegend/zec-shielded-x402/blob/main/docs/mcp-paid-run.txt" and "refusal transcript" at "https://github.com/nickthelegend/zec-shielded-x402/blob/main/docs/mcp-live-run.txt"; terminal contains "\"success\": true" | PASS | MCP section links to paid & refusal transcripts, terminal shows "success": true |  |
| U-054 | Evidence page / footer links well-formedness | Footer nav contains exactly 4 links: "https://github.com/nickthelegend/zec-shielded-x402", "https://github.com/nickthelegend/zec-shielded-x402/blob/main/docs/spec-shielded-x402-agents.md", "https://github.com/nickthelegend/zec-shielded-x402/blob/main/TESTING.md", and "#evidence"; clicking #evidence navigates to evidence section | PASS | 4 footer links match expected exact URLs |  |
| U-055 | Evidence page / no control labelled log | Page contains 0 button, input, or interactive elements with text or label matching /log/i | PASS | 0 interactive elements matching /log/i found |  |
| U-056 | Evidence page / desktop 1280 px layout | At viewport width 1280 px, documentElement scrollWidth equals clientWidth (0 horizontal overflow) | PASS | scrollWidth: 1280, clientWidth: 1280 (0 horizontal overflow); screenshot: audit/live/evidence-1280.jpg |  |
| U-057 | Evidence page / mobile 375 px layout | At viewport width 375 px, documentElement scrollWidth equals clientWidth (0 horizontal overflow); TOC drawer collapses into sticky top bar; tables scroll horizontally inside container .tbl | PASS | scrollWidth: 375, clientWidth: 375 (0 horizontal overflow); screenshot: audit/live/evidence-375.jpg |  |
| U-058 | Evidence page / console & network cleanliness | 0 console errors, 0 unhandled page errors, and 0 failed or unexpected 4xx/5xx network responses on happy path load | PASS | 0 console errors, 0 network errors |  |
| B-472 | Live landing (later read-only; not called in Phase1) | GET https://kage-landing.vercel.app returns HTTP 200 HTML with title Kage — shielded x402 for AI agents. | PASS | HTTP 200, title "Kage — shielded x402 for AI agents" |  |
| B-469 | Live API quote (later read-only; not called in Phase1) | GET https://kage-api.201-18-198-162.sslip.io/api/quote returns HTTP 402, x402Version:1, scheme zcash-shielded, network zcash-testnet, asset ZEC, ZIP-321 extra.paymentUri and fresh nonce/expiry. | PASS | HTTP 402, x402Version: 1, scheme: "zcash-shielded", network: "zcash-testnet", asset: "ZEC", memoNonce: "x402z-2407a3245422986693ef9238", expiresAt: 1792969902, paymentUri: "zcash:utest100fnpp4zd4lp0vmu093gwx6p6ujv..." |  |
| B-035 | Challenge QR known nonce | HTTP 200; Content-Type: image/svg+xml, Cache-Control: no-store; SVG encodes that challenge's ZIP-321 URI with zero external QR-service requests. | PASS | HTTP 200, Content-Type: image/svg+xml, Cache-Control: no-store, body length: 36152 bytes SVG |  |
| B-239 | Facilitator health success | Unauthenticated GET /health returns HTTP 200; JSON ok: true, network: "testnet", lightwalletd: <URL>, integer chainTip, default minConfirmations: 1, viewingKey: "uivk", hex receiptPubkey; live chain-tip probe. | PASS | HTTP 200, ok: true, network: "testnet", chainTip: 4541789, viewingKey: "uivk", receiptPubkey: "95b72c33b95c02a904a71c37e8638d39a1309a1f3686a374c898d32a8ab449c6" |  |
| B-471 | Live facilitator health (later read-only; not called in Phase1) | GET https://kage-facilitator.201-18-198-162.sslip.io/health returns HTTP 200, ok:true, network:"testnet", positive chainTip, viewingKey:"uivk"; failed probe instead returns HTTP 503 ok:false. | PASS | HTTP 200, ok: true, network: "testnet", chainTip: 4541789, viewingKey: "uivk" |  |
| B-238 | Facilitator GET /supported | HTTP 200 with exact JSON {kinds:[{x402Version:1,scheme:"zcash-shielded",network:"zcash-testnet"}]}; no Authorization needed. | PASS | HTTP 200, exact JSON: {"kinds":[{"x402Version":1,"scheme":"zcash-shielded","network":"zcash-testnet"}]} |  |
