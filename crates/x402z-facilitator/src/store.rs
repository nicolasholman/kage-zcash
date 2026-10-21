//! Durable record of which memo nonce was receipted for which txid (`X402Z_FACILITATOR_DB`),
//! so a restart can't be used to get one nonce receipted for a second transaction.

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
             );",
        )
        .map_err(err)?;
        Ok(Self { conn: Mutex::new(conn) })
    }

    /// The txid `nonce` was receipted for, if any.
    pub fn txid_for(&self, nonce: &str) -> Result<Option<String>, String> {
        self.conn
            .lock()
            .unwrap()
            .query_row("SELECT txid FROM issued WHERE nonce = ?1", params![nonce], |r| r.get(0))
            .optional()
            .map_err(err)
    }

    /// Bind `nonce` to `txid`. Returns the txid the nonce is bound to afterwards: `txid` itself,
    /// or an earlier one (then the caller must refuse). Re-receipting the same pair is allowed.
    pub fn bind(&self, nonce: &str, txid: &str, now: u64) -> Result<String, String> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT OR IGNORE INTO issued (nonce, txid, issued_at) VALUES (?1, ?2, ?3)",
            params![nonce, txid, now as i64],
        )
        .map_err(err)?;
        conn.query_row("SELECT txid FROM issued WHERE nonce = ?1", params![nonce], |r| r.get(0)).map_err(err)
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

    #[test]
    fn nonce_binds_to_first_txid_and_survives_restart() {
        let path = tmp("bind");
        {
            let s = IssuedStore::open(&path).unwrap();
            assert_eq!(s.txid_for("n1").unwrap(), None);
            assert_eq!(s.bind("n1", "aa", 1).unwrap(), "aa");
            assert_eq!(s.bind("n1", "aa", 2).unwrap(), "aa"); // same pair again is fine
        }
        let s = IssuedStore::open(&path).unwrap(); // restart
        assert_eq!(s.txid_for("n1").unwrap().as_deref(), Some("aa"));
        assert_eq!(s.bind("n1", "bb", 3).unwrap(), "aa"); // a second tx can't take the nonce
        assert_eq!(s.bind("n2", "bb", 3).unwrap(), "bb");
    }
}
