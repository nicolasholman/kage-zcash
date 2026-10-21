//! Durable challenge book: every 402 challenge the server issued, and whether its nonce has
//! been redeemed, in SQLite (`X402Z_SERVER_DB`). A restart between the 402 and the redeem
//! keeps the challenge redeemable, and a redeemed nonce stays burned.

use std::{path::Path, sync::Mutex};

use rusqlite::{params, Connection, OptionalExtension};
use x402z_protocol::PaymentRequired;

pub struct Store {
    conn: Mutex<Connection>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    pub challenge: PaymentRequired,
    /// Txid the nonce was redeemed with, once spent.
    pub spent_txid: Option<String>,
}

fn err(e: impl std::fmt::Display) -> String {
    format!("server database error: {e}")
}

impl Store {
    /// Open (or create) the database at `path`. `:memory:` gives a throwaway store.
    pub fn open(path: &Path) -> Result<Self, String> {
        let conn = Connection::open(path).map_err(|e| format!("cannot open X402Z_SERVER_DB {}: {e}", path.display()))?;
        conn.execute_batch(
            "PRAGMA journal_mode=WAL;
             PRAGMA busy_timeout=5000;
             CREATE TABLE IF NOT EXISTS challenges (
                 nonce      TEXT PRIMARY KEY,
                 challenge  TEXT NOT NULL,
                 expires_at INTEGER NOT NULL,
                 spent_txid TEXT,
                 spent_at   INTEGER
             );",
        )
        .map_err(err)?;
        Ok(Self { conn: Mutex::new(conn) })
    }

    pub fn insert(&self, ch: &PaymentRequired) -> Result<(), String> {
        let json = serde_json::to_string(ch).map_err(err)?;
        self.conn
            .lock()
            .unwrap()
            .execute(
                "INSERT INTO challenges (nonce, challenge, expires_at) VALUES (?1, ?2, ?3)",
                params![ch.memo_nonce, json, ch.expires_at as i64],
            )
            .map_err(err)?;
        Ok(())
    }

    pub fn get(&self, nonce: &str) -> Result<Option<Row>, String> {
        let row: Option<(String, Option<String>)> = self
            .conn
            .lock()
            .unwrap()
            .query_row("SELECT challenge, spent_txid FROM challenges WHERE nonce = ?1", params![nonce], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })
            .optional()
            .map_err(err)?;
        match row {
            None => Ok(None),
            Some((json, spent_txid)) => {
                let challenge = serde_json::from_str(&json).map_err(err)?;
                Ok(Some(Row { challenge, spent_txid }))
            }
        }
    }

    /// Burn `nonce` for `txid`. False if it was already spent (or never issued), so two
    /// concurrent redeems of one nonce can't both succeed.
    pub fn mark_spent(&self, nonce: &str, txid: &str, now: u64) -> Result<bool, String> {
        let n = self
            .conn
            .lock()
            .unwrap()
            .execute(
                "UPDATE challenges SET spent_txid = ?2, spent_at = ?3 WHERE nonce = ?1 AND spent_txid IS NULL",
                params![nonce, txid, now as i64],
            )
            .map_err(err)?;
        Ok(n == 1)
    }

    /// Unredeemed challenges that have not expired yet.
    pub fn count_open(&self, now: u64) -> Result<u64, String> {
        self.conn
            .lock()
            .unwrap()
            .query_row(
                "SELECT COUNT(*) FROM challenges WHERE spent_txid IS NULL AND expires_at > ?1",
                params![now as i64],
                |r| r.get::<_, i64>(0),
            )
            .map(|n| n as u64)
            .map_err(err)
    }

    /// Forget unredeemed challenges whose redeem window closed before `cutoff` (expiry time).
    /// Spent rows are kept so a redeemed nonce can never be redeemed again.
    pub fn prune_unspent_expired_before(&self, cutoff: u64) -> Result<usize, String> {
        self.conn
            .lock()
            .unwrap()
            .execute("DELETE FROM challenges WHERE spent_txid IS NULL AND expires_at < ?1", params![cutoff as i64])
            .map_err(err)
    }
}

#[cfg(test)]
impl Store {
    pub fn conn_for_tests(&self) -> std::sync::MutexGuard<'_, Connection> {
        self.conn.lock().unwrap()
    }
}
