"""Build the local disposition table, preserving AUDIT.md expectations verbatim."""
from collections import Counter
import json
from pathlib import Path
import re

root=Path(__file__).resolve().parents[2]
out=root/'audit/local'
coverage={}
excerpts=[]
def put(ident,status,evidence,fix='—',excerpt=None):
    coverage[ident]=(status,evidence,fix)
    if excerpt: excerpts.append(f'### {ident}\n\n{evidence}\n\n```text\n{excerpt}\n```\n')

def tested(ids,log,name,note=''):
    lines=(out/log).read_text().splitlines()
    matches=[s for s in lines if name in s and ('... ok' in s or '✓' in s or s.startswith('✔'))]
    assert matches,(log,name)
    for number in ids:
        ident=f'B-{number:03}'
        put(ident,'PASS',f'`{name}` — [log](local/{log}); {note}'.rstrip('; '),excerpt='\n'.join(matches))

def ui(ids,name):
    lines=[s for s in (out/'playwright.log').read_text().splitlines() if name in s and '✓' in s]
    assert len(lines)>=2,(ids,name)
    for number in ids:
        put(f'U-{number:03}','PASS',f'`{name}` — [desktop/mobile log](local/playwright.log)',excerpt='\n'.join(lines))

ui([31],'the install note links the hosted 402 over https')
ui([32],'copy buttons put the exact command on the clipboard')
ui([35,36,37],'hero leads with the LLM-over-MCP run')
ui([40,42],'hero tabs MCP / 402 / X-PAYMENT / pending / 200')
ui([44],'the recorded exchange autoplays once without reduced motion')
ui([45],'quickstart test counts agree with the Tests table')
ui([47,48],'sidebar spec nav: every link lands its heading')
ui([49],'heading anchors and the skip link work')
ui([50,51],'evidence table: ten explorer links')
ui([52,53],'Kage Tab section and its transcript links')
ui([59],'loads: facilitator reachable, price, merchant UA and receipt key')
ui([60],'the public port serves the paid API only')
ui([72,74,75,77],'Send request → live 402 card')
ui([78],'402 card copy buttons copy payTo')
ui([82],'paste an already-mined txid against a fresh challenge')
ui([85,86],"another client's payment lands in the feed")
ui([87],"a payment for the card's own challenge ticks 200")
ui([88],'sidebar nav, including "Payment feed"')
ui([94],'one-machine demo: the spendguard panel shows the real agent ledger')
ui([97],'reduced motion: new feed rows do not animate')
ui([98],'DNS rebinding: under another host name')

# The browser supplements log assertions by AUDIT row ID, plus unfiltered console/network events.
for stem in ['probe-initial','probe-main','gaps']:
    records=json.loads((out/f'{stem}-results.json').read_text())
    for ident in dict.fromkeys(r['id'] for r in records):
        rows=[r for r in records if r['id']==ident]
        status='FAIL' if any(r['status']=='FAIL' for r in rows) else 'PASS'
        evidence=f'`{ident}` at 1280/375 px — [assertions](local/{stem}.log), [events](local/{stem}-events.json)'
        screenshots=[r['detail'] for r in rows if isinstance(r.get('detail'),str) and r['detail'].endswith('.jpg')]
        if screenshots:evidence+='; '+', '.join(f'[{Path(p).name}](local/{Path(p).name})' for p in screenshots)
        fix='Not applied: source edits prohibited.' if status=='FAIL' else '—'
        put(ident,status,evidence,fix,'\n'.join(json.dumps(r,ensure_ascii=False) for r in rows))

# Six flows expose Chrome's automatic diagnostics for intentionally non-2xx/failed requests.
for number in [71,81,93,95,96,101]:
    ident=f'U-{number:03}'
    old=coverage[ident]
    coverage[ident]=(old[0],old[1],"Not applied. Required/error-path HTTP behavior is correct; Chrome console diagnostics fail the user's strict console rule.")
for ident,detail in {
    'U-011':'Expected link label Spec; actual label Spec & evidence (href is correct).',
    'U-062':'Four section elements; #keys exists as a div, rather than the fifth expected section.',
    'U-067':'#paidTotal innerText is 0TAZ, rather than exact 0 TAZ; visual spacing is present.',
}.items():
    old=coverage[ident]
    coverage[ident]=(old[0],old[1],detail+' Not changed (audit only).')
for number in [83,84]:
    put(f'U-{number:03}','UNTESTED','No fresh matching paid/unconfirmed chain transaction was created.','No send permitted. Server fixture tests cover API settlement/pending, but do not exercise these browser states.')

for row in json.loads((out/'api-results.json').read_text()):
    put(row['id'],row['status'],f"`{row['id']}` — [Playwright HTTP assertions](local/api-probe.log), [request/status log](local/api-events.json)",excerpt=json.dumps(row,ensure_ascii=False))
# Do not infer a decoded QR from a successful SVG load.
put('B-035','UNTESTED','[SVG/header checks passed](local/api-probe.log); suite loads the local QR.','Partial: QR payload was not decoded by an independent reader.')

core='protocol-spendguard-server.log'
tested([32],core,'public_base_url_must_be_an_http_origin')
tested([41,46],core,'rejected_forged_unknown_and_malformed_x_payments')
tested([51],core,'facilitator_down_is_a_502_and_burns_nothing')
tested([53],core,'a_facilitator_that_refuses_the_token_is_setup_required_and_burns_nothing')
tested([60],core,'a_lost_200_is_answered_again_for_the_same_payment_without_a_second_payment')
tested([61,62],core,'x_payment_settles_once_and_returns_x_payment_response')
tested([63],core,'with_the_resend_window_off_a_replayed_x_payment_is_refused')
tested([90],core,'x_payment_attempts_that_stay_pending_share_the_client_budget')
tested([93],core,'limiter_zero_means_disabled')
tested([97],core,'restart_between_challenge_and_redeem_keeps_the_challenge')
tested([100],core,'expired_challenges_are_pruned_at_most_once_a_minute')
tested([101],core,'pruning_and_the_open_count_use_the_open_challenges_index')
tested([106],core,'pending_and_rate_limited_feed_rows_carry_no_full_nonce_and_no_client_address')
tested([107],core,'feed_rows_are_clipped')
tested([151],core,'one_payment_buys_n_unlinkable_tokens_each_spendable_once')
tested([152,153,154,155],core,'forged_and_tampered_tokens_are_refused')
tested([159],core,'tab_key_and_spent_tokens_survive_a_restart')

# Each passing Rust row below is a direct unit assertion, not a live-chain payment.
tested([427],core,'round_trips_with_camel_case_keys')
tested([428],core,'unknown_fields_rejected')
tested([429,430,432],core,'rejects_expired_mainnet_wrong_scheme_bad_nonce')
tested([434,435],core,'shape_validation_ignores_expiry_and_paid_in_time_uses_grace')
tested([436],core,'receipt_claims_mined_at_is_optional_for_old_receipts')
tested([437,438],core,'challenge_to_requirements_and_back')
tested([439],core,'x_payment_header_is_base64_json_and_round_trips')
tested([441],core,'settlement_response_matches_spec_fields')
tested([442],core,'tab_payload_round_trips_and_is_scheme_checked')
tested([443],core,'amounts_are_decimal_zec_without_trailing_zeros')
tested([444],core,'memo_is_base64url_without_padding')
tested([446],core,'unknown_agent_and_zero_amount_denied')
tested([447],core,'host_normalisation_port_case_trailing_dot')
tested([448],core,'wildcard_matches_subdomains_only')
tested([449],core,'an_entry_with_a_port_allows_only_that_port')
tested([450],core,'an_entry_with_a_scheme_allows_only_that_scheme')
tested([451],core,'unknown_agent_and_zero_amount_denied')
tested([452],core,'cap_is_inclusive_and_cumulative')
tested([453],core,'record_spend_refuses_over_cap_and_does_not_mutate')
tested([454],core,'cap_resets_at_utc_midnight')
tested([455],core,'approval_threshold_inclusive')
tested([456],core,'deny_beats_approval')
tested([457],core,'overflow_is_denied')
tested([458],core,'round_trip_and_nothing_readable_on_disk')
tested([459,460,461,462,463],core,'wrong_key_tampering_truncation_and_purpose_swaps_are_refused')

agent='agent.log'
tested([299],agent,'a_paid_fetch_that_races_another_call_for_the_same_url')
tested([301],agent,'a_crash_between_broadcast_and_its_record_leaves_a_charged_resumable_entry')
tested([303,305],agent,'a_lost_answer_for_a_tx_lightwalletd_has_is_paid_and_a_rejection_is_not_charged')
tested([307],agent,'cap_survives_restarts_via_paid_audit_entries')
tested([312],agent,'a_settled_payment_is_never_downgraded_by_a_later_refusal')
tested([337,338],agent,'token_file_takes_per_origin_drops_expired_and_survives_reopen')
tested([344],agent,'tokens_are_sealed_at_rest_and_refused_under_a_wrong_key_or_a_changed_byte')

mcp='mcp.log'
tested([350,351,352],mcp,'handshake_and_tool_list')
tested([357,360],mcp,'missing_config_is_setup_required_not_a_crash')
tested([359],mcp,'without_a_ledger_key_the_tools_say_setup_required_and_the_ledger_stays_sealed')
tested([365,366,367,377],mcp,'spendguard_refusals_are_tool_errors_and_land_in_the_ledger')
tested([368,369],mcp,'a_retried_paid_fetch_resumes_the_broadcast_payment_instead_of_paying_twice')
tested([372],mcp,'a_slow_call_does_not_block_the_next_request')
tested([378,379],mcp,'tab_fetch_without_tokens_buys_a_tab_inside_spendguard')

ts='typescript-unit.log'
tested([390],ts,'ZIP 321: decimal ZEC without trailing zeros')
tested([391,392,393],ts,'X-PAYMENT is base64 JSON with the payload x402z-server reads')
tested([394],ts,'receipts verify only under the facilitator key and only unchanged')
tested([406],ts,'the memory store forgets challenges whose redeem window is over')
tested([409],ts,'402s and X-PAYMENT attempts share a per-client budget')

tested(list(range(474,484)),'live-testnet.log','every_kage_payment_verifies_with_only_the_published_uivk','Ten-entry PAYMENTS loop completed; heights/memos/amounts/pool/confirmations checked. No send path.')
tested([468],'live-testnet.log','every_kage_payment_verifies_with_only_the_published_uivk','client() asserts test chain, reads chain tip, and fetches transaction/block data only.')
put('B-473','PASS','[Boundary and method notes](local/method.md), [recorded-chain log](local/live-testnet.log)','Chain checks issued no send. The separately authorized stack generated its throwaway receipt key; no manual keygen/new-wallet or protected wallet/project-env reads.')

for number in [469,470,471,472]:
    put(f'B-{number:03}','UNTESTED','Hosted endpoint checks are outside this LOCAL assignment.','Run the separate live read-only audit.')
put('B-426','UNTESTED','The explicitly prohibited TypeScript live test was not run.','Rust recorded-chain tests passed; TypeScript live middleware settlement interoperability remains untested.')

rows=[]
for line in (root/'AUDIT.md').read_text().splitlines():
    match=re.match(r'^\| ([BU]-\d{3}) \| (.*?) \| (.*?) \| PENDING \|',line)
    if match:rows.append(match.groups())
assert len(rows)==584,len(rows)
counts=Counter(coverage.get(i,('UNTESTED',))[0] for i,_,_ in rows)
ui_counts=Counter(coverage.get(i,('UNTESTED',))[0] for i,_,_ in rows if i.startswith('U-'))

header=f'''Local audit completed on 2026-10-26 against commit `4e51b2b84c831bf297031b63416e9962c803c12c`.

Requested commands: build PASS; Playwright **63 passed, 0 failed, 1 skipped**; recorded-chain **3 passed** (all ten payments verified); MCP **7 passed**; agent **32 passed**; spendguard **16 passed**; protocol **14 passed**; server **42 passed**; TypeScript unit **9 passed**. Every package command exited 0. The desktop skip is the mobile-only layout test; its mobile run passed.

Audit dispositions: **{counts['PASS']} PASS, {counts['FAIL']} FAIL, {counts['UNTESTED']} UNTESTED** across the 584 unchanged AUDIT.md rows. UI rows: **{ui_counts['PASS']} PASS, {ui_counts['FAIL']} FAIL, {ui_counts['UNTESTED']} UNTESTED**. A passing package suite does not establish every exact expectation in AUDIT.md; uncovered variants are explicitly UNTESTED below. Hosted checks are assigned separately. For U-001/U-025, PASS covers the local URL only.

Nine UI rows fail the literal audit criteria. U-011 expects “Spec” but the landing says “Spec & evidence”; U-062 expects five sections but Keys is a div; U-067 exposes `0TAZ` rather than exact `0 TAZ` in innerText. U-071/U-081/U-093/U-095/U-096/U-101 fail the strict console rule: Chrome emits automatic resource diagnostics for the required 402/422/429/502 or the intentionally injected connection refusal. Their visible response behavior passed. Supplemental capture contains **13 console errors, 0 page errors, 0 unexpected HTTP 4xx/5xx**, and **2 failed requests deliberately induced for U-096**. The existing suite filters these expected HTTP diagnostics, explaining its green result. No source fixes were made.

All browser checks use installed Google Chrome 155.0.8059.85 via Playwright channel `chrome`, with desktop 1280 px and mobile 375 px. Supplements reuse one Chrome process per script. Network/console records include cross-origin failures, not only same-origin requests. Expected status codes remain in the record; console errors were not suppressed. Motion scheduling was observed within 150 ms browser timing tolerance; all four autoplay transitions and scroll/click/hover stops were exercised.

No Zcash transaction was sent. The existing suite uses synthetic signed receipts locally for paid-feed isolation tests; these are not chain payments. Only recorded transactions were looked up. No protected wallet, seed file, or project .env was opened by audit tooling. The authorized stack and existing tests loaded their own isolated throwaway key files without printing them. The TypeScript live test was skipped. U-083/U-084 require a fresh paid/unconfirmed transaction and remain UNTESTED.

Stack stop completed by recorded PID, Chrome closed, `e2e/.run/pids` removed, heavy slot released. [Cleanup verification](local/cleanup-verification.log), [commands/PIDs](local/test-summary.json), [method and boundaries](local/method.md), [per-row excerpts](local/excerpts.md), [browser summary](local/browser-summary.json). Only this report and `audit/local/` are delivery changes. No commit, push, deployment, AUDIT.md edit, or product-source edit was performed.

| ID | component/flow | exact expected result | status | evidence | fix |
| --- | --- | --- | --- | --- | --- |
'''
def cell(s):return s.replace('|','&#124;').replace('\n','<br>')
table=[]
for ident,component,expected in rows:
    status,evidence,fix=coverage.get(ident,('UNTESTED','No dedicated execution establishes every condition in this row. Package-suite pass counts are recorded above.','Not changed; outside exercised local browser/recorded-tx coverage.'))
    table.append('| '+' | '.join([ident,component,expected,status,cell(evidence),cell(fix)])+' |')
(root/'audit/results-local.md').write_text(header+'\n'.join(table)+'\n')
(out/'excerpts.md').write_text('Per-row evidence excerpts; raw logs are retained alongside this index.\n\n'+'\n'.join(excerpts))
(out/'disposition-counts.json').write_text(json.dumps({'all':dict(counts),'ui':dict(ui_counts)},indent=2)+'\n')
print(dict(counts),dict(ui_counts))
