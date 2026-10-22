//! Durable record of which memo nonce was receipted for which txid (`X402Z_FACILITATOR_DB`),
//! so a restart can't be used to get one nonce receipted for a second transaction.
//!
//! A binding is keyed by the whole requirement it was checked against: (nonce, payTo,
//! amountZat). A 1-zat transaction with a real challenge's nonce in its memo, settled at an amount
//! of 1, therefore can't take that nonce away from the payment the challenge actually asked for.
//! Rows from the older nonce-only table (`issued`) still block their nonce.

use std::{path::Path, sync::Mutex};

use rusqlite::{params, Connection, OptionalExtension};

pub struct IssuedStore {
    conn: Mutex<Connection>,
}

fn err(e: impl std::fmt::Display) -> String {
    format!("facilitator database error: {e}")
}

impl IssuedStore {
    /// Open (or create) the database at `path`. `:memory:` gives a throwaway store.
    pub fn open(path: &Path) -> Result<Self, String> {
        let conn =
            Connection::open(path).map_err(|e| format!("cannot open X402Z_FACILITATOR_DB {}: {e}", path.display()))?;
        conn.execute_batch(
            "PRAGMA journal_mode=WAL;
             PRAGMA busy_timeout=5000;
             CREATE TABLE IF NOT EXISTS issued (
                 nonce     TEXT PRIMARY KEY,
                 txid      TEXT NOT NULL,
                 issued_at INTEGER NOT NULL
             );
             CREATE TABLE IF NOT EXISTS bindings (
                 nonce      TEXT NOT NULL,
                 pay_to     TEXT NOT NULL,
                 amount_zat INTEGER NOT NULL,
                 txid       TEXT NOT NULL,
                 issued_at  INTEGER NOT NULL,
                 PRIMARY KEY (nonce, pay_to, amount_zat)
             );",
        )
        .map_err(err)?;
        Ok(Self { conn: Mutex::new(conn) })
    }

    /// The txid the requirement (nonce, payTo, amount) was receipted for, if any.
    pub fn txid_for(&self, nonce: &str, pay_to: &str, amount_zat: u64) -> Result<Option<String>, String> {
        let conn = self.conn.lock().unwrap();
        Self::bound(&conn, nonce, pay_to, amount_zat)
    }

    fn bound(conn: &Connection, nonce: &str, pay_to: &str, amount_zat: u64) -> Result<Option<String>, String> {
        let legacy: Option<String> =
            conn.query_row("SELECT txid FROM issued WHERE nonce = ?1", params![nonce], |r| r.get(0)).optional().map_err(err)?;
        if legacy.is_some() {
            return Ok(legacy);
        }
        conn.query_row(
            "SELECT txid FROM bindings WHERE nonce = ?1 AND pay_to = ?2 AND amount_zat = ?3",
            params![nonce, pay_to, amount_zat as i64],
            |r| r.get(0),
        )
        .optional()
        .map_err(err)
    }

    /// Bind the requirement (nonce, payTo, amount) to `txid`. Returns the txid it is bound to
    /// afterwards: `txid` itself, or an earlier one (then the caller must refuse). Re-receipting
    /// the same pair is allowed.
    pub fn bind(&self, nonce: &str, pay_to: &str, amount_zat: u64, txid: &str, now: u64) -> Result<String, String> {
        let conn = self.conn.lock().unwrap();
        if let Some(prev) = Self::bound(&conn, nonce, pay_to, amount_zat)? {
            return Ok(prev);
        }
        conn.execute(
            "INSERT OR IGNORE INTO bindings (nonce, pay_to, amount_zat, txid, issued_at) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![nonce, pay_to, amount_zat as i64, txid, now as i64],
        )
        .map_err(err)?;
        Self::bound(&conn, nonce, pay_to, amount_zat)?.ok_or_else(|| err("binding vanished after insert"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> std::path::PathBuf {
        let p = std::env::temp_dir().join(format!("x402z-fac-{}-{name}.sqlite", std::process::id()));
        for ext in ["", "-wal", "-shm"] {
            let _ = std::fs::remove_file(format!("{}{ext}", p.display()));
        }
        p
    }

    const UA: &str = "utest1merchant";

    #[test]
    fn nonce_binds_to_first_txid_and_survives_restart() {
        let path = tmp("bind");
        {
            let s = IssuedStore::open(&path).unwrap();
            assert_eq!(s.txid_for("n1", UA, 100).unwrap(), None);
            assert_eq!(s.bind("n1", UA, 100, "aa", 1).unwrap(), "aa");
            assert_eq!(s.bind("n1", UA, 100, "aa", 2).unwrap(), "aa"); // same pair again is fine
        }
        let s = IssuedStore::open(&path).unwrap(); // restart
        assert_eq!(s.txid_for("n1", UA, 100).unwrap().as_deref(), Some("aa"));
        assert_eq!(s.bind("n1", UA, 100, "bb", 3).unwrap(), "aa"); // a second tx can't take the nonce
        assert_eq!(s.bind("n2", UA, 100, "bb", 3).unwrap(), "bb");
    }

    #[test]
    fn a_dust_binding_at_another_amount_does_not_lock_out_the_real_payment() {
        let s = IssuedStore::open(std::path::Path::new(":memory:")).unwrap();
        // ATTACK: 1 zat + fee to the merchant with the open nonce in the memo, settled at amount 1
        assert_eq!(s.bind("n", UA, 1, "dust", 1).unwrap(), "dust");
        // the real payer's full payment for the real requirement still binds
        assert_eq!(s.txid_for("n", UA, 100_000).unwrap(), None);
        assert_eq!(s.bind("n", UA, 100_000, "real", 2).unwrap(), "real");
        // and is the only tx that ever binds it
        assert_eq!(s.bind("n", UA, 100_000, "other", 3).unwrap(), "real");
        // a binding to another payTo is a different requirement too
        assert_eq!(s.bind("n", "utest1someoneelse", 100_000, "x", 4).unwrap(), "x");
    }

    #[test]
    fn rows_from_the_old_nonce_only_table_still_block_their_nonce() {
        let path = tmp("legacy");
        {
            let c = Connection::open(&path).unwrap();
            c.execute_batch("CREATE TABLE issued (nonce TEXT PRIMARY KEY, txid TEXT NOT NULL, issued_at INTEGER NOT NULL); INSERT INTO issued VALUES ('old', 'aa', 1);").unwrap();
        }
        let s = IssuedStore::open(&path).unwrap();
        assert_eq!(s.txid_for("old", UA, 100_000).unwrap().as_deref(), Some("aa"));
        assert_eq!(s.bind("old", UA, 100_000, "bb", 2).unwrap(), "aa");
    }
}
