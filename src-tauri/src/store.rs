//! Local storage. Everything secret is stored as an encrypted [`Envelope`];
//! the database never holds a plaintext host address, username, or key.

use crate::crypto::Envelope;
use rusqlite::{params, Connection, OptionalExtension};
use std::path::Path;
use thiserror::Error;

pub const SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Error)]
pub enum StoreError {
    #[error("数据库错误：{0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("数据库中还没有保险库")]
    NoVault,
    #[error("存储的密文格式无效：{0}")]
    Envelope(#[from] serde_json::Error),
}

/// One stored row: an opaque, already-encrypted blob under a stable id.
#[derive(Debug, Clone)]
pub struct ItemRow {
    pub id: String,
    pub kind: String,
    pub envelope: Envelope,
    pub updated_at: i64,
}

pub struct Store {
    conn: Connection,
}

impl Store {
    pub fn open(path: &Path) -> Result<Self, StoreError> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).ok();
        }
        let conn = Connection::open(path)?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        let store = Self { conn };
        store.migrate()?;
        Ok(store)
    }

    pub fn open_in_memory() -> Result<Self, StoreError> {
        let store = Self {
            conn: Connection::open_in_memory()?,
        };
        store.migrate()?;
        Ok(store)
    }

    /// Schema is created once; later versions add tables idempotently.
    fn migrate(&self) -> Result<(), StoreError> {
        self.conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS meta (
                key   TEXT PRIMARY KEY,
                value TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS items (
                id         TEXT PRIMARY KEY,
                kind       TEXT NOT NULL,
                envelope   TEXT NOT NULL,
                updated_at INTEGER NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_items_kind ON items(kind);
            CREATE INDEX IF NOT EXISTS idx_items_updated ON items(updated_at);
            "#,
        )?;
        self.set_meta("schema_version", &SCHEMA_VERSION.to_string())?;
        Ok(())
    }

    pub fn get_meta(&self, key: &str) -> Result<Option<String>, StoreError> {
        let v = self
            .conn
            .query_row("SELECT value FROM meta WHERE key = ?1", params![key], |r| {
                r.get::<_, String>(0)
            })
            .optional()?;
        Ok(v)
    }

    pub fn set_meta(&self, key: &str, value: &str) -> Result<(), StoreError> {
        self.conn.execute(
            "INSERT INTO meta(key, value) VALUES(?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![key, value],
        )?;
        Ok(())
    }

    pub fn delete_meta(&self, key: &str) -> Result<(), StoreError> {
        self.conn
            .execute("DELETE FROM meta WHERE key = ?1", params![key])?;
        Ok(())
    }

    pub fn put_item(&self, row: &ItemRow) -> Result<(), StoreError> {
        let env = serde_json::to_string(&row.envelope)?;
        self.conn.execute(
            "INSERT INTO items(id, kind, envelope, updated_at) VALUES(?1, ?2, ?3, ?4)
             ON CONFLICT(id) DO UPDATE SET
                 kind = excluded.kind,
                 envelope = excluded.envelope,
                 updated_at = excluded.updated_at",
            params![row.id, row.kind, env, row.updated_at],
        )?;
        Ok(())
    }

    pub fn get_item(&self, id: &str) -> Result<Option<ItemRow>, StoreError> {
        let raw = self
            .conn
            .query_row(
                "SELECT id, kind, envelope, updated_at FROM items WHERE id = ?1",
                params![id],
                |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, String>(2)?,
                        r.get::<_, i64>(3)?,
                    ))
                },
            )
            .optional()?;
        match raw {
            Some((id, kind, env, updated_at)) => Ok(Some(ItemRow {
                id,
                kind,
                envelope: serde_json::from_str(&env)?,
                updated_at,
            })),
            None => Ok(None),
        }
    }

    pub fn list_items(&self) -> Result<Vec<ItemRow>, StoreError> {
        let mut stmt = self
            .conn
            .prepare("SELECT id, kind, envelope, updated_at FROM items ORDER BY updated_at DESC")?;
        let rows = stmt
            .query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, i64>(3)?,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?;

        rows.into_iter()
            .map(|(id, kind, env, updated_at)| {
                Ok(ItemRow {
                    id,
                    kind,
                    envelope: serde_json::from_str(&env)?,
                    updated_at,
                })
            })
            .collect()
    }

    pub fn delete_item(&self, id: &str) -> Result<(), StoreError> {
        self.conn
            .execute("DELETE FROM items WHERE id = ?1", params![id])?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crypto::seal;

    fn row(id: &str, kind: &str) -> ItemRow {
        let key = [5u8; 32];
        ItemRow {
            id: id.to_string(),
            kind: kind.to_string(),
            envelope: seal(&key, b"aad", b"payload").unwrap(),
            updated_at: 1,
        }
    }

    #[test]
    fn items_roundtrip() {
        let store = Store::open_in_memory().unwrap();
        store.put_item(&row("a", "host")).unwrap();
        store.put_item(&row("b", "host")).unwrap();
        assert_eq!(store.list_items().unwrap().len(), 2);

        let got = store.get_item("a").unwrap().unwrap();
        assert_eq!(got.kind, "host");
        store.delete_item("a").unwrap();
        assert!(store.get_item("a").unwrap().is_none());
        assert_eq!(store.list_items().unwrap().len(), 1);
    }

    #[test]
    fn upsert_replaces_envelope() {
        let store = Store::open_in_memory().unwrap();
        store.put_item(&row("x", "host")).unwrap();
        let mut updated = row("x", "key");
        updated.updated_at = 99;
        store.put_item(&updated).unwrap();
        assert_eq!(store.get_item("x").unwrap().unwrap().kind, "key");
        assert_eq!(store.list_items().unwrap().len(), 1);
    }

    #[test]
    fn meta_roundtrip() {
        let store = Store::open_in_memory().unwrap();
        assert_eq!(store.get_meta("missing").unwrap(), None);
        store.set_meta("vault_salt", "abc").unwrap();
        assert_eq!(
            store.get_meta("vault_salt").unwrap().as_deref(),
            Some("abc")
        );
        store.delete_meta("vault_salt").unwrap();
        assert_eq!(store.get_meta("vault_salt").unwrap(), None);
    }
}
