//! Check a payment to the merchant with only its unified *incoming* viewing key (uivk).
//!
//!     cargo run -p x402z-facilitator --example check_uivk -- <uivktest1...> <txid> [memoNonce]
//!
//! Fetches the transaction from lightwalletd (`X402Z_LWD_URL`, default testnet.zec.rocks),
//! trial-decrypts every Sapling, Orchard and Ironwood output with the UIVK, and prints what it
//! finds. An incoming key sees received notes and their memos; it cannot see spends or change,
//! and it cannot spend.
//!
//!     cargo run -p x402z-facilitator --example check_uivk -- derive
//!
//! prints the UIVK for the UFVK in `X402Z_MERCHANT_UFVK` (read from the environment, never
//! from the command line).

use orchard::note_encryption::{DomainVersion, IronwoodVersion, NoteEncryptionDomain, OrchardVersion};
use sapling::note_encryption::SaplingDomain;
use x402z_facilitator::{lwd, verify, NETWORK};
use zcash_keys::keys::{UnifiedFullViewingKey, UnifiedIncomingViewingKey};
use zcash_note_encryption::try_note_decryption;
use zcash_primitives::transaction::components::sapling::zip212_enforcement;
use zcash_protocol::memo::{Memo, MemoBytes};

fn die(msg: impl std::fmt::Display) -> ! {
    eprintln!("error: {msg}");
    std::process::exit(1)
}

fn memo_text(m: &[u8; 512]) -> String {
    match MemoBytes::from_bytes(m).ok().and_then(|b| Memo::try_from(&b).ok()) {
        Some(Memo::Text(t)) => format!("text {:?}", &*t),
        Some(Memo::Empty) => "empty".into(),
        _ => "non-text".into(),
    }
}

fn orchard_family<V: DomainVersion>(
    pool: &str,
    ivk: &orchard::keys::IncomingViewingKey,
    bundle: &orchard::bundle::Bundle<orchard::bundle::Authorized, zcash_protocol::value::ZatBalance>,
    found: &mut Vec<(String, u64, String)>,
) {
    let p = orchard::keys::PreparedIncomingViewingKey::new(ivk);
    for a in bundle.actions() {
        let d = NoteEncryptionDomain::<V>::for_action(a);
        if let Some((note, _, memo)) = try_note_decryption(&d, &p, a) {
            found.push((pool.into(), note.value().inner(), memo_text(&memo)));
        }
    }
}

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) == Some("derive") {
        let s = std::env::var("X402Z_MERCHANT_UFVK").unwrap_or_else(|_| die("set X402Z_MERCHANT_UFVK"));
        let ufvk = UnifiedFullViewingKey::decode(&NETWORK, &s).unwrap_or_else(|e| die(e));
        println!("{}", ufvk.to_unified_incoming_viewing_key().encode(&NETWORK).unwrap_or_else(|e| die(format!("{e:?}"))));
        return;
    }
    let [uivk, txid, rest @ ..] = args.as_slice() else {
        die("usage: check_uivk <uivktest1...> <txid> [memoNonce]   |   check_uivk derive")
    };
    let uivk = UnifiedIncomingViewingKey::decode(&NETWORK, uivk).unwrap_or_else(|e| die(e));
    let id = lwd::parse_txid(txid).unwrap_or_else(|| die("bad txid"));
    let url = std::env::var("X402Z_LWD_URL").unwrap_or_else(|_| lwd::DEFAULT_LWD_URL.to_string());
    let mut c = lwd::connect(&url).await.unwrap_or_else(|e| die(e));
    let tip = lwd::chain_tip(&mut c).await.unwrap_or_else(|e| die(e));
    let (data, loc) = match lwd::get_transaction(&mut c, &id).await.unwrap_or_else(|e| die(e)) {
        lwd::FetchedTx::Found { data, location } => (data, location),
        lwd::FetchedTx::NotFound => die("transaction not found"),
    };
    let mined = match loc {
        lwd::TxLocation::Mined(h) => Some(h),
        lwd::TxLocation::Mempool => None,
        lwd::TxLocation::Orphaned => die("transaction is only on an orphaned fork"),
    };
    let tx = verify::parse_tx(&NETWORK, &data, mined, tip).unwrap_or_else(|e| die(e));
    if tx.txid() != id {
        die("lightwalletd returned a different transaction");
    }

    let mut found = Vec::new();
    if let (Some(b), Some(ivk)) = (tx.sapling_bundle(), uivk.sapling()) {
        let d = SaplingDomain::new(zip212_enforcement(&NETWORK, mined.unwrap_or(tip + 1)));
        let p = ivk.prepare();
        for o in b.shielded_outputs() {
            if let Some((note, _, memo)) = try_note_decryption(&d, &p, o) {
                found.push(("Sapling".into(), note.value().inner(), memo_text(&memo)));
            }
        }
    }
    if let Some(ivk) = uivk.orchard() {
        if let Some(b) = tx.orchard_bundle() {
            orchard_family::<OrchardVersion>("Orchard", ivk, b, &mut found);
        }
        if let Some(b) = tx.ironwood_bundle() {
            orchard_family::<IronwoodVersion>("Ironwood", ivk, b, &mut found);
        }
    }

    let conf = verify::confirmations(mined, tip);
    match mined {
        Some(h) => println!("tx {txid}: mined at {h}, {conf} confirmation(s) (tip {tip})"),
        None => println!("tx {txid}: in mempool, 0 confirmations (tip {tip})"),
    }
    if found.is_empty() {
        die("no output decrypts with this incoming viewing key");
    }
    let nonce = rest.first();
    let mut total = 0u64;
    for (pool, v, memo) in &found {
        println!("incoming {v} zat via {pool}, memo {memo}");
        if nonce.is_none_or(|n| *memo == format!("text {n:?}")) {
            total += v;
        }
    }
    match nonce {
        Some(n) => {
            println!("total to merchant with memo {n:?}: {total} zat");
            if total == 0 {
                std::process::exit(2);
            }
        }
        None => println!("total to merchant: {total} zat"),
    }
}
