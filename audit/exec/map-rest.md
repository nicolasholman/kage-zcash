156 rows: 2 COVERED, 151 NEEDS_PROBE, 2 NEEDS_SEND, 1 NEEDS_LIVE_GET

| ID | verdict | evidence |
| --- | --- | --- |
| B-251 | NEEDS_PROBE | Run x402z-facilitator with X402Z_MERCHANT_UIVK set to a full UFVK and probe GET /health on loopback expecting warning in stderr and viewingKey in health JSON. |
| B-252 | NEEDS_PROBE | Run x402z-facilitator without X402Z_FACILITATOR_TOKEN and assert exit code 2 and missing token error in stderr. |
| B-253 | NEEDS_PROBE | Run x402z-facilitator with X402Z_FACILITATOR_TOKEN under 32 characters and assert exit code 2 and token length error in stderr. |
| B-254 | NEEDS_PROBE | Run x402z-facilitator with missing or invalid X402Z_RECEIPT_SIGNING_KEY and assert exit code 2 and signing key error in stderr. |
| B-255 | NEEDS_PROBE | Run x402z-facilitator with unset ZCASH_LWD_URL and assert stderr contains default testnet.zec.rocks URL. |
| B-256 | NEEDS_PROBE | Run x402z-facilitator with ZCASH_LWD_URL set to an unreachable loopback port and assert exit code 1 and connection error in stderr. |
| B-257 | NEEDS_PROBE | Run x402z-facilitator against a mock lightwalletd serving mainnet and assert exit code 1 and testnet chain requirement in stderr. |
| B-258 | NEEDS_PROBE | Run x402z-facilitator with negative or non-integer X402Z_MIN_CONFIRMATIONS or X402Z_EXPIRY_GRACE_SECS and assert exit code 2 and stderr. |
| B-259 | NEEDS_PROBE | Run x402z-facilitator with X402Z_FACILITATOR_DB set to an uncreatable directory path and assert exit code 2 and database error in stderr. |
| B-260 | NEEDS_PROBE | Run x402z-agent without subcommands against a loopback server returning paid 200 and assert exit code 0 and stdout paid JSON. |
| B-261 | NEEDS_PROBE | Run x402z-agent unknown-cmd and assert exit code 2 and command list usage in stderr. |
| B-262 | NEEDS_PROBE | Run x402z-agent new-wallet in a temporary wallet directory that already contains an account database and assert exit code 2. |
| B-263 | NEEDS_PROBE | Run x402z-agent address in an empty wallet directory and assert exit code 4 and no wallet account message in stderr. |
| B-264 | NEEDS_PROBE | Run x402z-agent balance with a fixture wallet and mock lightwalletd and assert exit code 0 and formatted spendable/total balance output. |
| B-265 | NEEDS_PROBE | Run x402z-agent pay with X402Z_TARGET_URL unset and assert exit code 2 and missing target error in stderr. |
| B-266 | NEEDS_PROBE | Run x402z-agent address with X402Z_AGENT_WALLET_DIR unset and assert exit code 2 and missing wallet dir in stderr. |
| B-267 | NEEDS_PROBE | Run x402z-agent pay with X402Z_AGENT_LEDGER unset and assert exit code 2 and missing ledger message in stderr. |
| B-268 | NEEDS_PROBE | Run x402z-agent pay with X402Z_AGENT_LEDGER_KEY and seed unset and assert exit code 2 and missing ledger key in stderr. |
| B-269 | NEEDS_PROBE | Run x402z-agent pay with non-64-hex X402Z_AGENT_LEDGER_KEY and assert exit code 2 and hex format error in stderr. |
| B-270 | NEEDS_PROBE | Run x402z-agent pay with non-numeric X402Z_AGENT_DAILY_CAP_ZAT and assert exit code 2 and integer format error in stderr. |
| B-271 | NEEDS_PROBE | Run x402z-agent pay against a loopback 402 with price exceeding daily cap and assert exit code 3, stderr daily cap exceeded, and ledger denial. |
| B-272 | NEEDS_PROBE | Run x402z-agent pay against a loopback 402 with price at approval threshold in a non-interactive terminal and assert exit code 3. |
| B-273 | NEEDS_PROBE | Run x402z-agent pay against a target host not on allowlist and assert exit code 3, preflight stderr, and uncharged audit row. |
| B-274 | NEEDS_PROBE | Run x402z-agent pay targeting port 9999 when allowlist entry pins port 4020 and assert exit code 3 before making any connection. |
| B-275 | NEEDS_PROBE | Run x402z-agent pay targeting an HTTP URL when allowlist pins HTTPS and assert exit code 3 and allowlist refusal in stderr. |
| B-276 | NEEDS_PROBE | Run x402z-agent pay targeting a non-loopback HTTP URL without X402Z_AGENT_ALLOW_HTTP=1 and assert exit code 3 and plain HTTP refusal in stderr. |
| B-277 | NEEDS_PROBE | Run x402z-agent pay with a non-HTTP URL scheme and assert exit code 2 and invalid URL in stderr. |
| B-278 | NEEDS_PROBE | Run x402z-agent tab-redeem or tab-fetch against a disallowed host and assert preflight refusal before sending any network request. |
| B-279 | NEEDS_PROBE | Run x402z-agent pay against an allowlisted loopback endpoint returning HTTP 302 and assert exit code 1 and redirect refusal in stderr. |
| B-280 | NEEDS_PROBE | Run x402z-agent tab-buy against an allowlisted loopback endpoint returning HTTP 302 and assert exit code 1 before spendguard. |
| B-281 | NEEDS_PROBE | Run x402z-agent pay against a loopback server returning Content-Length over 1048576 bytes and assert exit code 1 and limit error in stderr. |
| B-282 | NEEDS_PROBE | Run x402z-agent pay against a loopback server streaming chunked bytes exceeding 1048576 bytes and assert exit code 1 and stderr limit message. |
| B-283 | NEEDS_PROBE | Run x402z-agent pay against a loopback endpoint returning initial HTTP 200 and assert exit code 0 with unchanged stdout body and no payment headers. |
| B-284 | NEEDS_PROBE | Run x402z-agent pay against a loopback endpoint returning HTTP 500 and assert exit code 1 and expected 402 in stderr. |
| B-285 | NEEDS_PROBE | Run x402z-agent pay against a loopback endpoint returning malformed JSON on HTTP 402 and assert exit code 1 and stderr error. |
| B-286 | NEEDS_PROBE | Run x402z-agent pay against a loopback 402 offering only EVM exact scheme and assert exit code 1 and no zcash option in stderr. |
| B-287 | NEEDS_PROBE | Run x402z-agent pay against a loopback 402 with an expired challenge timestamp and assert exit code 1 and expired challenge error in stderr. |
| B-288 | NEEDS_PROBE | Run x402z-agent pay against a loopback 402 containing invalid requirement fields and assert exit code 1 and invalid challenge error in stderr. |
| B-289 | NEEDS_PROBE | Run x402z-agent pay against a valid loopback 402 with X402Z_AGENT_SEED unset and assert exit code 4 and unconfigured seed message in stderr. |
| B-290 | NEEDS_PROBE | Run x402z-agent pay with a fixture wallet holding 0 spendable balance and assert exit code 4 and insufficient balance in stderr. |
| B-291 | NEEDS_PROBE | Run x402z-agent pay against a loopback 402 with wallet sync failing during preparation and assert exit code 1 and audit row reason starting with allowed, but not paid:. |
| B-292 | NEEDS_PROBE | Run x402z-agent tab-redeem or pay flow against a loopback server returning 402 invalid_payment and assert exit code 5 and server refusal in stderr. |
| B-293 | COVERED:nonce_already_used_for_the_agents_own_payment_keeps_it_settled_and_says_do_not_pay_again:crates/x402z-agent/tests/lost_answer.rs | assert!(matches!(&e, PayError::AlreadyRedeemed(m) if m.contains("Do NOT pay again"))); assert_eq!((e.exit_code(), e.kind()), (6, "already_redeemed")); assert_eq!(entry.settled, Some(true)); assert_eq!(l.guard(...).spent_today(...), 100_000); matches HTTP 402 nonce_already_used exit 6, stderr Do NOT pay again for it., ledger settled: true, and charged once. |
| B-294 | COVERED:nonce_already_used_for_the_agents_own_payment_keeps_it_settled_and_says_do_not_pay_again:crates/x402z-agent/tests/lost_answer.rs | assert_eq!((e.exit_code(), e.kind()), (6, "already_redeemed")); assert_eq!(entry.settled, Some(true)); assert_eq!(l.guard(...).spent_today(...), 100_000); under (Some(true), "after a 200") matches subsequent already-redeemed refusal exit 6, preserves settled: true, and zero duplicate charges. |
| B-295 | NEEDS_PROBE | Run x402z-agent pay against a loopback server closing connection immediately after HTTP 200 headers and assert exit code 1 and settled state in ledger. |
| B-296 | NEEDS_PROBE | Run x402z-agent pay against a loopback server returning 402 settlement_pending until timeout and assert exit code 1 without refunding or refusal in ledger. |
| B-297 | NEEDS_PROBE | Run x402z-agent pay with short confirmation timeout against a pending loopback server and assert exit code 1 and resume instruction in stderr. |
| B-298 | NEEDS_PROBE | Seed ledger with an unresolved payment and run x402z-agent pay asserting it resumes that URL without starting a new challenge. |
| B-300 | NEEDS_PROBE | Spawn two concurrent x402z-agent pay processes competing for a single remaining cap slot under ledger lock and assert one succeeds while the second exits 3. |
| B-302 | NEEDS_PROBE | Run x402z-agent pay with a mock wallet broadcast returning timeout and assert exit code 1, stderr unknown outcome, and charged pending entry. |
| B-304 | NEEDS_PROBE | Run x402z-agent pay with a mock wallet broadcast returning rejection and assert exit code 1, stderr message, and uncharged audit row. |
| B-306 | NEEDS_PROBE | In-process test calling LedgerFile::load on a non-existent path and asserting agents: [], audit: [], and 0 spent today. |
| B-308 | NEEDS_PROBE | In-process test checking SpendGuard under a modified policy on a pre-populated ledger asserting past spends remain counted. |
| B-309 | NEEDS_PROBE | Run x402z-agent pointing X402Z_AGENT_LEDGER to a file sealed with non-ledger JSON and assert exit code 2 and invalid ledger message in stderr. |
| B-310 | NEEDS_PROBE | Run x402z-agent pointing to a ledger sealed under a different key and assert exit code 2 and wrong key message in stderr. |
| B-311 | NEEDS_PROBE | Run x402z-agent pointing to an unsealed plaintext ledger JSON and assert exit code 2 and seal-ledger guidance in stderr. |
| B-313 | NEEDS_PROBE | Run x402z-agent seal-ledger against isolated plaintext ledger and tab fixtures and assert exit code 0, sealed in stderr, and valid encryption headers. |
| B-314 | NEEDS_PROBE | Run x402z-agent seal-ledger on already sealed files and assert exit code 0 and already sealed or absent in stderr. |
| B-315 | NEEDS_PROBE | Run x402z-agent seal-ledger on non-JSON non-sealed bytes and assert exit code 1 and stderr error. |
| B-316 | NEEDS_PROBE | Run x402z-agent tab-buy 3 --external against a loopback 402 with tab offer and assert exit code 0 and stdout JSON fields without broadcasting. |
| B-317 | NEEDS_PROBE | In-process test of tab::buy with a mock wallet against a loopback tab issuer asserting one transaction build and token storage. |
| B-318 | NEEDS_PROBE | Run x402z-agent tab-buy with non-integer token count and assert exit code 2 and usage in stderr. |
| B-319 | NEEDS_PROBE | Run x402z-agent tab-buy 25 against a loopback server offering max 20 tokens and assert exit code 2 and limit error in stderr. |
| B-320 | NEEDS_PROBE | Run x402z-agent tab-buy against a loopback tab offer with unit price causing arithmetic overflow and assert exit code 2 and stderr. |
| B-321 | NEEDS_PROBE | Run x402z-agent tab-buy 1 against a loopback 402 lacking extra.tab and assert exit code 1 and no tab offer error in stderr. |
| B-322 | NEEDS_PROBE | Run x402z-agent tab-buy when a previously pinned key for the origin is active and merchant serves a different key, asserting exit code 1 and stderr message. |
| B-323 | NEEDS_PROBE | Run x402z-agent tab-buy against an offer whose epoch and grace expire before confirmation timeout and assert exit code 1 and stderr message. |
| B-324 | NEEDS_PROBE | Run x402z-agent tab-buy against an offer with issuePath pointing to a different origin and assert exit code 1 and origin error in stderr. |
| B-325 | NEEDS_PROBE | Run tab::buy_or_resume in-process or via CLI concurrently while a pending purchase exists and assert collection of existing tab without second spend. |
| B-326 | NEEDS_PROBE | Run x402z-agent tab-redeem <txid> against a loopback mock issuer returning 200 unblinded signatures and assert exit code 0 and sealed token file storage. |
| B-327 | NEEDS_PROBE | Run x402z-agent tab-redeem without arguments and assert exit code 2 and usage in stderr. |
| B-328 | NEEDS_PROBE | Run x402z-agent tab-redeem bad-txid and assert exit code 2 and 64-hex error in stderr. |
| B-329 | NEEDS_PROBE | Run x402z-agent tab-redeem <unrecorded-txid> and assert exit code 1 and no pending tab found in stderr. |
| B-330 | NEEDS_PROBE | In-process test of tab::finish_external against a mock issuer returning 409 key rotation asserting re-blinding and collection without second payment. |
| B-331 | NEEDS_PROBE | In-process test of tab::finish_external against a mock issuer returning 409 with an updated issuePath asserting retry against the new path. |
| B-332 | NEEDS_PROBE | In-process test of tab::finish_external against a mock issuer returning wrong keyId or mismatched signature count asserting exit code 1. |
| B-333 | NEEDS_PROBE | In-process test of tab::finish_external against a mock issuer returning invalid RSA signatures asserting exit code 1 and verification failure in stderr. |
| B-334 | NEEDS_PROBE | Run x402z-agent tab-redeem with 0-second confirmation timeout against a mock issuer returning 202 pending and assert exit code 1 and pending preserved. |
| B-335 | NEEDS_PROBE | Run x402z-agent tab-fetch with a seeded token file against a loopback server accepting tab tokens and assert exit code 0, content in stdout, and remaining count in stderr. |
| B-336 | NEEDS_PROBE | Run x402z-agent tab-fetch when no tokens exist for the target origin and assert exit code 4 and guidance in stderr without network calls. |
| B-339 | NEEDS_PROBE | Run x402z-agent tab-fetch against a mock server returning 402 token_already_spent and assert exit code 1 without retry or broadcast. |
| B-340 | NEEDS_PROBE | Run x402z-agent tab-fetch against a closed loopback port and assert exit code 1 and token removal. |
| B-341 | NEEDS_PROBE | Run x402z-agent tab-fetch against a loopback server closing connection mid-body and assert exit code 1 and token consumed without replay. |
| B-342 | NEEDS_PROBE | Run x402z-agent tab-status with an empty tab file and assert exit code 0 and empty tokens/pending JSON structure. |
| B-343 | NEEDS_PROBE | Run x402z-agent tab-status with a seeded tab file containing active and expired tokens and assert exit code 0 and JSON listing. |
| B-345 | NEEDS_PROBE | Run x402z-agent tab-status pointing to a tampered sealed tab file and assert exit code 2 and wrong key error in stderr. |
| B-346 | NEEDS_PROBE | Send initialize with version 2025-06-18 to x402z-mcp over stdio and assert result.protocolVersion: 2025-06-18 and capabilities.tools.listChanged: false. |
| B-347 | NEEDS_PROBE | Send initialize with version 2025-03-26 to x402z-mcp over stdio and assert result.protocolVersion: 2025-03-26. |
| B-348 | NEEDS_PROBE | Send initialize with version 2024-11-05 to x402z-mcp over stdio and assert result.protocolVersion: 2024-11-05 and capabilities.tools.listChanged: false. |
| B-349 | NEEDS_PROBE | Send initialize with unsupported version 1999-01-01 to x402z-mcp over stdio and assert fallback to result.protocolVersion: 2025-06-18. |
| B-353 | NEEDS_PROBE | Send an unknown JSON-RPC method request to x402z-mcp over stdio and assert error.code: -32601 and message. |
| B-354 | NEEDS_PROBE | Call tools/call with an unknown tool name to x402z-mcp over stdio and assert error.code: -32602 and message. |
| B-355 | NEEDS_PROBE | Send malformed JSON to x402z-mcp over stdio and assert error.code: -32700 parse error followed by successful ping. |
| B-356 | NEEDS_PROBE | Send an empty blank line to x402z-mcp over stdio and assert zero response emitted before the next valid request. |
| B-358 | NEEDS_PROBE | Call each of the five tools on x402z-mcp over stdio without X402Z_AGENT_LEDGER_KEY and assert isError: true and setup_required on all five. |
| B-361 | NEEDS_PROBE | Call MCP redeem_payment omitting url, txid, or memoNonce over stdio and assert isError: true and invalid_arguments. |
| B-362 | NEEDS_PROBE | Call MCP redeem_payment with non-64-hex txid over stdio and assert isError: true, invalid_arguments, and txid format message. |
| B-363 | NEEDS_PROBE | Call MCP tab_fetch with invalid buy values (0, -1, 101) over stdio and assert isError: true, invalid_arguments, and exact message. |
| B-364 | NEEDS_PROBE | Start x402z-mcp with invalid X402Z_MCP_DEFAULT_TAB_SIZE and call tab_fetch over stdio asserting isError: true and setup_required. |
| B-370 | NEEDS_PROBE | Call MCP paid_fetch over stdio against a loopback server returning 402 nonce_already_used and assert isError: true and already_redeemed. |
| B-371 | NEEDS_PROBE | Call MCP paid_fetch over stdio against a loopback server returning HTTP 200 and assert isError: false, status: 200, and paid: false. |
| B-373 | NEEDS_PROBE | Inspect MCP tool call responses across all tools over stdio asserting JSON-RPC envelope and content array structure. |
| B-374 | NEEDS_PROBE | Call MCP spend_status over stdio with fresh config and assert isError: false and all required status JSON fields. |
| B-375 | NEEDS_PROBE | Call MCP ledger_tail over stdio on an empty ledger and assert isError: false, entries: [], and count: 0. |
| B-376 | NEEDS_PROBE | Call MCP ledger_tail over stdio with n: 0 and n: 1000 asserting clamped bounds and newest-first entry order. |
| B-380 | NEEDS_PROBE | Call MCP tab_fetch over stdio against a loopback server returning 202 pending and assert isError: false and status: tab_pending. |
| B-381 | NEEDS_PROBE | Call MCP tab_fetch over stdio with a seeded token file against a loopback server accepting tab tokens asserting status: 200 and updated tokens. |
| B-382 | NEEDS_PROBE | In-process test calling parsePaymentRequired on a body with mixed EVM and Zcash options and asserting returned challenge fields. |
| B-383 | NEEDS_PROBE | In-process test calling parsePaymentRequired with version 2 or non-PaymentRequired shape asserting X402ZcashError and message. |
| B-384 | NEEDS_PROBE | In-process test calling parsePaymentRequired on an accepts list without Zcash shielded option asserting error message. |
| B-385 | NEEDS_PROBE | In-process test calling parsePaymentRequired with absent, whitespace, or oversized memo nonce asserting error message. |
| B-386 | NEEDS_PROBE | In-process test calling parsePaymentRequired with missing or non-numeric expiresAt asserting error message. |
| B-387 | NEEDS_PROBE | In-process test calling parsePaymentRequired with non-digit or zero amount asserting zatoshi integer error message. |
| B-388 | NEEDS_PROBE | In-process test calling parsePaymentRequired with a non-testnet UA address asserting payTo is not a testnet unified address. |
| B-389 | NEEDS_PROBE | In-process test calling parsePaymentRequired with nowUnix >= expiresAt asserting challenge expired at error message. |
| B-395 | NEEDS_PROBE | In-process test calling fetchWithPayment with non-GET HTTP methods asserting error before making any request. |
| B-396 | NEEDS_PROBE | In-process test calling fetchWithPayment against a loopback endpoint returning a 3xx redirect asserting redirect refusal error. |
| B-397 | NEEDS_PROBE | In-process test constructing paywall without facilitatorToken asserting setup required error message. |
| B-398 | NEEDS_PROBE | In-process test constructing paywall with non-64-hex receiptPubkeyHex asserting setup required error message. |
| B-399 | NEEDS_PROBE | In-process test constructing paywall with invalid payTo asserting testnet unified address error message. |
| B-400 | NEEDS_PROBE | Send unpaid GET to paywall middleware over loopback and assert HTTP 402 and full challenge JSON structure. |
| B-401 | NEEDS_PROBE | Send GET with invalid base64 in X-PAYMENT to paywall over loopback and assert HTTP 402 and invalid_payload errorReason. |
| B-402 | NEEDS_PROBE | Present X-PAYMENT with version 2, EVM scheme, or wrong network to paywall over loopback and assert HTTP 402 and errorReason. |
| B-403 | NEEDS_PROBE | Present X-PAYMENT with non-64-hex txid or bad nonce to paywall over loopback and assert HTTP 400. |
| B-404 | NEEDS_PROBE | Present X-PAYMENT with an unissued memo nonce to paywall over loopback and assert HTTP 402 unknown_challenge. |
| B-405 | NEEDS_PROBE | In-process unit test asserting empty memoryStore has size 0, undefined get, and false markSpent. |
| B-407 | NEEDS_PROBE | Send three requests to paywall configured with maxEntries: 2 over loopback and assert third returns HTTP 503 and store full error. |
| B-408 | NEEDS_PROBE | Recreate memoryStore in-process and present previously issued nonce asserting HTTP 402 unknown_challenge. |
| B-410 | NEEDS_PROBE | Send burst requests to paywall configured with ratePerMin: 0 over loopback and assert zero HTTP 429 responses. |
| B-411 | NEEDS_PROBE | Present payment to paywall against an unresponsive facilitator loopback socket and assert HTTP 502 and facilitator_error header. |
| B-412 | NEEDS_PROBE | Present payment to paywall configured with wrong facilitator token against a loopback facilitator and assert HTTP 502. |
| B-413 | NEEDS_PROBE | Present payment to paywall against a mock facilitator returning malformed JSON and assert HTTP 502. |
| B-414 | NEEDS_PROBE | Present payment to paywall against a mock facilitator returning pending status and assert HTTP 402 settlement_pending. |
| B-415 | NEEDS_PROBE | Present payment to paywall against a mock facilitator returning rejection and assert HTTP 402 with fresh challenge. |
| B-416 | NEEDS_PROBE | Present payment to paywall when mock facilitator returns 200 without a receipt and assert HTTP 502 facilitator_error. |
| B-417 | NEEDS_PROBE | Present payment to paywall when mock facilitator returns a receipt signed with a foreign key and assert HTTP 402 signature invalid. |
| B-418 | NEEDS_PROBE | Present payment to paywall when mock facilitator receipt claims mismatch presented payment and assert HTTP 402. |
| B-419 | NEEDS_PROBE | Present payment to paywall when mock facilitator reports 0 confirmations and assert HTTP 402 settlement_pending. |
| B-420 | NEEDS_PROBE | Present payment to paywall when mock facilitator reports payment mined after challenge expiry and assert HTTP 402 payment_expired. |
| B-421 | NEEDS_PROBE | Present valid payment fixture to paywall against a mock facilitator over loopback and assert HTTP 200 and single nonce markSpent. |
| B-422 | NEEDS_PROBE | Resend identical valid payment to paywall within 600s resend window over loopback and assert HTTP 200 and unchanged spent record. |
| B-423 | NEEDS_PROBE | Resend payment to paywall after 600s resend window has expired over loopback and assert HTTP 402 nonce_already_used. |
| B-424 | NEEDS_PROBE | Present different txid for an already spent nonce to paywall over loopback and assert HTTP 402 nonce_already_used. |
| B-425 | NEEDS_PROBE | Present payment to paywall with a store whose markSpent returns false and assert HTTP 402 nonce_already_used. |
| B-426 | NEEDS_PROBE | Run packages/x402-zcash/test/live.test.ts after the binaries exist. |
| B-431 | NEEDS_PROBE | In-process test validating challenges with non-utest1 addresses or addresses shorter than 20 characters expecting Err(InvalidChallenge::BadAddress). |
| B-433 | NEEDS_PROBE | In-process test validating challenges with empty, >64-byte, or non-alphanumeric nonces expecting Err(InvalidChallenge::BadNonce). |
| B-440 | NEEDS_PROBE | In-process test passing version 2, EVM exact scheme, mainnet, and missing nonce payloads to protocol payload validation. |
| B-445 | NEEDS_PROBE | In-process test parsing ZIP-321 URIs with librustzcash and asserting identical re-rendered URIs across 1, 100000, and 123456789 zat amounts. |
| B-464 | NEEDS_PROBE | In-process unit test calling parse_key_hex with non-64-hex strings asserting the exact error string. |
| B-465 | NEEDS_PROBE | In-process unit test asserting MemBlockCache::get_tip_height(None) returns Ok(None) and empty scan range returns Ok([]). |
| B-466 | NEEDS_PROBE | In-process unit test testing block insertion, range queries, deletion, and tip height tracking on MemBlockCache. |
| B-467 | NEEDS_PROBE | Send request to facilitator /settle with a database configured to fail writes and assert HTTP 503 and facilitator_error. |
| B-470 | NEEDS_LIVE_GET | Read-only GET of /, /api/events, /api/status on https://kage-api.201-18-198-162.sslip.io expecting 404. |
| U-083 | NEEDS_SEND:needs a funded Zcash testnet send | Requires browser submission to POST /api/redeem of a newly funded testnet transaction matching the freshly issued challenge memo. |
| U-084 | NEEDS_SEND:needs a funded Zcash testnet send | Requires an unconfirmed broadcast on testnet matching the freshly issued dashboard challenge to observe pending state in UI. |
