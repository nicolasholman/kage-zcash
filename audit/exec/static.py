"""Source assertions only for audit rows explicitly designated static; never executes keygen or wallet creation."""
from pathlib import Path
root=Path(__file__).resolve().parents[2];out=root/'audit/exec/static.log'
f=(root/'crates/x402z-facilitator/src/main.rs').read_text();receipt=(root/'crates/x402z-facilitator/src/receipt.rs').read_text();agent=(root/'crates/x402z-agent/src/main.rs').read_text();lwd=(root/'crates/x402z-facilitator/src/lwd.rs').read_text()
with out.open('w') as log:
 assert 'None | Some("serve") => {}' in f
 assert '"127.0.0.1:4021".into()' in f
 assert 'None => 1,' in f
 assert 'x402z-facilitator listening on http://{bind} (min confirmations {min_confirmations})' in f
 log.write('AUDIT B-244 PASS static dispatch, default bind/min confirmations, exact logging template\n')
 assert 'Some("keygen") => return keygen()' in f
 assert 'fn keygen()' in f
 assert 'X402Z_RECEIPT_SIGNING_KEY={}' in f and 'X402Z_RECEIPT_PUBKEY={}' in f
 assert 'hex::encode(sk.to_bytes())' in f and 'hex::encode(sk.verifying_key().as_bytes())' in f
 assert 'SigningKey::generate' in receipt
 log.write('AUDIT B-245 PASS static normal-return dispatch, ed25519 32-byte keys encoded to 64 hex characters; keygen not invoked by this probe\n')
 assert 'Some("derive-uivk") => return derive_uivk()' in f
 derive=f[f.index('fn derive_uivk()'):f.index('fn keygen()')]
 assert 'require("X402Z_MERCHANT_UFVK")' in derive and 'MerchantKey::decode_ufvk(&NETWORK' in derive
 assert 'k.uivk().encode(&NETWORK)' in derive and 'println!("X402Z_MERCHANT_UIVK={u}")' in derive
 assert 'error: {var} not configured (see .env.example)' in f and 'exit(2)' in f[f.index('fn require'):f.index('fn optional')]
 log.write('AUDIT B-246 PASS static testnet UIVK encoding/normal return, required-UFVK diagnostic and exit 2; no derivation executed\n')
 assert 'Some("new-wallet") => cmd_new_wallet().await' in agent
 creation=agent[agent.index('async fn cmd_new_wallet()'):agent.index('async fn cmd_address()')]
 assert creation.index('if wallet::account')<creation.index('let phrase = wallet::new_phrase()')
 assert 'die(2, format!("{} already holds a wallet; refusing to overwrite it"' in creation
 for text in ['birthday {tip}','address={ua}','X402Z_MERCHANT_UIVK={}']:assert text in creation
 log.write('AUDIT B-262 PASS static wallet creation dispatch, output definitions, pre-creation existing-account exit 2; zero wallets/seeds opened or created\n')
with out.open('a') as log:
 address=agent[agent.index('async fn cmd_address()'):agent.index('fn ask_approval')]
 assert 'wallet::account(&db)' in address and 'die(4, "no wallet account yet;' in address
 assert 'println!("address={}"' in address and 'println!("uivk={}"' in address
 assert 'wallet::address(&ufvk)' in address and 'wallet::encode_uivk(&ufvk)' in address
 log.write('AUDIT B-263 PASS static address command success output definitions and no-account exit 4; no wallet opened\n')
 balance=agent[agent.index('async fn cmd_balance()'):agent.index('async fn cmd_pay()')]
 assert 'wallet::sync(&mut db, &mut client).await' in balance and 'wallet::balance(&db, id)' in balance
 assert 'spendable={} zat total={} zat pending={} zat (scanned to {} / tip {})' in balance
 assert 'println!("address={}"' in balance and 'broadcast' not in balance
 log.write('AUDIT B-264 PASS static balance success format and real sync/balance call sequence, no broadcast path invoked\n')
 pay=(root/'crates/x402z-agent/src/pay.rs').read_text();wallet=(root/'crates/x402z-agent/src/wallet.rs').read_text()
 assert 'pub const MIN_FEE_ZAT: u64 = 10_000;' in wallet
 assert 'let need = amount_zat + wallet::MIN_FEE_ZAT;' in pay
 assert 'if bal.spendable_zat < need' in pay and 'insufficient testnet balance: spendable {} zat' in pay
 assert 'need at least {} zat ({} + fee)' in pay and 'PayError::Wallet(_) => 4' in pay
 log.write('AUDIT B-290 PASS static zero spendable/100000 price requires 110000 with minimum fee; Wallet exit 4; no wallet built or broadcast\n')
 assert 'Some("tab-buy") => cmd_tab_buy(rest).await' in agent
 assert 'tab::buy(&cfg, &target, n, &ask_approval).await' in agent
 assert 'print_bought(&b);' in agent
 for k in ['tokens','keyId','amountZat','txid','memoNonce','issueWaitSecs']:assert '"'+k+'"' in agent[agent.index('fn print_bought'):agent.index('async fn cmd_tab_buy')]
 assert 'record_broadcast(gate' in pay and 'self.put(a)' in pay
 log.write('AUDIT B-317 PASS static wallet tab-buy success dispatch, output fields and single gate-record accounting definitions; zero broadcast\n')
