//! Sync with a user-owned Cloudflare Worker.
//!
//! The engine sends and receives only what is already encrypted locally. It does
//! not derive keys (that is [`crate::crypto`]) and it does not decide what an item
//! means (that is [`crate::vault`]); it moves envelopes and version numbers
//! between the local SQLite file and the Worker.
//!
//! Conflict rule: last write wins on the plaintext `updated_at` the client
//! recorded, and a tie keeps the server copy. The server enforces the same rule,
//! so a stale push is rejected rather than silently overwriting.

use serde::{Deserialize, Serialize};
use std::time::Duration;

const META_CURSOR: &str = "sync_cursor";
const META_URL: &str = "sync_url";
const META_TOKEN: &str = "sync_token";

#[derive(Debug, thiserror::Error)]
pub enum SyncError {
    #[error("同步服务地址无效：{0}")]
    Config(String),
    #[error("请求失败：{0}")]
    Http(String),
    #[error("同步服务返回错误：{0}")]
    Server(String),
    #[error("本地数据库错误：{0}")]
    Store(String),
    #[error("尚未配置同步")]
    NotConfigured,
}

/// One item as it travels over the wire. `envelope` is the same JSON blob the
/// local database holds.
///
/// Field names match the Worker's JSON (snake_case); the Worker is the contract
/// here, not the Rust conventions.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WireItem {
    pub id: String,
    pub kind: String,
    pub envelope: String,
    pub revision: u64,
    pub seq: u64,
    #[serde(default)]
    pub deleted: bool,
    pub updated_at: i64,
}

/// One page of changes from the Worker.
#[derive(Debug, Deserialize)]
pub struct PullPage {
    pub items: Vec<WireItem>,
    pub next_since: u64,
    pub has_more: bool,
}

/// What the Worker did with a batch.
#[derive(Debug, Deserialize)]
pub struct PushResult {
    pub accepted: u64,
    pub rejected: u64,
    pub seq: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncReport {
    pub pushed: u64,
    pub pulled: u64,
    pub rejected: u64,
    pub cursor: u64,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct Health {
    pub service: String,
    pub api: u32,
    pub initialized: bool,
}

pub struct Client {
    base: String,
    token: Option<String>,
    http: reqwest::Client,
}

impl Client {
    pub fn new(base: &str) -> Result<Self, SyncError> {
        let trimmed = base.trim().trim_end_matches('/');
        if !(trimmed.starts_with("https://") || trimmed.starts_with("http://")) {
            return Err(SyncError::Config("需要以 https:// 开头".into()));
        }
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .map_err(|e| SyncError::Http(e.to_string()))?;
        Ok(Self {
            base: trimmed.to_string(),
            token: None,
            http,
        })
    }

    pub fn with_token(mut self, token: impl Into<String>) -> Self {
        self.token = Some(token.into());
        self
    }

    fn url(&self, path: &str) -> String {
        format!("{}{path}", self.base)
    }

    fn auth(&self) -> Result<&str, SyncError> {
        self.token.as_deref().ok_or(SyncError::NotConfigured)
    }

    pub async fn health(&self) -> Result<Health, SyncError> {
        let resp = self
            .http
            .get(self.url("/v1/health"))
            .send()
            .await
            .map_err(|e| SyncError::Http(e.to_string()))?;
        resp.json()
            .await
            .map_err(|e| SyncError::Server(e.to_string()))
    }

    /// Pulls one page of changes.
    pub async fn pull(&self, since: u64, limit: u32) -> Result<PullPage, SyncError> {
        let resp = self
            .http
            .get(self.url(&format!("/v1/items?since={since}&limit={limit}")))
            .bearer_auth(self.auth()?)
            .send()
            .await
            .map_err(|e| SyncError::Http(e.to_string()))?;
        if !resp.status().is_success() {
            return Err(SyncError::Server(format!("拉取失败：{}", resp.status())));
        }
        resp.json()
            .await
            .map_err(|e| SyncError::Server(e.to_string()))
    }

    /// Pushes a batch. The server may reject individual items as stale.
    pub async fn push(&self, items: &[WireItem]) -> Result<PushResult, SyncError> {
        let resp = self
            .http
            .put(self.url("/v1/items"))
            .bearer_auth(self.auth()?)
            .json(&serde_json::json!({ "items": items }))
            .send()
            .await
            .map_err(|e| SyncError::Http(e.to_string()))?;
        if !resp.status().is_success() {
            return Err(SyncError::Server(format!("推送失败：{}", resp.status())));
        }
        resp.json()
            .await
            .map_err(|e| SyncError::Server(e.to_string()))
    }
}

/// Connects to a Worker and stores the address and token.
pub async fn connect(state: &crate::AppState, url: &str, token: &str) -> Result<Health, SyncError> {
    let client = Client::new(url)?.with_token(token);
    let health = client.health().await?;
    if health.service != "loki-sync" {
        return Err(SyncError::Config(format!(
            "这个地址不是 loki 同步服务（返回了 {}）",
            health.service
        )));
    }

    let vault = state
        .vault
        .lock()
        .map_err(|e| SyncError::Store(e.to_string()))?;
    vault
        .store()
        .set_meta(META_URL, url.trim_end_matches('/'))
        .map_err(|e| SyncError::Store(e.to_string()))?;
    vault
        .store()
        .set_meta(META_TOKEN, token)
        .map_err(|e| SyncError::Store(e.to_string()))?;
    Ok(health)
}

fn stored_client(state: &crate::AppState) -> Result<Client, SyncError> {
    let vault = state
        .vault
        .lock()
        .map_err(|e| SyncError::Store(e.to_string()))?;
    let url = vault
        .store()
        .get_meta(META_URL)
        .map_err(|e| SyncError::Store(e.to_string()))?
        .ok_or(SyncError::NotConfigured)?;
    let token = vault
        .store()
        .get_meta(META_TOKEN)
        .map_err(|e| SyncError::Store(e.to_string()))?
        .ok_or(SyncError::NotConfigured)?;
    Ok(Client::new(&url)?.with_token(token))
}

/// Kinds that stay on this machine.
///
/// A conversation holds server output and grows large; it is private to the
/// device that produced it, so it is neither pushed nor accepted from a peer.
pub fn is_local_only(kind: &str) -> bool {
    kind == "ai_conversation"
}

/// Rows that may be pushed to the sync server.
fn pushable(rows: &[crate::store::ItemRow]) -> impl Iterator<Item = &crate::store::ItemRow> {
    rows.iter().filter(|r| !is_local_only(&r.kind))
}

/// Runs one full sync: pull what is new, then push what is local and newer.
///
/// The vault lock is taken and released around each database step rather than
/// held across the HTTP calls: a MutexGuard is not Send, and holding it through
/// an await would also block every other command for the whole sync.
pub async fn run_once(state: &crate::AppState) -> Result<SyncReport, SyncError> {
    let client = stored_client(state)?;

    let (mut cursor, local_items) = {
        let vault = state
            .vault
            .lock()
            .map_err(|e| SyncError::Store(e.to_string()))?;
        let store = vault.store();
        let cursor = read_cursor(store)?;
        let items = store
            .list_items()
            .map_err(|e| SyncError::Store(e.to_string()))?;
        (cursor, items)
    };

    // ---- pull -------------------------------------------------------------
    let mut pulled = 0u64;
    loop {
        let page = client.pull(cursor, 100).await?;

        {
            let vault = state
                .vault
                .lock()
                .map_err(|e| SyncError::Store(e.to_string()))?;
            let store = vault.store();
            for item in &page.items {
                // Never let a peer inject a conversation into this device.
                if is_local_only(&item.kind) {
                    continue;
                }
                let envelope: crate::crypto::Envelope = match serde_json::from_str(&item.envelope) {
                    Ok(e) => e,
                    // A blob that does not parse is skipped, not fatal: one
                    // bad row must not stop the whole sync.
                    Err(_) => continue,
                };

                let local = store
                    .get_item(&item.id)
                    .map_err(|e| SyncError::Store(e.to_string()))?;

                // Apply only if the remote copy is newer, by the same rule the
                // server uses.
                let take = match &local {
                    None => true,
                    Some(l) => item.updated_at > l.updated_at,
                };

                if take {
                    store
                        .put_item(&crate::store::ItemRow {
                            id: item.id.clone(),
                            kind: item.kind.clone(),
                            envelope,
                            updated_at: item.updated_at,
                        })
                        .map_err(|e| SyncError::Store(e.to_string()))?;
                    pulled += 1;
                }
            }
        }

        cursor = page.next_since;
        if !page.has_more || page.items.is_empty() {
            break;
        }
    }

    // ---- push (no lock held) ----------------------------------------------
    let mut wire = Vec::with_capacity(local_items.len());
    for row in pushable(&local_items) {
        wire.push(WireItem {
            id: row.id.clone(),
            kind: row.kind.clone(),
            envelope: serde_json::to_string(&row.envelope).unwrap_or_default(),
            revision: 1,
            seq: 0,
            deleted: false,
            updated_at: row.updated_at,
        });
    }

    let mut pushed = 0;
    let mut rejected = 0;
    for chunk in wire.chunks(100) {
        let res = client.push(chunk).await?;
        pushed += res.accepted;
        rejected += res.rejected;
        if res.seq > cursor {
            cursor = res.seq;
        }
    }

    {
        let vault = state
            .vault
            .lock()
            .map_err(|e| SyncError::Store(e.to_string()))?;
        vault
            .store()
            .set_meta(META_CURSOR, &cursor.to_string())
            .map_err(|e| SyncError::Store(e.to_string()))?;
    }

    Ok(SyncReport {
        pushed,
        pulled,
        rejected,
        cursor,
    })
}

fn read_cursor(store: &crate::store::Store) -> Result<u64, SyncError> {
    Ok(store
        .get_meta(META_CURSOR)
        .map_err(|e| SyncError::Store(e.to_string()))?
        .and_then(|s| s.parse().ok())
        .unwrap_or(0))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(id: &str, kind: &str) -> crate::store::ItemRow {
        crate::store::ItemRow {
            id: id.to_string(),
            kind: kind.to_string(),
            envelope: crate::crypto::Envelope {
                v: 1,
                n: "n".into(),
                c: "c".into(),
            },
            updated_at: 1,
        }
    }

    #[test]
    fn conversations_are_never_pushed() {
        let rows = vec![
            row("h1", "host"),
            row("c1", "ai_conversation"),
            row("k1", "key"),
        ];
        let ids: Vec<&str> = pushable(&rows).map(|r| r.id.as_str()).collect();
        assert_eq!(ids, vec!["h1", "k1"]);
        assert!(is_local_only("ai_conversation"));
        assert!(!is_local_only("host"));
    }

    #[test]
    fn rejects_addresses_that_are_not_urls() {
        assert!(Client::new("example.com").is_err());
        assert!(Client::new("ftp://example.com").is_err());
        assert!(Client::new("https://sync.example.com").is_ok());
    }

    #[test]
    fn trailing_slashes_do_not_double_up() {
        let c = Client::new("https://sync.example.com/").unwrap();
        assert_eq!(c.url("/v1/health"), "https://sync.example.com/v1/health");
    }

    #[test]
    fn auth_is_required_for_pull_and_push() {
        let c = Client::new("https://sync.example.com").unwrap();
        assert!(matches!(c.auth(), Err(SyncError::NotConfigured)));
    }

    #[test]
    fn wire_item_shape_is_stable() {
        let item = WireItem {
            id: "h1".into(),
            kind: "host".into(),
            envelope: "{\"v\":1}".into(),
            revision: 2,
            seq: 7,
            deleted: false,
            updated_at: 1_700_000_000,
        };
        let v = serde_json::to_value(&item).unwrap();
        // These names are the Worker's contract; renaming them here silently
        // breaks every deployed backend.
        assert_eq!(v["id"], "h1");
        assert_eq!(v["updated_at"], 1_700_000_000i64);
        assert_eq!(v["deleted"], false);
        assert_eq!(v["revision"], 2);
    }
}
