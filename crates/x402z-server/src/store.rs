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
    /// When it was spent (unix seconds).
    pub spent_at: Option<u64>,
    /// The nonce paid for a Kage Tab (it bought tokens, not this one request).
    pub spent_for_tab: bool,
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
             );
             -- Open (unspent) challenges by expiry: pruning and the open count read only these
             -- rows instead of scanning every challenge ever issued.
             CREATE INDEX IF NOT EXISTS challenges_open ON challenges(expires_at) WHERE spent_txid IS NULL;
             -- Kage Tab: blind-RSA issuing keys (one active per epoch) and spent tokens.
             CREATE TABLE IF NOT EXISTS tab_keys (
                 key_id     TEXT PRIMARY KEY,
                 sk_der     BLOB NOT NULL,
                 created_at INTEGER NOT NULL,
                 active     INTEGER NOT NULL
             );
             CREATE TABLE IF NOT EXISTS tab_issued (
                 nonce     TEXT PRIMARY KEY,
                 key_id    TEXT NOT NULL,
                 tokens    INTEGER NOT NULL,
                 issued_at INTEGER NOT NULL
             );
             CREATE TABLE IF NOT EXISTS tab_spent (
                 token_id TEXT PRIMARY KEY,
                 key_id   TEXT NOT NULL,
                 spent_at INTEGER NOT NULL
             );",
        )
        .map_err(err)?;
        // When a tab key stopped being the active one (added after the first release).
        let has_retired_at = conn
            .prepare("SELECT 1 FROM pragma_table_info('tab_keys') WHERE name = 'retired_at'")
            .and_then(|mut q| q.exists([]))
            .map_err(err)?;
        if !has_retired_at {
            conn.execute_batch("ALTER TABLE tab_keys ADD COLUMN retired_at INTEGER").map_err(err)?;
        }
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
        let row: Option<(String, Option<String>, Option<i64>, bool)> = self
            .conn
            .lock()
            .unwrap()
            .query_row(
                "SELECT challenge, spent_txid, spent_at, EXISTS(SELECT 1 FROM tab_issued t WHERE t.nonce = c.nonce)
                 FROM challenges c WHERE c.nonce = ?1",
                params![nonce],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .optional()
            .map_err(err)?;
        match row {
            None => Ok(None),
            Some((json, spent_txid, spent_at, spent_for_tab)) => {
                let challenge = serde_json::from_str(&json).map_err(err)?;
                Ok(Some(Row { challenge, spent_txid, spent_at: spent_at.map(|t| t.max(0) as u64), spent_for_tab }))
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

/// Kage Tab storage.
impl Store {
    /// The active issuing key: (key_id, secret key DER, created_at).
    pub fn active_tab_key(&self) -> Result<Option<(String, Vec<u8>, u64)>, String> {
        self.conn
            .lock()
            .unwrap()
            .query_row("SELECT key_id, sk_der, created_at FROM tab_keys WHERE active = 1", [], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get::<_, i64>(2)? as u64))
            })
            .optional()
            .map_err(err)
    }

    /// The most recently retired key: (key_id, secret key DER, created_at, retired_at).
    pub fn previous_tab_key(&self) -> Result<Option<(String, Vec<u8>, u64, u64)>, String> {
        self.conn
            .lock()
            .unwrap()
            .query_row(
                "SELECT key_id, sk_der, created_at, retired_at FROM tab_keys WHERE active = 0 AND retired_at IS NOT NULL
                 ORDER BY retired_at DESC, created_at DESC LIMIT 1",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get::<_, i64>(2)? as u64, r.get::<_, i64>(3)? as u64)),
            )
            .optional()
            .map_err(err)
    }

    /// Retire every key and make `key_id` the only active one (a new epoch).
    pub fn install_tab_key(&self, key_id: &str, sk_der: &[u8], now: u64) -> Result<(), String> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction().map_err(err)?;
        tx.execute("UPDATE tab_keys SET retired_at = ?1 WHERE active = 1", params![now as i64]).map_err(err)?;
        tx.execute("UPDATE tab_keys SET active = 0", []).map_err(err)?;
        tx.execute(
            "INSERT INTO tab_keys (key_id, sk_der, created_at, active) VALUES (?1, ?2, ?3, 1)",
            params![key_id, sk_der, now as i64],
        )
        .map_err(err)?;
        tx.commit().map_err(err)
    }

    /// Burn `nonce` for `txid` and record the tab issuance in one transaction. False if the
    /// nonce was already spent (by a tab or a single payment), so a nonce issues at most once.
    pub fn spend_for_tab(&self, nonce: &str, txid: &str, key_id: &str, tokens: u32, now: u64) -> Result<bool, String> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction().map_err(err)?;
        let n = tx
            .execute(
                "UPDATE challenges SET spent_txid = ?2, spent_at = ?3 WHERE nonce = ?1 AND spent_txid IS NULL",
                params![nonce, txid, now as i64],
            )
            .map_err(err)?;
        if n != 1 {
            return Ok(false);
        }
        tx.execute(
            "INSERT INTO tab_issued (nonce, key_id, tokens, issued_at) VALUES (?1, ?2, ?3, ?4)",
            params![nonce, key_id, tokens, now as i64],
        )
        .map_err(err)?;
        tx.commit().map_err(err)?;
        Ok(true)
    }

    /// Record a redeemed token. False if it was already spent.
    pub fn spend_token(&self, token_id: &str, key_id: &str, now: u64) -> Result<bool, String> {
        let n = self
            .conn
            .lock()
            .unwrap()
            .execute(
                "INSERT OR IGNORE INTO tab_spent (token_id, key_id, spent_at) VALUES (?1, ?2, ?3)",
                params![token_id, key_id, now as i64],
            )
            .map_err(err)?;
        Ok(n == 1)
    }
}

#[cfg(test)]
impl Store {
    pub fn conn_for_tests(&self) -> std::sync::MutexGuard<'_, Connection> {
        self.conn.lock().unwrap()
    }
}
