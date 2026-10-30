//! IPC surface. Every command returns `Result<T, String>` so the WebView gets a
//! readable message; secrets travel one way only (into Rust).

use crate::crypto::RecoveryCode;
use crate::ssh::{self, ConnectOutcome, TermEvent};
use crate::vault::{PlainItem, VaultStatus};
use crate::AppState;
use base64::Engine;
use serde::{Deserialize, Serialize};
use tauri::{Emitter, Manager, State};

/// A host as the UI sees it. Note what is absent: no password, no key material.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HostView {
    pub id: String,
    pub name: String,
    /// `ssh`, `telnet`, or `serial`. Defaults to `ssh` for older rows.
    pub protocol: String,
    pub address: String,
    pub port: u16,
    pub username: String,
    pub tags: Vec<String>,
    /// Serial framing; only meaningful when `protocol == "serial"`.
    pub baud: Option<u32>,
    pub data_bits: Option<u8>,
    pub parity: Option<String>,
    pub stop_bits: Option<u8>,
    /// Whether a password is stored, never the password itself.
    pub has_password: bool,
    /// Whether a private key is stored, never the key itself.
    pub has_key: bool,
    /// A key from the key vault to log in with, by id.
    pub key_id: Option<String>,
    /// Another saved host to tunnel through (`ProxyJump`).
    pub jump_host_id: Option<String>,
    /// Whether to offer the local ssh-agent's keys.
    pub use_agent: bool,
    /// Typed into the shell right after it opens.
    pub startup_command: Option<String>,
}

impl HostView {
    /// A plain SSH host; tests and non-serial call paths use this instead of
    /// spelling out every optional field.
    pub fn ssh(id: &str, name: &str, address: &str, port: u16, username: &str) -> Self {
        Self {
            id: id.to_string(),
            name: name.to_string(),
            protocol: "ssh".to_string(),
            address: address.to_string(),
            port,
            username: username.to_string(),
            tags: vec![],
            baud: None,
            data_bits: None,
            parity: None,
            stop_bits: None,
            has_password: false,
            has_key: false,
            key_id: None,
            jump_host_id: None,
            use_agent: true,
            startup_command: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionView {
    pub id: String,
    pub title: String,
    pub status: String,
    pub fingerprint: Option<String>,
}

#[tauri::command]
pub fn vault_status(state: State<'_, AppState>) -> Result<VaultStatus, String> {
    let vault = state.vault.lock().map_err(|e| e.to_string())?;
    Ok(vault.status())
}

/// What `vault_create` hands back: the status plus the one-time recovery code.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateVaultResult {
    pub status: VaultStatus,
    /// Shown once; never stored in plaintext anywhere.
    pub recovery_code: Option<String>,
}

#[tauri::command]
pub fn vault_create(
    state: State<'_, AppState>,
    password: String,
) -> Result<CreateVaultResult, String> {
    let mut vault = state.vault.lock().map_err(|e| e.to_string())?;
    let code = vault
        .create(&password, crate::crypto::KdfParams::default())
        .map_err(|e| e.to_string())?;
    Ok(CreateVaultResult {
        status: vault.status(),
        recovery_code: Some(code.display),
    })
}

/// Unlocks with a recovery code, for a user who forgot the password.
#[tauri::command]
pub fn vault_recover(state: State<'_, AppState>, code: String) -> Result<VaultStatus, String> {
    let parsed = RecoveryCode::parse(&code).map_err(|_| "恢复码格式不正确".to_string())?;
    let mut vault = state.vault.lock().map_err(|e| e.to_string())?;
    vault
        .unlock_with_recovery(&parsed)
        .map_err(|e| e.to_string())?;
    Ok(vault.status())
}

/// Writes the recovery code to a file the user picked, right after creation.
///
/// The code is passed back in because Rust never keeps it. It is parsed before
/// anything is written, so this cannot be used to write arbitrary text, and on
/// Unix the file is created readable by its owner only.
#[tauri::command]
pub fn recovery_code_export(path: String, code: String) -> Result<(), String> {
    let parsed = RecoveryCode::parse(&code).map_err(|_| "恢复码格式不正确".to_string())?;
    let body = recovery_file_text(&parsed.display);
    write_private_file(std::path::Path::new(&path), body.as_bytes())
        .map_err(|e| format!("无法保存到 {path}：{e}"))
}

fn recovery_file_text(code: &str) -> String {
    format!(
        "loki 恢复码 / loki recovery code\n\
         ================================\n\n\
         {code}\n\n\
         忘记主密码时，在解锁页点「忘记密码？」并输入上面的恢复码。\n\
         它和主密码一样重要：任何拿到它的人都能解开这台电脑上的 loki 保险库。\n\
         请打印或存进密码管理器，然后删除这个文件。\n\n\
         If you forget the master password, choose \"忘记密码？\" on the unlock screen and\n\
         enter the code above. Anyone holding it can open the loki vault on this computer:\n\
         print it or store it in a password manager, then delete this file.\n\n\
         生成时间 / Created: {}\n",
        format_utc(now_secs())
    )
}

/// `YYYY-MM-DD HH:MM UTC` for a Unix timestamp, without a date crate.
fn format_utc(secs: i64) -> String {
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    // Civil-from-days (Howard Hinnant), valid for the proleptic Gregorian calendar.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!(
        "{y:04}-{m:02}-{d:02} {:02}:{:02} UTC",
        rem / 3600,
        rem % 3600 / 60
    )
}

fn write_private_file(path: &std::path::Path, data: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let mut f = opts.open(path)?;
    f.write_all(data)?;
    f.sync_all()
}

/// Sets a new master password. The vault must be unlocked first.
#[tauri::command]
pub fn vault_change_password(
    state: State<'_, AppState>,
    new_password: String,
) -> Result<VaultStatus, String> {
    if new_password.len() < 8 {
        return Err("新密码至少 8 位".to_string());
    }
    let mut vault = state.vault.lock().map_err(|e| e.to_string())?;
    vault
        .change_password(&new_password, crate::crypto::KdfParams::default())
        .map_err(|e| e.to_string())?;
    Ok(vault.status())
}

#[tauri::command]
pub fn vault_unlock(state: State<'_, AppState>, password: String) -> Result<VaultStatus, String> {
    let mut vault = state.vault.lock().map_err(|e| e.to_string())?;
    vault.unlock(&password).map_err(|e| e.to_string())?;
    Ok(vault.status())
}

#[tauri::command]
/// Locks the vault and drops the key material.
///
/// With `close_sessions` the live SSH connections are torn down too; a locked
/// app should not keep a shell open that the UI can no longer show.
pub async fn lock_vault(
    app: &tauri::AppHandle,
    state: &AppState,
    close_sessions: bool,
) -> Result<VaultStatus, String> {
    let status = {
        let mut vault = state.vault.lock().map_err(|e| e.to_string())?;
        vault.lock();
        vault.status()
    };

    if close_sessions {
        close_everything(state).await;
    }

    // The UI listens for this to drop the lock screen in place.
    let _ = app.emit("vault-locked", &status);
    Ok(status)
}

/// Tears down every live connection: SSH, telnet, serial, local shells,
/// forwards and transfers. A locked app must not keep anything open that the
/// UI can no longer show or stop.
pub async fn close_everything(state: &AppState) {
    // Recordings must be flushed before their sessions disappear.
    finish_all_recordings(state);
    if let Ok(mut f) = state.forwards.lock() {
        for view in f.list() {
            f.remove(&view.id);
        }
    }
    if let Ok(mut t) = state.transfers.lock() {
        for flag in t.values() {
            flag.store(true, std::sync::atomic::Ordering::SeqCst);
        }
        t.clear();
    }
    if let Ok(mut m) = state.sftp.lock() {
        m.clear();
    }
    if let Ok(mut a) = state.attachments.lock() {
        a.clear();
    }
    if let Ok(mut d) = state.direct.lock() {
        d.stop_all();
    }
    let sessions = state.ssh.lock().map(|mut m| m.drain()).unwrap_or_default();
    for session in sessions {
        ssh::close(session).await;
    }
}

#[tauri::command]
pub async fn vault_lock(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
) -> Result<VaultStatus, String> {
    lock_vault(&app, &state, true).await
}

/// Lists hosts. Decryption happens here, in Rust, and the plaintext is never
/// persisted; only the fields below cross the IPC boundary.
#[tauri::command]
pub fn hosts_list(state: State<'_, AppState>) -> Result<Vec<HostView>, String> {
    let vault = state.vault.lock().map_err(|e| e.to_string())?;
    let rows = vault.store().list_items().map_err(|e| e.to_string())?;

    let mut out = Vec::new();
    for row in rows.iter().filter(|r| r.kind == "host") {
        let item = vault.decrypt_item(row).map_err(|e| e.to_string())?;
        out.push(to_host_view(&item));
    }
    Ok(out)
}

#[tauri::command]
pub fn host_save(state: State<'_, AppState>, host: HostInput) -> Result<String, String> {
    let vault = state.vault.lock().map_err(|e| e.to_string())?;

    let id = host
        .id
        .clone()
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());

    // Blank means "leave what is stored alone": editing a host must not wipe a
    // password the user never retyped.
    let existing = if host.id.is_some() {
        vault
            .store()
            .get_item(&id)
            .map_err(|e| e.to_string())?
            .map(|row| vault.decrypt_item(&row))
            .transpose()
            .map_err(|e| e.to_string())?
            .map(|item| item.payload)
    } else {
        None
    };
    let keep = |field: &str| -> Option<String> {
        existing
            .as_ref()
            .and_then(|p| p.get(field))
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .map(String::from)
    };

    let password = host
        .password
        .as_ref()
        .filter(|p| !p.is_empty())
        .cloned()
        .or_else(|| (!host.clear_password).then(|| keep("password")).flatten());
    let pasted = host
        .private_key
        .as_ref()
        .filter(|k| !k.trim().is_empty())
        .map(|k| ssh::normalize_private_key(k));
    let private_key = pasted
        .clone()
        .or_else(|| (!host.clear_key).then(|| keep("private_key")).flatten());
    // Catch a broken paste now rather than at the next connect attempt.
    if let Some(pem) = &pasted {
        if let Err(e) = russh::keys::decode_secret_key(pem, None) {
            let msg = e.to_string();
            if !msg.to_lowercase().contains("encrypted") && !msg.contains("passphrase") {
                return Err(format!("私钥无法解析：{msg}"));
            }
        }
    }
    if host.jump_host_id.as_deref() == Some(id.as_str()) {
        return Err("主机不能把自己当作跳板机".into());
    }

    let payload = serde_json::json!({
        "name": host.name,
        "protocol": host.protocol,
        "address": host.address,
        "port": host.port,
        "username": host.username,
        "tags": host.tags,
        "baud": host.baud,
        "data_bits": host.data_bits,
        "parity": host.parity,
        "stop_bits": host.stop_bits,
        // Credentials sit inside the same envelope as everything else, so an
        // operator reading the database cannot even tell one is present.
        "password": password,
        "private_key": private_key,
        "key_id": host.key_id.filter(|k| !k.is_empty()),
        "jump_host_id": host.jump_host_id.filter(|k| !k.is_empty()),
        "use_agent": host.use_agent,
        "startup_command": host.startup_command.filter(|c| !c.trim().is_empty()),
    });

    let row = vault
        .encrypt_item(&id, "host", &payload)
        .map_err(|e| e.to_string())?;
    vault.store().put_item(&row).map_err(|e| e.to_string())?;
    Ok(id)
}

#[tauri::command]
pub fn host_delete(state: State<'_, AppState>, id: String) -> Result<(), String> {
    let vault = state.vault.lock().map_err(|e| e.to_string())?;
    // A host that tunnelled through this one would otherwise fail with a
    // confusing "jump host not found" on its next connect.
    let rows = vault.store().list_items().map_err(|e| e.to_string())?;
    for row in rows.iter().filter(|r| r.kind == "host" && r.id != id) {
        let mut item = vault.decrypt_item(row).map_err(|e| e.to_string())?;
        if item.payload.get("jump_host_id").and_then(|v| v.as_str()) == Some(id.as_str()) {
            item.payload["jump_host_id"] = serde_json::Value::Null;
            let updated = vault
                .encrypt_item(&item.id, "host", &item.payload)
                .map_err(|e| e.to_string())?;
            vault
                .store()
                .put_item(&updated)
                .map_err(|e| e.to_string())?;
        }
    }
    vault.store().delete_item(&id).map_err(|e| e.to_string())
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HostInput {
    pub id: Option<String>,
    pub name: String,
    #[serde(default = "default_protocol")]
    pub protocol: String,
    pub address: String,
    pub port: u16,
    pub username: String,
    pub tags: Vec<String>,
    #[serde(default)]
    pub baud: Option<u32>,
    #[serde(default)]
    pub data_bits: Option<u8>,
    #[serde(default)]
    pub parity: Option<String>,
    #[serde(default)]
    pub stop_bits: Option<u8>,
    pub password: Option<String>,
    /// An `OpenSSH` private key, stored encrypted alongside the host.
    pub private_key: Option<String>,
    /// Drop the stored password instead of keeping it.
    #[serde(default)]
    pub clear_password: bool,
    /// Drop the stored private key instead of keeping it.
    #[serde(default)]
    pub clear_key: bool,
    #[serde(default)]
    pub key_id: Option<String>,
    #[serde(default)]
    pub jump_host_id: Option<String>,
    #[serde(default = "default_true")]
    pub use_agent: bool,
    #[serde(default)]
    pub startup_command: Option<String>,
}

fn default_true() -> bool {
    true
}

fn default_protocol() -> String {
    "ssh".to_string()
}

fn to_host_view(item: &PlainItem) -> HostView {
    let p = &item.payload;
    HostView {
        id: item.id.clone(),
        name: p
            .get("name")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        address: p
            .get("address")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        protocol: p
            .get("protocol")
            .and_then(|v| v.as_str())
            .unwrap_or("ssh")
            .to_string(),
        port: p.get("port").and_then(|v| v.as_u64()).unwrap_or(22) as u16,
        baud: p.get("baud").and_then(|v| v.as_u64()).map(|v| v as u32),
        data_bits: p.get("data_bits").and_then(|v| v.as_u64()).map(|v| v as u8),
        parity: p.get("parity").and_then(|v| v.as_str()).map(String::from),
        stop_bits: p.get("stop_bits").and_then(|v| v.as_u64()).map(|v| v as u8),
        username: p
            .get("username")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        tags: p
            .get("tags")
            .and_then(|v| v.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|t| t.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default(),
        has_password: p
            .get("password")
            .and_then(|v| v.as_str())
            .map(|s| !s.is_empty())
            .unwrap_or(false),
        has_key: p
            .get("private_key")
            .and_then(|v| v.as_str())
            .map(|s| !s.is_empty())
            .unwrap_or(false),
        key_id: str_field(p, "key_id"),
        jump_host_id: str_field(p, "jump_host_id"),
        use_agent: p.get("use_agent").and_then(|v| v.as_bool()).unwrap_or(true),
        startup_command: str_field(p, "startup_command"),
    }
}

fn str_field(p: &serde_json::Value, name: &str) -> Option<String> {
    p.get(name)
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(String::from)
}

/// Copies a host, secrets included, under a new id. The copy is made here
/// because the WebView never holds the secrets it would need to do it.
#[tauri::command]
pub fn host_duplicate(state: State<'_, AppState>, id: String) -> Result<String, String> {
    let vault = state.vault.lock().map_err(|e| e.to_string())?;
    let row = vault
        .store()
        .get_item(&id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "主机不存在".to_string())?;
    let mut payload = vault.decrypt_item(&row).map_err(|e| e.to_string())?.payload;
    let name = payload
        .get("name")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    payload["name"] = serde_json::Value::String(format!("{name} 副本"));
    let new_id = uuid::Uuid::new_v4().to_string();
    let row = vault
        .encrypt_item(&new_id, "host", &payload)
        .map_err(|e| e.to_string())?;
    vault.store().put_item(&row).map_err(|e| e.to_string())?;
    Ok(new_id)
}

#[tauri::command]
pub fn sessions_list(state: State<'_, AppState>) -> Result<Vec<SessionView>, String> {
    let mgr = state.ssh.lock().map_err(|e| e.to_string())?;
    Ok(mgr.list().iter().map(session_view).collect())
}

/// Pulls the host and its credential out of the vault.
///
/// The secret lives only in the returned value and is dropped once the
/// handshake finishes. A key takes precedence over a password when both exist.
fn load_host(state: &AppState, host_id: &str) -> Result<(HostView, serde_json::Value), String> {
    let vault = state.vault.lock().map_err(|e| e.to_string())?;
    let row = vault
        .store()
        .get_item(host_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "主机不存在".to_string())?;
    let item = vault.decrypt_item(&row).map_err(|e| e.to_string())?;
    let view = to_host_view(&item);
    Ok((view, item.payload))
}

/// Builds the credential list for a host payload, in the order `ssh` tries
/// them: the host's own key, a key from the key vault, the agent, then a
/// password (saved, or typed just now).
///
/// Separate from loading the host: telnet and serial have no credential, and
/// demanding one before the protocol is known rejected valid connections.
fn credentials_for(
    vault: &crate::vault::Vault,
    payload: &serde_json::Value,
    one_time_password: Option<&str>,
) -> Result<Vec<ssh::Credential>, String> {
    let field = |name: &str| -> String {
        payload
            .get(name)
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string()
    };
    let mut out = Vec::new();

    let key = field("private_key");
    if !key.is_empty() {
        out.push(ssh::Credential::Key {
            pem: key,
            passphrase: None,
        });
    }
    let key_id = field("key_id");
    if !key_id.is_empty() {
        let row = vault
            .store()
            .get_item(&key_id)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| "这台主机引用的密钥已被删除".to_string())?;
        let item = vault.decrypt_item(&row).map_err(|e| e.to_string())?;
        if let Some(pem) = item.payload.get("private_key").and_then(|v| v.as_str()) {
            out.push(ssh::Credential::Key {
                pem: pem.to_string(),
                passphrase: None,
            });
        }
    }
    if payload
        .get("use_agent")
        .and_then(|v| v.as_bool())
        .unwrap_or(true)
    {
        out.push(ssh::Credential::Agent);
    }
    let password = field("password");
    if !password.is_empty() {
        out.push(ssh::Credential::Password(password));
    } else if let Some(pw) = one_time_password.filter(|p| !p.is_empty()) {
        out.push(ssh::Credential::Password(pw.to_string()));
    }
    Ok(out)
}

/// The vault's trusted keys for `host:port`.
fn known_keys(
    vault: &crate::vault::Vault,
    host: &str,
    port: u16,
) -> Result<Vec<ssh::KnownKey>, String> {
    let rows = vault.store().list_items().map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    for row in rows.iter().filter(|r| r.kind == "known_host") {
        let item = vault.decrypt_item(row).map_err(|e| e.to_string())?;
        let p = &item.payload;
        let same_host = p
            .get("host")
            .and_then(|v| v.as_str())
            .is_some_and(|h| h.eq_ignore_ascii_case(host));
        let same_port = p.get("port").and_then(|v| v.as_u64()) == Some(port as u64);
        if same_host && same_port {
            out.push(ssh::KnownKey {
                key_type: str_field(p, "key_type").unwrap_or_default(),
                fingerprint: str_field(p, "fingerprint").unwrap_or_default(),
            });
        }
    }
    Ok(out)
}

/// Trusts a key for its `host:port`, replacing any older key of the same type.
fn save_known_key(vault: &crate::vault::Vault, key: &ssh::SeenKey) -> Result<(), String> {
    let rows = vault.store().list_items().map_err(|e| e.to_string())?;
    for row in rows.iter().filter(|r| r.kind == "known_host") {
        let item = vault.decrypt_item(row).map_err(|e| e.to_string())?;
        let p = &item.payload;
        let same = p
            .get("host")
            .and_then(|v| v.as_str())
            .is_some_and(|h| h.eq_ignore_ascii_case(&key.host))
            && p.get("port").and_then(|v| v.as_u64()) == Some(key.port as u64)
            && p.get("key_type").and_then(|v| v.as_str()) == Some(key.key_type.as_str());
        if same {
            vault
                .store()
                .delete_item(&row.id)
                .map_err(|e| e.to_string())?;
        }
    }
    let id = uuid::Uuid::new_v4().to_string();
    let payload = serde_json::json!({
        "host": key.host.to_lowercase(),
        "port": key.port,
        "key_type": key.key_type,
        "fingerprint": key.fingerprint,
        "added_at": now_secs(),
    });
    let row = vault
        .encrypt_item(&id, "known_host", &payload)
        .map_err(|e| e.to_string())?;
    vault.store().put_item(&row).map_err(|e| e.to_string())
}

fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KnownHostView {
    pub id: String,
    pub host: String,
    pub port: u16,
    pub key_type: String,
    pub fingerprint: String,
    pub added_at: i64,
}

#[tauri::command]
pub fn known_hosts_list(state: State<'_, AppState>) -> Result<Vec<KnownHostView>, String> {
    let vault = state.vault.lock().map_err(|e| e.to_string())?;
    let rows = vault.store().list_items().map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    for row in rows.iter().filter(|r| r.kind == "known_host") {
        let item = vault.decrypt_item(row).map_err(|e| e.to_string())?;
        let p = &item.payload;
        out.push(KnownHostView {
            id: item.id.clone(),
            host: str_field(p, "host").unwrap_or_default(),
            port: p.get("port").and_then(|v| v.as_u64()).unwrap_or(22) as u16,
            key_type: str_field(p, "key_type").unwrap_or_default(),
            fingerprint: str_field(p, "fingerprint").unwrap_or_default(),
            added_at: p.get("added_at").and_then(|v| v.as_i64()).unwrap_or(0),
        });
    }
    out.sort_by(|a, b| a.host.cmp(&b.host).then(a.port.cmp(&b.port)));
    Ok(out)
}

#[tauri::command]
pub fn known_host_delete(state: State<'_, AppState>, id: String) -> Result<(), String> {
    let vault = state.vault.lock().map_err(|e| e.to_string())?;
    vault.store().delete_item(&id).map_err(|e| e.to_string())
}

/// An unsaved SSH target typed into quick connect.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QuickTarget {
    pub address: String,
    pub port: u16,
    pub username: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenInput {
    /// A saved host, or…
    pub host_id: Option<String>,
    /// …an ad-hoc target.
    pub target: Option<QuickTarget>,
    /// The fingerprint the user just confirmed.
    pub accept: Option<String>,
    /// A password typed in for this attempt only, and the host it is for.
    pub password: Option<String>,
    pub password_host: Option<String>,
    /// Store the typed password on the saved host after a successful login.
    #[serde(default)]
    pub remember_password: bool,
    pub cols: Option<u32>,
    pub rows: Option<u32>,
}

/// Everything the terminal wants to know before it may connect.
#[derive(Debug, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ConnectResult {
    /// `connected`, `hostKey` or `needPassword`.
    pub kind: String,
    pub session: Option<SessionView>,
    /// For `hostKey`: what the server presented, and for which hop.
    pub fingerprint: Option<String>,
    pub key_type: Option<String>,
    pub host: Option<String>,
    pub port: Option<u16>,
    /// For `hostKey`: the fingerprint the vault had for this host before.
    /// Present means the key *changed*.
    pub previous: Option<String>,
    /// For `needPassword`.
    pub username: Option<String>,
}

/// Connects to a saved host or a quick-connect target.
#[tauri::command]
pub async fn session_open(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    input: OpenInput,
) -> Result<ConnectResult, String> {
    let cols = input.cols.unwrap_or(120);
    let rows = input.rows.unwrap_or(32);

    // Gather everything from the vault first, then drop the lock before
    // touching the network.
    let (host, payload) = match (&input.host_id, &input.target) {
        (Some(id), _) => load_host(&state, id)?,
        (None, Some(t)) => {
            let mut view = HostView::ssh("", "", &t.address, t.port, &t.username);
            view.name = format!("{}@{}", t.username, t.address);
            let payload = serde_json::json!({ "use_agent": true });
            (view, payload)
        }
        (None, None) => return Err("没有指定要连接的主机".into()),
    };

    // Telnet and serial do not use SSH at all; they feed the same event channel
    // so the terminal needs no special case. Dispatch before asking for a
    // credential, which those protocols do not have.
    if host.protocol == "telnet" {
        return open_telnet_host(&app, host, Some(cols), Some(rows)).await;
    }
    if host.protocol == "serial" {
        return open_serial_host(&app, host);
    }

    let pw_for = |address: &str| -> Option<&str> {
        match (&input.password, &input.password_host) {
            (Some(p), Some(h)) if h.eq_ignore_ascii_case(address) => Some(p.as_str()),
            // Without a host tag the password is for the target only.
            (Some(p), None) if address == host.address => Some(p.as_str()),
            _ => None,
        }
    };

    let (target, jump) = {
        let vault = state.vault.lock().map_err(|e| e.to_string())?;
        let target = ssh::Hop {
            address: host.address.clone(),
            port: host.port,
            username: host.username.clone(),
            credentials: credentials_for(&vault, &payload, pw_for(&host.address))?,
            known: known_keys(&vault, &host.address, host.port)?,
        };
        let jump = match host.jump_host_id.as_deref() {
            None => None,
            Some(jid) => {
                let row = vault
                    .store()
                    .get_item(jid)
                    .map_err(|e| e.to_string())?
                    .ok_or_else(|| "跳板机已被删除，请在主机设置里重新选择".to_string())?;
                let item = vault.decrypt_item(&row).map_err(|e| e.to_string())?;
                let jv = to_host_view(&item);
                Some(ssh::Hop {
                    credentials: credentials_for(&vault, &item.payload, pw_for(&jv.address))?,
                    known: known_keys(&vault, &jv.address, jv.port)?,
                    address: jv.address,
                    port: jv.port,
                    username: jv.username,
                })
            }
        };
        (target, jump)
    };

    // Forward terminal output to the WebView on one event channel.
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<TermEvent>();
    let emitter = app.clone();
    tokio::spawn(async move {
        while let Some(ev) = rx.recv().await {
            if let Err(e) = emitter.emit("term-event", &ev) {
                tracing::warn!("emit term-event failed: {e}");
            }
        }
    });

    let title = if host.name.is_empty() {
        format!("{}@{}", host.username, host.address)
    } else {
        host.name.clone()
    };
    let res = ssh::connect_chain(
        jump.as_ref(),
        &target,
        &host.id,
        &title,
        input.accept.as_deref(),
        cols,
        rows,
        tx,
    )
    .await?;

    // Keys the user confirmed on this attempt become trusted, whether or not
    // the rest of the chain made it: the confirmation already happened.
    if !res.accepted.is_empty() {
        let vault = state.vault.lock().map_err(|e| e.to_string())?;
        for key in &res.accepted {
            save_known_key(&vault, key)?;
        }
    }

    match res.outcome {
        ConnectOutcome::UnknownHostKey {
            fingerprint,
            key_type,
            host,
            port,
            previous,
        } => Ok(ConnectResult {
            kind: "hostKey".into(),
            fingerprint: Some(fingerprint),
            key_type: Some(key_type),
            host: Some(host),
            port: Some(port),
            previous,
            ..Default::default()
        }),
        ConnectOutcome::NeedPassword { host, username } => Ok(ConnectResult {
            kind: "needPassword".into(),
            host: Some(host),
            username: Some(username),
            ..Default::default()
        }),
        ConnectOutcome::Live(session) => {
            if input.remember_password {
                if let (Some(id), Some(pw)) = (&input.host_id, &input.password) {
                    remember_password(&state, id, pw)?;
                }
            }
            if let Some(cmd) = &host.startup_command {
                let _ = session.write(format!("{cmd}\r").as_bytes()).await;
            }
            let view = session_view(&session.info);
            let mut mgr = state.ssh.lock().map_err(|e| e.to_string())?;
            mgr.insert(session);
            Ok(ConnectResult {
                kind: "connected".into(),
                session: Some(view),
                ..Default::default()
            })
        }
    }
}

/// Saves a password typed at the prompt onto the host it was for.
fn remember_password(state: &AppState, host_id: &str, password: &str) -> Result<(), String> {
    let vault = state.vault.lock().map_err(|e| e.to_string())?;
    let Some(row) = vault.store().get_item(host_id).map_err(|e| e.to_string())? else {
        return Ok(());
    };
    let mut item = vault.decrypt_item(&row).map_err(|e| e.to_string())?;
    item.payload["password"] = serde_json::Value::String(password.to_string());
    let row = vault
        .encrypt_item(host_id, "host", &item.payload)
        .map_err(|e| e.to_string())?;
    vault.store().put_item(&row).map_err(|e| e.to_string())
}

/// Opens a shell on this machine.
#[tauri::command]
pub fn local_open(
    state: State<'_, AppState>,
    cols: Option<u16>,
    rows: Option<u16>,
) -> Result<crate::direct::DirectInfo, String> {
    let id = uuid::Uuid::new_v4().to_string();
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<ssh::TermEvent>();
    let info = {
        let mut mgr = state.direct.lock().map_err(|e| e.to_string())?;
        mgr.open_local(id, cols.unwrap_or(120), rows.unwrap_or(32), tx)?
    };
    let app = state.app.clone();
    tauri::async_runtime::spawn(async move {
        while let Some(ev) = rx.recv().await {
            if let Err(e) = app.emit("term-event", &ev) {
                tracing::warn!("emit term-event failed: {e}");
            }
        }
    });
    Ok(info)
}

/// One resource reading for a session's status bar.
#[tauri::command]
pub async fn session_stats(
    state: State<'_, AppState>,
    session_id: String,
) -> Result<crate::stats::ServerStats, String> {
    let session = {
        let mgr = state.ssh.lock().map_err(|e| e.to_string())?;
        mgr.get(&session_id)
            .ok_or_else(|| "会话不存在".to_string())?
    };
    crate::stats::sample(&session).await
}

/// Opens a saved telnet host.
async fn open_telnet_host(
    app: &tauri::AppHandle,
    host: HostView,
    cols: Option<u32>,
    rows: Option<u32>,
) -> Result<ConnectResult, String> {
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<TermEvent>();
    let emitter = app.clone();
    tokio::spawn(async move {
        while let Some(ev) = rx.recv().await {
            if let Err(e) = emitter.emit("term-event", &ev) {
                tracing::warn!("emit term-event failed: {e}");
            }
        }
    });

    let conn = crate::direct::DirectManager::connect_telnet(
        &host.address,
        host.port,
        cols.unwrap_or(120) as u16,
        rows.unwrap_or(32) as u16,
    )
    .await?;

    let id = uuid::Uuid::new_v4().to_string();
    let info = {
        let app_state = app.state::<AppState>();
        let mut mgr = app_state.direct.lock().map_err(|e| e.to_string())?;
        mgr.register_telnet(id, host.address.clone(), host.port, conn, tx)
    };

    Ok(ConnectResult {
        kind: "connected".into(),
        session: Some(SessionView {
            id: info.id,
            title: if host.name.is_empty() {
                info.target.clone()
            } else {
                host.name.clone()
            },
            status: "connected".into(),
            fingerprint: None,
        }),
        ..Default::default()
    })
}

/// Opens a saved serial host.
fn open_serial_host(app: &tauri::AppHandle, host: HostView) -> Result<ConnectResult, String> {
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<TermEvent>();
    let emitter = app.clone();
    tauri::async_runtime::spawn(async move {
        while let Some(ev) = rx.recv().await {
            if let Err(e) = emitter.emit("term-event", &ev) {
                tracing::warn!("emit term-event failed: {e}");
            }
        }
    });

    let id = uuid::Uuid::new_v4().to_string();
    let app_state = app.state::<AppState>();
    let info = {
        let mut mgr = app_state.direct.lock().map_err(|e| e.to_string())?;
        mgr.open_serial(
            id,
            crate::direct::SerialOptions {
                path: host.address.clone(),
                baud: host.baud.unwrap_or(115200),
                data_bits: host.data_bits.unwrap_or(8),
                parity: host.parity.clone().unwrap_or_else(|| "none".to_string()),
                stop_bits: host.stop_bits.unwrap_or(1),
            },
            tx,
        )?
    };

    Ok(ConnectResult {
        kind: "connected".into(),
        session: Some(SessionView {
            id: info.id,
            title: if host.name.is_empty() {
                info.target.clone()
            } else {
                host.name.clone()
            },
            status: "connected".into(),
            fingerprint: None,
        }),
        ..Default::default()
    })
}

/// Sends keyboard input to a session.
#[tauri::command]
pub async fn term_write(
    state: State<'_, AppState>,
    session_id: String,
    data: String,
) -> Result<(), String> {
    let session = {
        let mgr = state.ssh.lock().map_err(|e| e.to_string())?;
        mgr.get(&session_id)
            .ok_or_else(|| "会话不存在".to_string())?
    };
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(&data)
        .unwrap_or_else(|_| data.as_bytes().to_vec());
    session.write(&bytes).await
}

/// Returns output the shell produced before the pane attached.
///
/// The UI calls this on mount, then keeps consuming `term-event`; the two
/// overlap is harmless because the backend never re-sends what it hands out
/// The buffer is drained, so calling twice does not duplicate output.
#[tauri::command]
pub fn term_backlog(state: State<'_, AppState>, session_id: String) -> Result<String, String> {
    // SSH keeps its buffer on the session; telnet and serial keep theirs in the
    // direct manager. Try both so the UI does not need to know which it is.
    let from_ssh = {
        let mgr = state.ssh.lock().map_err(|e| e.to_string())?;
        mgr.get(&session_id).map(|s| s.take_backlog())
    };
    if let Some(bytes) = from_ssh {
        return Ok(base64::engine::general_purpose::STANDARD.encode(bytes));
    }

    let mgr = state.direct.lock().map_err(|e| e.to_string())?;
    Ok(base64::engine::general_purpose::STANDARD.encode(mgr.take_backlog(&session_id)))
}

/// Keeps the remote PTY in step with the window.
#[tauri::command]
pub async fn term_resize(
    state: State<'_, AppState>,
    session_id: String,
    cols: u32,
    rows: u32,
) -> Result<(), String> {
    let session = {
        let mgr = state.ssh.lock().map_err(|e| e.to_string())?;
        mgr.get(&session_id)
            .ok_or_else(|| "会话不存在".to_string())?
    };
    session.resize(cols, rows).await
}

#[tauri::command]
pub async fn session_close(state: State<'_, AppState>, id: String) -> Result<(), String> {
    // Stop a running recording before the session is dropped, so the file is
    // closed with its footer rather than left to the pump.
    if let Ok((_, slot)) = recorder_target(&state, &id) {
        if let Ok(mut guard) = slot.lock() {
            if let Some(mut recorder) = guard.take() {
                recorder.finish(&format!("# 结束: {}\n", crate::recorder::local_stamp()));
            }
        }
    }
    let session = {
        let mut mgr = state.ssh.lock().map_err(|e| e.to_string())?;
        mgr.remove(&id)
    };
    // Forwards and the cached SFTP channel ride on this connection; leaving
    // them registered would show dead entries and keep the handle alive.
    if let Ok(mut f) = state.forwards.lock() {
        f.stop_for_session(&id);
    }
    if let Ok(mut m) = state.sftp.lock() {
        m.remove(&id);
    }
    if let Some(s) = session {
        ssh::close(s).await;
    }
    Ok(())
}

fn session_view(s: &ssh::Session) -> SessionView {
    SessionView {
        id: s.id.clone(),
        title: s.title.clone(),
        status: s.status.clone(),
        fingerprint: s.fingerprint.clone(),
    }
}

// ---- Updates -------------------------------------------------------------

/// What a check found, in the shape the UI needs.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateInfo {
    pub current_version: String,
    pub version: String,
    pub notes: Option<String>,
    pub date: Option<String>,
}

/// Checks the configured endpoint for a newer release.
///
/// `Ok(None)` means "already current" — distinct from an error, so the UI can
/// say "已是最新版本" instead of showing a failure.
#[tauri::command]
pub async fn update_check(app: tauri::AppHandle) -> Result<Option<UpdateInfo>, String> {
    use tauri_plugin_updater::UpdaterExt;

    let updater = app.updater_builder().build().map_err(|e| e.to_string())?;
    match updater.check().await.map_err(|e| e.to_string())? {
        Some(update) => Ok(Some(UpdateInfo {
            current_version: update.current_version.clone(),
            version: update.version.clone(),
            notes: update.body.clone(),
            date: update.date.map(|d| d.to_string()),
        })),
        None => Ok(None),
    }
}

/// Downloads, verifies and hands the update to the platform installer.
///
/// Progress is emitted as `update-progress` with `{ downloaded, total }`.
/// On success the app usually restarts itself; the UI should also offer a
/// manual restart in case the platform finishes asynchronously.
#[tauri::command]
pub async fn update_install(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
) -> Result<(), String> {
    use tauri_plugin_updater::UpdaterExt;

    // One install at a time: a second click would race the same temp file.
    {
        let mut busy = state.update_busy.lock().map_err(|e| e.to_string())?;
        if *busy {
            return Err("正在更新中".into());
        }
        *busy = true;
    }

    // Release the flag on every exit path.
    let state_ref = state.inner();
    let result = async {
        let updater = app.updater_builder().build().map_err(|e| e.to_string())?;
        let update = updater
            .check()
            .await
            .map_err(|e| e.to_string())?
            .ok_or_else(|| "没有可用更新".to_string())?;

        let emitter = app.clone();
        let mut downloaded: u64 = 0;
        update
            .download_and_install(
                move |chunk: usize, total: Option<u64>| {
                    downloaded += chunk as u64;
                    let _ = emitter.emit(
                        "update-progress",
                        serde_json::json!({ "downloaded": downloaded, "total": total }),
                    );
                },
                || {},
            )
            .await
            .map_err(|e| e.to_string())
    }
    .await;

    if let Ok(mut busy) = state_ref.update_busy.lock() {
        *busy = false;
    }

    // macOS replaces the .app in place, so the running process must re-exec the
    // new binary. On Windows the NSIS installer restarts the app itself.
    #[cfg(target_os = "macos")]
    if result.is_ok() {
        app.restart();
    }

    result
}

// ---- Telnet & serial -----------------------------------------------------

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TelnetInput {
    pub host: String,
    pub port: u16,
    #[serde(default)]
    pub cols: Option<u16>,
    #[serde(default)]
    pub rows: Option<u16>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SerialInput {
    pub path: String,
    pub baud: u32,
    #[serde(default)]
    pub data_bits: Option<u8>,
    #[serde(default)]
    pub parity: Option<String>,
    #[serde(default)]
    pub stop_bits: Option<u8>,
}

/// Lists serial ports present on this machine.
#[tauri::command]
pub fn serial_ports() -> Result<Vec<crate::serial::SerialPortInfo>, String> {
    crate::serial::list_ports()
}

/// Opens a telnet connection. Output arrives on the same `term-event` channel SSH
/// uses, so the terminal component needs no special case.
#[tauri::command]
pub async fn telnet_open(
    state: State<'_, AppState>,
    input: TelnetInput,
) -> Result<crate::direct::DirectInfo, String> {
    let id = uuid::Uuid::new_v4().to_string();
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<ssh::TermEvent>();

    // Connect first, then take the lock: see DirectManager::connect_telnet.
    let conn = crate::direct::DirectManager::connect_telnet(
        &input.host,
        input.port,
        input.cols.unwrap_or(120),
        input.rows.unwrap_or(32),
    )
    .await?;

    let info = {
        let mut mgr = state.direct.lock().map_err(|e| e.to_string())?;
        mgr.register_telnet(id, input.host, input.port, conn, tx)
    };

    let app = state.app.clone();
    tokio::spawn(async move {
        while let Some(ev) = rx.recv().await {
            if let Err(e) = app.emit("term-event", &ev) {
                tracing::warn!("emit term-event failed: {e}");
            }
        }
    });

    Ok(info)
}

#[tauri::command]
pub fn serial_open(
    state: State<'_, AppState>,
    input: SerialInput,
) -> Result<crate::direct::DirectInfo, String> {
    let id = uuid::Uuid::new_v4().to_string();
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<ssh::TermEvent>();

    let info = {
        let mut mgr = state.direct.lock().map_err(|e| e.to_string())?;
        mgr.open_serial(
            id,
            crate::direct::SerialOptions {
                path: input.path,
                baud: input.baud,
                data_bits: input.data_bits.unwrap_or(8),
                parity: input.parity.unwrap_or_else(|| "none".to_string()),
                stop_bits: input.stop_bits.unwrap_or(1),
            },
            tx,
        )?
    };

    let app = state.app.clone();
    tokio::spawn(async move {
        while let Some(ev) = rx.recv().await {
            if let Err(e) = app.emit("term-event", &ev) {
                tracing::warn!("emit term-event failed: {e}");
            }
        }
    });

    Ok(info)
}

/// Writes to a telnet or serial session.
#[tauri::command]
pub async fn direct_write(
    state: State<'_, AppState>,
    session_id: String,
    data: String,
) -> Result<(), String> {
    use base64::Engine;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(&data)
        .map_err(|e| format!("数据编码错误：{e}"))?;

    let session = {
        let mgr = state.direct.lock().map_err(|e| e.to_string())?;
        mgr.get(&session_id)
            .ok_or_else(|| "会话不存在".to_string())?
    };
    session.write(&bytes).await
}

#[tauri::command]
pub async fn direct_resize(
    state: State<'_, AppState>,
    session_id: String,
    cols: u16,
    rows: u16,
) -> Result<(), String> {
    let session = {
        let mgr = state.direct.lock().map_err(|e| e.to_string())?;
        mgr.get(&session_id)
            .ok_or_else(|| "会话不存在".to_string())?
    };
    session.resize(cols, rows).await
}

#[tauri::command]
pub fn direct_close(state: State<'_, AppState>, session_id: String) -> Result<(), String> {
    if let Ok((_, slot)) = recorder_target(&state, &session_id) {
        if let Ok(mut guard) = slot.lock() {
            if let Some(mut recorder) = guard.take() {
                recorder.finish(&format!("# 结束: {}\n", crate::recorder::local_stamp()));
            }
        }
    }
    let mut mgr = state.direct.lock().map_err(|e| e.to_string())?;
    if !mgr.remove(&session_id) {
        return Err("会话不存在".into());
    }
    Ok(())
}

// ---- Terminal export & session recording ---------------------------------

/// Turns a session title into something usable as a file name stem.
fn safe_file_stem(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for ch in raw.chars() {
        if ch.is_alphanumeric() || matches!(ch, '-' | '_' | '.') {
            out.push(ch);
        } else if ch.is_whitespace()
            || matches!(ch, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|')
        {
            out.push('-');
        }
    }
    let trimmed = out.trim_matches(['-', '.'].as_slice());
    let stem: String = trimmed.chars().take(60).collect();
    if stem.is_empty() {
        "loki".to_string()
    } else {
        stem
    }
}

/// Asks the OS for a save location. Runs off the main thread: the blocking
/// dialog must not be called there.
fn pick_save_path(
    app: &tauri::AppHandle,
    default_name: &str,
) -> Result<Option<std::path::PathBuf>, String> {
    use tauri_plugin_dialog::DialogExt;

    let picked = app
        .dialog()
        .file()
        .set_file_name(default_name)
        .blocking_save_file();
    match picked {
        None => Ok(None),
        Some(fp) => fp
            .into_path()
            .map(Some)
            .map_err(|e| format!("保存路径无效：{e}")),
    }
}

/// Writes terminal text to a file the user picks.
///
/// The path comes from the native dialog and never from the WebView, so a
/// compromised renderer cannot choose where to write.
#[tauri::command]
pub async fn terminal_export(
    app: tauri::AppHandle,
    default_name: String,
    text: String,
) -> Result<Option<String>, String> {
    let app_for_dialog = app.clone();
    let name = default_name.clone();
    let picked =
        tauri::async_runtime::spawn_blocking(move || pick_save_path(&app_for_dialog, &name))
            .await
            .map_err(|e| e.to_string())??;
    let Some(path) = picked else {
        return Ok(None);
    };

    // Normalise line endings: the terminal hands us CRLF, a text file should not.
    let body = text.replace("\r\n", "\n");
    std::fs::write(&path, body).map_err(|e| format!("写入失败：{e}"))?;
    Ok(Some(path.display().to_string()))
}

/// The shared slot a session's recorder lives in.
type RecorderSlot = std::sync::Arc<std::sync::Mutex<Option<crate::recorder::Recorder>>>;

/// A session's display title plus where its recording would attach.
type RecorderTarget = (String, RecorderSlot);

/// Where a recording for this session would go.
fn recorder_target(state: &AppState, session_id: &str) -> Result<RecorderTarget, String> {
    let from_ssh = {
        let mgr = state.ssh.lock().map_err(|e| e.to_string())?;
        mgr.get(session_id)
            .map(|s| (s.info.title.clone(), s.recorder_slot()))
    };
    if let Some(hit) = from_ssh {
        return Ok(hit);
    }

    let mgr = state.direct.lock().map_err(|e| e.to_string())?;
    let title = mgr
        .info(session_id)
        .map(|i| i.target)
        .ok_or_else(|| "会话不存在".to_string())?;
    let slot = mgr
        .recorder_slot(session_id)
        .ok_or_else(|| "会话不存在".to_string())?;
    Ok((title, slot))
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecordInfo {
    pub path: String,
    pub started_at: String,
    pub bytes: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecordStopped {
    pub path: String,
    pub bytes: u64,
}

/// Starts recording a session's output to a file the user picks.
///
/// `plain` strips escape sequences; otherwise the raw byte stream (with colour)
/// is kept. Returns `None` when the user cancels the dialog.
#[tauri::command]
pub async fn session_record_start(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    session_id: String,
    plain: bool,
) -> Result<Option<RecordInfo>, String> {
    let (title, slot) = recorder_target(&state, &session_id)?;

    // One recording per session: two writers on one file would interleave.
    if slot.lock().map_err(|e| e.to_string())?.is_some() {
        return Err("这个会话已经在录制".into());
    }

    let stem = safe_file_stem(&title);
    let ext = if plain { "txt" } else { "log" };
    let default_name = format!("{stem}-{}.{ext}", crate::recorder::file_stamp());

    let app_for_dialog = app.clone();
    let picked = tauri::async_runtime::spawn_blocking(move || {
        pick_save_path(&app_for_dialog, &default_name)
    })
    .await
    .map_err(|e| e.to_string())??;
    let Some(path) = picked else {
        return Ok(None);
    };

    let mode = if plain {
        "纯文本（已去除控制字符）"
    } else {
        "原始输出（含颜色控制符）"
    };
    let header = format!(
        "# loki 会话记录\n# 目标: {title}\n# 开始: {}\n# 模式: {mode}\n\n",
        crate::recorder::local_stamp()
    );

    let recorder = crate::recorder::Recorder::start(&path, &header, plain)
        .map_err(|e| format!("无法创建文件：{e}"))?;
    let info = RecordInfo {
        path: path.display().to_string(),
        started_at: recorder.started_at().to_string(),
        bytes: recorder.bytes(),
    };

    *slot.lock().map_err(|e| e.to_string())? = Some(recorder);
    Ok(Some(info))
}

/// Stops recording and closes the file.
#[tauri::command]
pub fn session_record_stop(
    state: State<'_, AppState>,
    session_id: String,
) -> Result<Option<RecordStopped>, String> {
    let (_, slot) = recorder_target(&state, &session_id)?;
    let taken = slot.lock().map_err(|e| e.to_string())?.take();
    let Some(mut recorder) = taken else {
        return Ok(None);
    };

    let path = recorder.path().display().to_string();
    let footer = format!("# 结束: {}\n", crate::recorder::local_stamp());
    let bytes = recorder.finish(&footer);
    Ok(Some(RecordStopped { path, bytes }))
}

/// Current recording for a session, if any.
#[tauri::command]
pub fn session_record_status(
    state: State<'_, AppState>,
    session_id: String,
) -> Result<Option<RecordInfo>, String> {
    let Ok((_, slot)) = recorder_target(&state, &session_id) else {
        return Ok(None);
    };
    let guard = slot.lock().map_err(|e| e.to_string())?;
    Ok(guard.as_ref().map(|r| RecordInfo {
        path: r.path().display().to_string(),
        started_at: r.started_at().to_string(),
        bytes: r.bytes(),
    }))
}

/// Finishes every open recording. Used when sessions go away wholesale.
fn finish_all_recordings(state: &AppState) {
    let mut slots = Vec::new();
    if let Ok(mgr) = state.ssh.lock() {
        for session in mgr.list() {
            if let Some(live) = mgr.get(&session.id) {
                slots.push(live.recorder_slot());
            }
        }
    }
    if let Ok(mgr) = state.direct.lock() {
        slots.extend(mgr.recorder_slots());
    }

    for slot in slots {
        if let Ok(mut guard) = slot.lock() {
            if let Some(mut recorder) = guard.take() {
                recorder.finish(&format!("# 结束: {}\n", crate::recorder::local_stamp()));
            }
        }
    }
}

// ---- AI conversations ----------------------------------------------------

/// A saved chat, without its turns: what the history list shows.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConversationSummary {
    pub id: String,
    pub title: String,
    pub host_label: String,
    pub updated_at: i64,
    pub turn_count: usize,
}

/// Conversations need the vault open: the payload holds server output.
fn require_unlocked(vault: &crate::vault::Vault) -> Result<(), String> {
    if vault.status().unlocked {
        Ok(())
    } else {
        Err("保险库已锁定".into())
    }
}

/// Every stored conversation, decrypted, newest first.
fn all_conversations(
    vault: &crate::vault::Vault,
) -> Result<Vec<(crate::store::ItemRow, serde_json::Value)>, String> {
    let rows = vault.store().list_items().map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    for row in rows.into_iter().filter(|r| r.kind == "ai_conversation") {
        // A row that will not decrypt (older key, corruption) is skipped rather
        // than breaking the whole list.
        if let Ok(item) = vault.decrypt_item(&row) {
            out.push((row, item.payload));
        }
    }
    Ok(out)
}

/// Lists conversation summaries, optionally only those of one host.
#[tauri::command]
pub fn ai_conversations_list(
    state: State<'_, AppState>,
    host_id: Option<String>,
) -> Result<Vec<ConversationSummary>, String> {
    let vault = state.vault.lock().map_err(|e| e.to_string())?;
    require_unlocked(&vault)?;

    let mut out: Vec<ConversationSummary> = all_conversations(&vault)?
        .into_iter()
        .filter(|(_, payload)| match &host_id {
            Some(want) => payload.get("hostId").and_then(|v| v.as_str()) == Some(want.as_str()),
            None => true,
        })
        .map(|(row, payload)| ConversationSummary {
            id: row.id,
            title: payload
                .get("title")
                .and_then(|v| v.as_str())
                .unwrap_or("未命名对话")
                .to_string(),
            host_label: payload
                .get("hostLabel")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
            updated_at: row.updated_at,
            turn_count: payload
                .get("turns")
                .and_then(|v| v.as_array())
                .map(|a| a.len())
                .unwrap_or(0),
        })
        .collect();

    out.sort_by_key(|c| std::cmp::Reverse(c.updated_at));
    Ok(out)
}

/// The full conversation, or None when it does not exist.
#[tauri::command]
pub fn ai_conversation_get(
    state: State<'_, AppState>,
    id: String,
) -> Result<Option<serde_json::Value>, String> {
    let vault = state.vault.lock().map_err(|e| e.to_string())?;
    require_unlocked(&vault)?;

    let Some(row) = vault.store().get_item(&id).map_err(|e| e.to_string())? else {
        return Ok(None);
    };
    if row.kind != "ai_conversation" {
        return Ok(None);
    }
    let item = vault.decrypt_item(&row).map_err(|e| e.to_string())?;
    Ok(Some(item.payload))
}

/// Drops the oldest conversations once there are too many.
fn prune_conversations(vault: &crate::vault::Vault) -> Result<(), String> {
    let mut rows: Vec<(crate::store::ItemRow, serde_json::Value)> = all_conversations(vault)?;
    let limit = crate::ai::conversation::MAX_CONVERSATIONS;
    if rows.len() <= limit {
        return Ok(());
    }
    // Oldest first, and remove the surplus from that end.
    rows.sort_by_key(|r| r.0.updated_at);
    let surplus = rows.len() - limit;
    for (row, _) in rows.into_iter().take(surplus) {
        vault
            .store()
            .delete_item(&row.id)
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// Inserts or replaces a conversation. Limits are applied here so a caller
/// cannot store an oversized payload.
#[tauri::command]
pub fn ai_conversation_save(
    state: State<'_, AppState>,
    conversation: serde_json::Value,
) -> Result<(), String> {
    let vault = state.vault.lock().map_err(|e| e.to_string())?;
    require_unlocked(&vault)?;

    let id = conversation
        .get("id")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .ok_or("缺少对话 id")?
        .to_string();

    let mut payload = crate::ai::conversation::truncate(conversation);
    if let Some(obj) = payload.as_object_mut() {
        obj.insert("id".into(), serde_json::Value::String(id.clone()));
        obj.insert("updatedAt".into(), serde_json::Value::from(now_secs()));
    }

    let row = vault
        .encrypt_item(&id, "ai_conversation", &payload)
        .map_err(|e| e.to_string())?;
    vault.store().put_item(&row).map_err(|e| e.to_string())?;
    prune_conversations(&vault)
}

/// Renames a conversation, which also moves it to the top of the list.
#[tauri::command]
pub fn ai_conversation_rename(
    state: State<'_, AppState>,
    id: String,
    title: String,
) -> Result<(), String> {
    let title = title.trim();
    if title.is_empty() {
        return Err("标题不能为空".into());
    }
    let vault = state.vault.lock().map_err(|e| e.to_string())?;
    require_unlocked(&vault)?;

    let Some(row) = vault.store().get_item(&id).map_err(|e| e.to_string())? else {
        return Err("对话不存在".into());
    };
    if row.kind != "ai_conversation" {
        return Err("对话不存在".into());
    }
    let mut payload = vault.decrypt_item(&row).map_err(|e| e.to_string())?.payload;
    if let Some(obj) = payload.as_object_mut() {
        obj.insert("title".into(), serde_json::Value::String(title.to_string()));
        obj.insert("updatedAt".into(), serde_json::Value::from(now_secs()));
    }
    let updated = vault
        .encrypt_item(&id, "ai_conversation", &payload)
        .map_err(|e| e.to_string())?;
    vault.store().put_item(&updated).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn ai_conversation_delete(state: State<'_, AppState>, id: String) -> Result<(), String> {
    let vault = state.vault.lock().map_err(|e| e.to_string())?;
    require_unlocked(&vault)?;
    // Only ever delete a conversation: an id from the list must not be able to
    // remove a host or a key by mistake.
    if let Some(row) = vault.store().get_item(&id).map_err(|e| e.to_string())? {
        if row.kind != "ai_conversation" {
            return Err("不是对话记录".into());
        }
    }
    vault.store().delete_item(&id).map_err(|e| e.to_string())
}

// ---- Sync ----------------------------------------------------------------

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncConnectInput {
    pub url: String,
    pub token: String,
}

/// Checks the Worker and remembers the address and token.
#[tauri::command]
pub async fn sync_connect(
    state: State<'_, AppState>,
    input: SyncConnectInput,
) -> Result<String, String> {
    let health = crate::sync::connect(&state, &input.url, &input.token)
        .await
        .map_err(|e| e.to_string())?;
    Ok(if health.initialized {
        "已连接，远端已有保险库".to_string()
    } else {
        "已连接，远端还没有保险库".to_string()
    })
}

#[tauri::command]
pub fn sync_status(state: State<'_, AppState>) -> Result<Option<serde_json::Value>, String> {
    let vault = state.vault.lock().map_err(|e| e.to_string())?;
    let url = vault
        .store()
        .get_meta("sync_url")
        .map_err(|e| e.to_string())?;
    let cursor = vault
        .store()
        .get_meta("sync_cursor")
        .map_err(|e| e.to_string())?;
    Ok(url.map(|u| serde_json::json!({ "url": u, "cursor": cursor.and_then(|c| c.parse::<u64>().ok()).unwrap_or(0) })))
}

#[tauri::command]
pub async fn sync_run(state: State<'_, AppState>) -> Result<crate::sync::SyncReport, String> {
    crate::sync::run_once(&state)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn sync_disconnect(state: State<'_, AppState>) -> Result<(), String> {
    let vault = state.vault.lock().map_err(|e| e.to_string())?;
    vault
        .store()
        .set_meta("sync_url", "")
        .map_err(|e| e.to_string())?;
    vault
        .store()
        .set_meta("sync_token", "")
        .map_err(|e| e.to_string())?;
    Ok(())
}

// ---- AI assistant --------------------------------------------------------

/// A provider as the UI sees it: the API key is never included.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderView {
    pub id: String,
    pub name: String,
    pub protocol: String,
    pub base_url: String,
    /// Every model the user added, in their order.
    pub models: Vec<String>,
    pub has_key: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderInput {
    pub id: Option<String>,
    pub name: String,
    pub protocol: String,
    pub base_url: String,
    #[serde(default)]
    pub models: Vec<String>,
    /// Blank keeps the stored key.
    pub api_key: Option<String>,
}

fn provider_from_payload(p: &serde_json::Value) -> ProviderView {
    ProviderView {
        id: p
            .get("id")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        name: p
            .get("name")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        protocol: p
            .get("protocol")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        base_url: crate::ai::provider::stored_base_url(p),
        models: {
            // `model` is the 0.7 shape: a single string. Fold it into the list
            // so an upgraded install keeps its provider usable.
            let list: Vec<String> = p
                .get("models")
                .and_then(|v| v.as_array())
                .map(|a| {
                    a.iter()
                        .filter_map(|m| m.as_str().map(String::from))
                        .collect()
                })
                .unwrap_or_default();
            if list.is_empty() {
                p.get("model")
                    .and_then(|v| v.as_str())
                    .filter(|m| !m.is_empty())
                    .map(|m| vec![m.to_string()])
                    .unwrap_or_default()
            } else {
                list
            }
        },
        has_key: p
            .get("api_key")
            .and_then(|v| v.as_str())
            .map(|k| !k.is_empty())
            .unwrap_or(false),
    }
}

#[tauri::command]
pub fn ai_providers_list(state: State<'_, AppState>) -> Result<Vec<ProviderView>, String> {
    let vault = state.vault.lock().map_err(|e| e.to_string())?;
    let rows = vault.store().list_items().map_err(|e| e.to_string())?;

    let mut out = Vec::new();
    for row in rows.iter().filter(|r| r.kind == "provider") {
        let item = vault.decrypt_item(row).map_err(|e| e.to_string())?;
        out.push(provider_from_payload(&item.payload));
    }
    Ok(out)
}

#[tauri::command]
pub fn ai_provider_save(
    state: State<'_, AppState>,
    input: ProviderInput,
) -> Result<ProviderView, String> {
    let vault = state.vault.lock().map_err(|e| e.to_string())?;
    let id = input
        .id
        .clone()
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());

    // Reject a malformed address here rather than at request time, where the
    // error would surface as a confusing network failure.
    let base_url =
        crate::ai::provider::validate_base_url(&input.base_url).map_err(|e| e.to_string())?;

    // A blank key on an edit keeps the stored one, so the UI never has to hold
    // the secret just to rename a provider — but only while the address still
    // points at the same server.
    let api_key = match input.api_key.as_ref().filter(|k| !k.is_empty()) {
        Some(k) => k.clone(),
        None => match &input.id {
            Some(id) => reusable_key(&vault, id, &base_url)?,
            None => String::new(),
        },
    };

    // Trim, drop blanks, de-duplicate while keeping the user's order.
    let mut models: Vec<String> = Vec::new();
    for m in &input.models {
        let m = m.trim();
        if !m.is_empty() && !models.iter().any(|x| x == m) {
            models.push(m.to_string());
        }
    }

    let payload = serde_json::json!({
        "id": id,
        "name": input.name,
        "protocol": input.protocol,
        "base_url": base_url,
        "models": models,
        "api_key": api_key,
    });

    let row = vault
        .encrypt_item(&id, "provider", &payload)
        .map_err(|e| e.to_string())?;
    vault.store().put_item(&row).map_err(|e| e.to_string())?;
    Ok(provider_from_payload(&payload))
}

#[tauri::command]
pub fn ai_provider_delete(state: State<'_, AppState>, id: String) -> Result<(), String> {
    let vault = state.vault.lock().map_err(|e| e.to_string())?;
    vault.store().delete_item(&id).map_err(|e| e.to_string())
}

/// The stored API key of provider `id`, if `new_base_url` is on the same
/// server (scheme, host, port) as the stored address; otherwise empty.
///
/// Without this check, editing a provider's address to another host and
/// clicking "test connection" would send the old provider's key to the new one.
fn reusable_key(
    vault: &crate::vault::Vault,
    id: &str,
    new_base_url: &str,
) -> Result<String, String> {
    let Some(row) = vault.store().get_item(id).map_err(|e| e.to_string())? else {
        return Ok(String::new());
    };
    let item = vault.decrypt_item(&row).map_err(|e| e.to_string())?;
    let stored_url = crate::ai::provider::stored_base_url(&item.payload);
    if !crate::ai::provider::same_origin(&stored_url, new_base_url) {
        return Ok(String::new());
    }
    Ok(item
        .payload
        .get("api_key")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string())
}

/// Reads one provider, key included, for the turn engine. Not exposed to the UI.
fn load_provider(
    state: &AppState,
    provider_id: &str,
) -> Result<crate::ai::provider::ProviderConfig, String> {
    let vault = state.vault.lock().map_err(|e| e.to_string())?;
    let row = vault
        .store()
        .get_item(provider_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "模型服务商不存在".to_string())?;
    let item = vault.decrypt_item(&row).map_err(|e| e.to_string())?;
    let f = |name: &str| {
        item.payload
            .get(name)
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string()
    };
    let protocol = crate::ai::provider::Protocol::parse(&f("protocol"));
    let models: Vec<String> = item
        .payload
        .get("models")
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|m| m.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();
    let models = if models.is_empty() {
        item.payload
            .get("model")
            .and_then(|v| v.as_str())
            .filter(|m| !m.is_empty())
            .map(|m| vec![m.to_string()])
            .unwrap_or_default()
    } else {
        models
    };

    Ok(crate::ai::provider::ProviderConfig {
        id: f("id"),
        name: f("name"),
        protocol,
        base_url: crate::ai::provider::stored_base_url(&item.payload),
        model: models.first().cloned().unwrap_or_default(),
        models,
        api_key: f("api_key"),
    })
}

/// Builds a runtime config from a dialog's draft.
///
/// The dialog can ask for the model list or test the connection *before* saving,
/// so a blank key means "use the one already stored for this id" — as long as
/// the draft still points at the server that key was saved for.
fn config_from_input(
    state: &AppState,
    input: &ProviderInput,
) -> Result<crate::ai::provider::ProviderConfig, String> {
    let base_url =
        crate::ai::provider::validate_base_url(&input.base_url).map_err(|e| e.to_string())?;

    let api_key = match input.api_key.as_ref().filter(|k| !k.is_empty()) {
        Some(k) => k.clone(),
        None => match &input.id {
            Some(id) => {
                let vault = state.vault.lock().map_err(|e| e.to_string())?;
                reusable_key(&vault, id, &base_url)?
            }
            None => String::new(),
        },
    };

    Ok(crate::ai::provider::ProviderConfig {
        id: input.id.clone().unwrap_or_default(),
        name: input.name.clone(),
        protocol: crate::ai::provider::Protocol::parse(&input.protocol),
        base_url,
        model: input.models.first().cloned().unwrap_or_default(),
        models: input.models.clone(),
        api_key,
    })
}

/// Fetches the provider's model list, without saving anything.
#[tauri::command]
pub async fn ai_provider_models(
    state: State<'_, AppState>,
    input: ProviderInput,
) -> Result<Vec<String>, String> {
    let config = config_from_input(&state, &input)?;
    crate::ai::provider::list_models(&config)
        .await
        .map_err(|e| e.to_string())
}

/// Proves the URL and key work, returning a human-readable verdict.
#[tauri::command]
pub async fn ai_provider_test(
    state: State<'_, AppState>,
    input: ProviderInput,
) -> Result<String, String> {
    let config = config_from_input(&state, &input)?;
    crate::ai::provider::test_connection(&config)
        .await
        .map_err(|e| e.to_string())
}

/// Starts a turn. Events stream on `ai-event`; gated calls wait for
/// [`ai_approve`].
/// One attachment the panel wants this message to carry.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AiAttachSend {
    pub id: String,
    pub share_content: bool,
}

// Eight parameters is the price of a flat IPC call; grouping them into a struct
// would only move the same fields into a wrapper.
#[allow(clippy::too_many_arguments)]
#[tauri::command]
pub async fn ai_send(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    provider_id: String,
    model: Option<String>,
    session_id: Option<String>,
    mode: String,
    history: Vec<serde_json::Value>,
    attachments: Option<Vec<AiAttachSend>>,
) -> Result<(), String> {
    let mut config = load_provider(&state, &provider_id)?;

    // The panel picks one of the provider's models; fall back to the first.
    if let Some(chosen) = model.filter(|m| !m.trim().is_empty()) {
        if config.models.iter().any(|m| m == &chosen) {
            config.model = chosen;
        }
    }
    if config.model.trim().is_empty() {
        return Err("这个服务商还没有添加模型".into());
    }

    let permission = match mode.as_str() {
        "auto" => crate::ai::tools::PermissionMode::Auto,
        "bypass" => crate::ai::tools::PermissionMode::Bypass,
        _ => crate::ai::tools::PermissionMode::Manual,
    };

    let session = match session_id {
        Some(id) => {
            let mgr = state.ssh.lock().map_err(|e| e.to_string())?;
            mgr.get(&id)
        }
        None => None,
    };

    // The most recent terminal output, so `read_terminal` has something to read.
    let terminal_tail = session
        .as_ref()
        .map(|s| String::from_utf8_lossy(&s.peek_backlog()).to_string())
        .unwrap_or_default();

    // Attachments are described here, not by the panel: the content must never
    // pass through the WebView. Marking them `sent` also makes detach keep the
    // uploaded copy, since the model has already been told the path.
    let (block, secret_paths) = match attachments.filter(|list| !list.is_empty()) {
        None => (String::new(), Vec::new()),
        Some(list) => {
            let mut rows = Vec::new();
            let mut secrets = Vec::new();
            {
                let mut reg = state.attachments.lock().map_err(|e| e.to_string())?;
                for want in &list {
                    let Some(item) = reg.items.get_mut(&want.id) else {
                        continue;
                    };
                    item.sent = true;
                    if item.secret {
                        secrets.push(item.remote_path.clone());
                    }
                    rows.push((
                        item.name.clone(),
                        item.size,
                        item.sha256.clone(),
                        item.secret,
                        item.remote_path.clone(),
                        if item.secret {
                            None
                        } else if want.share_content {
                            item.content.clone()
                        } else {
                            None
                        },
                    ));
                }
            }
            let descs: Vec<crate::ai::attach::AttachDesc> = rows
                .iter()
                .map(|r| crate::ai::attach::AttachDesc {
                    name: &r.0,
                    size: r.1,
                    sha256: &r.2,
                    secret: r.3,
                    remote_path: &r.4,
                    content: r.5.as_deref(),
                })
                .collect();
            (crate::ai::attach::describe(&descs), secrets)
        }
    };

    let mut history = history;
    if !block.is_empty() {
        if let Some(last) = history
            .iter_mut()
            .rev()
            .find(|m| m.get("role").and_then(|r| r.as_str()) == Some("user"))
        {
            let current = last
                .get("content")
                .and_then(|c| c.as_str())
                .unwrap_or("")
                .to_string();
            if let Some(obj) = last.as_object_mut() {
                obj.insert(
                    "content".into(),
                    serde_json::Value::String(format!("{current}\n\n{block}")),
                );
            }
        }
    }

    let history: Vec<crate::ai::provider::ChatMessage> = history
        .iter()
        .filter_map(|v| serde_json::from_value(v.clone()).ok())
        .collect();

    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<crate::ai::AiEvent>();
    let (approval_tx, approval_rx) = tokio::sync::mpsc::unbounded_channel::<crate::ai::Approval>();

    {
        let mut slot = state.ai_approvals.lock().map_err(|e| e.to_string())?;
        *slot = Some(approval_tx);
    }

    let emitter = app.clone();
    tokio::spawn(async move {
        while let Some(ev) = rx.recv().await {
            if let Err(e) = emitter.emit("ai-event", &ev) {
                tracing::warn!("emit ai-event failed: {e}");
            }
        }
    });

    crate::ai::run_turn(
        crate::ai::TurnRequest {
            config,
            history,
            mode: permission,
            session,
            terminal_tail,
            secret_paths,
        },
        tx,
        approval_rx,
    )
    .await;

    Ok(())
}

/// Answers a gated tool call.
#[tauri::command]
pub fn ai_approve(
    state: State<'_, AppState>,
    call_id: String,
    approved: bool,
) -> Result<(), String> {
    let slot = state.ai_approvals.lock().map_err(|e| e.to_string())?;
    match slot.as_ref() {
        Some(tx) => tx
            .send(crate::ai::Approval { call_id, approved })
            .map_err(|_| "对话已结束".to_string()),
        None => Err("当前没有进行中的对话".into()),
    }
}

// ---- AI attachments ------------------------------------------------------

/// One file the user attached, and what became of it.
#[derive(Debug, Clone)]
pub struct Attachment {
    pub id: String,
    pub session_id: String,
    pub name: String,
    pub size: u64,
    pub sha256: String,
    pub kind: String,
    pub secret: bool,
    pub remote_path: String,
    pub batch_dir: String,
    /// Cached only for shareable files; never set for a private key.
    pub content: Option<String>,
    /// Set once the attachment has been described to the model.
    pub sent: bool,
}

/// Attachments of this run, plus the staging folders to clean up later.
#[derive(Default)]
pub struct AttachRegistry {
    pub items: std::collections::HashMap<String, Attachment>,
    /// Staging directory per session, so "clean up" can remove a whole batch.
    pub batches: std::collections::HashMap<String, Vec<String>>,
}

impl AttachRegistry {
    pub fn clear(&mut self) {
        self.items.clear();
        self.batches.clear();
    }
}

/// What the panel learns about an attachment: no content and no local path.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AttachmentView {
    pub id: String,
    pub name: String,
    pub size: u64,
    pub sha256: String,
    pub kind: String,
    pub secret: bool,
    pub remote_path: String,
    pub verified: bool,
    pub shareable: bool,
}

/// Uploads local files to a fresh staging folder and returns what the panel
/// shows. The bytes never come back to the WebView.
#[tauri::command]
pub async fn ai_attach(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    session_id: String,
    paths: Vec<String>,
) -> Result<Vec<AttachmentView>, String> {
    use crate::ai::attach;

    if paths.is_empty() {
        return Err("没有选择文件".into());
    }
    if paths.len() > attach::MAX_BATCH {
        return Err(format!(
            "一次最多附加 {} 个文件，这次选了 {} 个",
            attach::MAX_BATCH,
            paths.len()
        ));
    }

    let session = {
        let mgr = state.ssh.lock().map_err(|e| e.to_string())?;
        mgr.get(&session_id)
            .ok_or_else(|| "会话不存在".to_string())?
    };
    let (sftp, home) = sftp_for(&state, &session_id).await?;

    // One staging folder per attach action.
    let now = chrono::Local::now();
    let rand = (uuid::Uuid::new_v4().as_u128() & 0xffff) as u16;
    let batch_name = attach::staging_name(now.timestamp(), rand);
    let dir = {
        let guard = sftp.lock().await;
        crate::ai::staging::prepare_batch(&guard, &home, &batch_name).await?
    };

    let mut views = Vec::with_capacity(paths.len());

    for raw in &paths {
        let path = std::path::Path::new(raw);
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .filter(|n| !n.is_empty() && !n.contains('/'))
            .ok_or_else(|| format!("无效的文件名：{raw}"))?
            .to_string();

        let meta = std::fs::metadata(path).map_err(|e| format!("读取 {name} 失败：{e}"))?;
        if !meta.is_file() {
            return Err(format!("{name} 不是普通文件"));
        }
        let size = meta.len();
        if size as usize > attach::MAX_FILE {
            return Err(format!(
                "{} 有 {}，超过单个文件上限 {}",
                name,
                attach::human_size(size),
                attach::human_size(attach::MAX_FILE as u64)
            ));
        }

        let bytes = std::fs::read(path).map_err(|e| format!("读取 {name} 失败：{e}"))?;
        let sha256 = attach::sha256_hex(&bytes);
        let kind = attach::sniff_kind(&bytes).to_string();
        let secret = attach::is_secret(&name, &bytes);
        let shareable = kind == "text" && !secret && (size as usize) <= attach::SHARE_LIMIT;

        let id = uuid::Uuid::new_v4().to_string();
        let _ = app.emit(
            "attach-progress",
            serde_json::json!({ "id": id, "name": name, "percent": 0 }),
        );

        let (remote_path, verified) = {
            let guard = sftp.lock().await;
            crate::ai::staging::upload_file(&session, &guard, &dir, &name, &bytes).await?
        };

        let _ = app.emit(
            "attach-progress",
            serde_json::json!({ "id": id, "name": name, "percent": 60 }),
        );

        let _ = app.emit(
            "attach-progress",
            serde_json::json!({ "id": id, "name": name, "percent": 100 }),
        );

        let content = if shareable {
            Some(String::from_utf8_lossy(&bytes).to_string())
        } else {
            None
        };

        let record = Attachment {
            id: id.clone(),
            session_id: session_id.clone(),
            name: name.clone(),
            size,
            sha256: sha256.clone(),
            kind: kind.clone(),
            secret,
            remote_path: remote_path.clone(),
            batch_dir: dir.clone(),
            content: content.clone(),
            sent: false,
        };

        {
            let mut reg = state.attachments.lock().map_err(|e| e.to_string())?;
            reg.batches
                .entry(session_id.clone())
                .or_default()
                .push(record.batch_dir.clone());
            reg.items.insert(id.clone(), record);
        }

        views.push(AttachmentView {
            id,
            name,
            size,
            sha256,
            kind,
            secret,
            remote_path,
            verified,
            shareable,
        });
    }

    Ok(views)
}

/// Forgets an attachment, deleting the uploaded copy if it was never used.
#[tauri::command]
pub async fn ai_detach(state: State<'_, AppState>, id: String) -> Result<(), String> {
    let record = {
        let mut reg = state.attachments.lock().map_err(|e| e.to_string())?;
        reg.items.remove(&id)
    };
    let Some(record) = record else {
        return Ok(());
    };
    if record.sent {
        // The model may have been told about the path; leave it alone.
        return Ok(());
    }

    let sftp = {
        let map = state.sftp.lock().map_err(|e| e.to_string())?;
        map.get(&record.session_id).cloned()
    };
    if let Some(sftp) = sftp {
        let guard = sftp.lock().await;
        let _ = guard.remove_file(&record.remote_path).await;
    }
    Ok(())
}

/// Removes every staging folder this run created for a session.
#[tauri::command]
pub async fn ai_attachments_cleanup(
    state: State<'_, AppState>,
    session_id: String,
) -> Result<u64, String> {
    let (dirs, ids) = {
        let mut reg = state.attachments.lock().map_err(|e| e.to_string())?;
        let dirs = reg.batches.remove(&session_id).unwrap_or_default();
        let ids: Vec<String> = reg
            .items
            .values()
            .filter(|a| a.session_id == session_id)
            .map(|a| a.id.clone())
            .collect();
        for id in &ids {
            reg.items.remove(id);
        }
        (dirs, ids)
    };
    let _ = ids;

    if dirs.is_empty() {
        return Ok(0);
    }
    let sftp = {
        let map = state.sftp.lock().map_err(|e| e.to_string())?;
        map.get(&session_id).cloned()
    };
    let Some(sftp) = sftp else {
        return Ok(0);
    };
    let guard = sftp.lock().await;
    let mut removed = 0u64;
    for dir in dirs {
        removed += crate::ai::staging::remove_tree(&guard, &dir).await;
    }
    Ok(removed)
}

// ---- Port forwarding -----------------------------------------------------

#[tauri::command]
pub async fn forward_start(
    state: State<'_, AppState>,
    session_id: String,
    spec: crate::forward::ForwardSpec,
) -> Result<crate::forward::ForwardView, String> {
    let session = {
        let mgr = state.ssh.lock().map_err(|e| e.to_string())?;
        mgr.get(&session_id)
            .ok_or_else(|| "会话不存在".to_string())?
    };

    let (view, stop) = crate::forward::start(session, spec).await?;
    let mut mgr = state.forwards.lock().map_err(|e| e.to_string())?;
    mgr.insert(crate::forward::Running {
        view: view.clone(),
        stop,
    });
    Ok(view)
}

#[tauri::command]
pub fn forward_list(
    state: State<'_, AppState>,
) -> Result<Vec<crate::forward::ForwardView>, String> {
    let mgr = state.forwards.lock().map_err(|e| e.to_string())?;
    Ok(mgr.list())
}

#[tauri::command]
pub fn forward_stop(state: State<'_, AppState>, id: String) -> Result<(), String> {
    let mut mgr = state.forwards.lock().map_err(|e| e.to_string())?;
    if !mgr.remove(&id) {
        return Err("转发不存在".into());
    }
    Ok(())
}

// ---- SFTP ----------------------------------------------------------------

/// Opens (or reuses) the SFTP channel for a session and hands back its home dir.
async fn sftp_for(
    state: &AppState,
    session_id: &str,
) -> Result<
    (
        std::sync::Arc<tokio::sync::Mutex<russh_sftp::client::SftpSession>>,
        String,
    ),
    String,
> {
    let cached = {
        let map = state.sftp.lock().map_err(|e| e.to_string())?;
        map.get(session_id).cloned()
    };
    if let Some(sftp) = cached {
        let guard = sftp.lock().await;
        let home = crate::sftp::home_dir(&guard).await?;
        return Ok((sftp.clone(), home));
    }

    let session = {
        let mgr = state.ssh.lock().map_err(|e| e.to_string())?;
        mgr.get(session_id)
            .ok_or_else(|| "会话不存在".to_string())?
    };

    let sftp = crate::sftp::open_sftp(&session.handle).await?;
    let home = crate::sftp::home_dir(&sftp).await?;
    let shared = std::sync::Arc::new(tokio::sync::Mutex::new(sftp));

    let mut map = state.sftp.lock().map_err(|e| e.to_string())?;
    map.insert(session_id.to_string(), shared.clone());
    Ok((shared, home))
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DirListing {
    pub path: String,
    pub home: String,
    pub entries: Vec<crate::sftp::FileEntry>,
}

/// Lists a directory. `path` may be empty to start at the home directory.
#[tauri::command]
pub async fn sftp_list(
    state: State<'_, AppState>,
    session_id: String,
    path: String,
) -> Result<DirListing, String> {
    let (sftp, home) = sftp_for(&state, &session_id).await?;
    let target = if path.is_empty() { home.clone() } else { path };

    let guard = sftp.lock().await;
    let entries = crate::sftp::list_dir(&guard, &target).await?;
    Ok(DirListing {
        path: target,
        home,
        entries,
    })
}

/// Reads a file as UTF-8 text, for the built-in viewer/editor.
#[tauri::command]
pub async fn sftp_read(
    state: State<'_, AppState>,
    session_id: String,
    path: String,
) -> Result<String, String> {
    let (sftp, _) = sftp_for(&state, &session_id).await?;
    let guard = sftp.lock().await;
    let bytes = crate::sftp::read_file(&guard, &path).await?;
    String::from_utf8(bytes).map_err(|_| "这不是文本文件，无法在编辑器里打开".to_string())
}

#[tauri::command]
pub async fn sftp_write(
    state: State<'_, AppState>,
    session_id: String,
    path: String,
    content: String,
) -> Result<(), String> {
    let (sftp, _) = sftp_for(&state, &session_id).await?;
    let guard = sftp.lock().await;
    crate::sftp::write_file(&guard, &path, content.as_bytes()).await
}

#[tauri::command]
pub async fn sftp_mkdir(
    state: State<'_, AppState>,
    session_id: String,
    path: String,
) -> Result<(), String> {
    let (sftp, _) = sftp_for(&state, &session_id).await?;
    let guard = sftp.lock().await;
    crate::sftp::mkdir(&guard, &path).await
}

#[tauri::command]
pub async fn sftp_delete(
    state: State<'_, AppState>,
    session_id: String,
    path: String,
    is_dir: bool,
) -> Result<(), String> {
    let (sftp, _) = sftp_for(&state, &session_id).await?;
    let guard = sftp.lock().await;
    if is_dir {
        crate::transfer::remove_recursive(&guard, &path).await
    } else {
        crate::sftp::remove(&guard, &path, false).await
    }
}

#[tauri::command]
pub async fn sftp_rename(
    state: State<'_, AppState>,
    session_id: String,
    from: String,
    to: String,
) -> Result<(), String> {
    let (sftp, _) = sftp_for(&state, &session_id).await?;
    let guard = sftp.lock().await;
    crate::sftp::rename(&guard, &from, &to).await
}

// ---- Keys ----------------------------------------------------------------

/// A stored key as the UI sees it. The private half never crosses this boundary.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KeyView {
    pub id: String,
    pub name: String,
    /// `ed25519` or `rsa4096`.
    pub kind: String,
    /// `SHA256:...` of the public key, as OpenSSH prints it.
    pub fingerprint: String,
    /// The public key in authorized_keys form, safe to display and copy.
    pub public_key: String,
    pub created_at: i64,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KeyGenInput {
    pub name: String,
    /// `ed25519` (default) or `rsa4096`.
    pub kind: String,
    pub comment: String,
}

/// Lists stored keys. Only public material is returned.
#[tauri::command]
pub fn keys_list(state: State<'_, AppState>) -> Result<Vec<KeyView>, String> {
    let vault = state.vault.lock().map_err(|e| e.to_string())?;
    let rows = vault.store().list_items().map_err(|e| e.to_string())?;

    let mut out = Vec::new();
    for row in rows.iter().filter(|r| r.kind == "key") {
        let item = vault.decrypt_item(row).map_err(|e| e.to_string())?;
        out.push(key_view(&item, row.updated_at));
    }
    Ok(out)
}

/// Generates a key pair. The private half is stored encrypted and never returned.
#[tauri::command]
pub fn key_generate(state: State<'_, AppState>, input: KeyGenInput) -> Result<KeyView, String> {
    use russh::keys::ssh_key::{Algorithm, LineEnding, PrivateKey};

    let algorithm = match input.kind.as_str() {
        "ed25519" => Algorithm::Ed25519,
        // 4096-bit RSA is slow to generate but what older servers still expect.
        "rsa4096" => Algorithm::Rsa { hash: None },
        other => return Err(format!("不支持的密钥类型：{other}")),
    };

    let mut rng = rand::rngs::OsRng;
    let key = PrivateKey::random(&mut rng, algorithm).map_err(|e| format!("生成密钥失败：{e}"))?;

    let mut public = key.public_key().clone();
    if !input.comment.is_empty() {
        public.set_comment(input.comment.clone());
    }
    let public_line = public
        .to_openssh()
        .map_err(|e| format!("序列化公钥失败：{e}"))?;
    let private_pem = key
        .to_openssh(LineEnding::LF)
        .map_err(|e| format!("序列化私钥失败：{e}"))?;
    let fp = public.fingerprint(russh::keys::HashAlg::Sha256).to_string();

    let vault = state.vault.lock().map_err(|e| e.to_string())?;
    let id = uuid::Uuid::new_v4().to_string();
    let payload = serde_json::json!({
        "name": input.name,
        "kind": input.kind,
        "fingerprint": fp,
        "public_key": public_line,
        // Stored inside the same envelope as everything else.
        "private_key": private_pem.as_str(),
    });

    let row = vault
        .encrypt_item(&id, "key", &payload)
        .map_err(|e| e.to_string())?;
    vault.store().put_item(&row).map_err(|e| e.to_string())?;

    Ok(KeyView {
        id,
        name: input.name,
        kind: input.kind,
        fingerprint: fp,
        public_key: public_line,
        created_at: row.updated_at,
    })
}

/// Imports an existing private key. A passphrase-protected key is decrypted
/// once here and stored unencrypted inside the vault envelope, which is itself
/// encrypted; asking for the passphrase on every connect would add nothing.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KeyImportInput {
    pub name: String,
    #[serde(default)]
    pub private_key: String,
    /// A key file to read instead; the contents never pass through the WebView.
    #[serde(default)]
    pub path: Option<String>,
    pub passphrase: Option<String>,
}

fn key_kind(alg: &russh::keys::ssh_key::Algorithm) -> String {
    use russh::keys::ssh_key::Algorithm;
    match alg {
        Algorithm::Ed25519 => "ed25519".into(),
        Algorithm::Rsa { .. } => "rsa".into(),
        Algorithm::Ecdsa { .. } => "ecdsa".into(),
        other => other.as_str().to_string(),
    }
}

fn store_private_key(
    vault: &crate::vault::Vault,
    name: &str,
    key: &russh::keys::PrivateKey,
) -> Result<KeyView, String> {
    use russh::keys::ssh_key::LineEnding;
    let public = key.public_key();
    let fp = public.fingerprint(russh::keys::HashAlg::Sha256).to_string();

    // The same key imported twice is one key.
    let rows = vault.store().list_items().map_err(|e| e.to_string())?;
    for row in rows.iter().filter(|r| r.kind == "key") {
        let item = vault.decrypt_item(row).map_err(|e| e.to_string())?;
        if item.payload.get("fingerprint").and_then(|v| v.as_str()) == Some(fp.as_str()) {
            return Ok(key_view(&item, row.updated_at));
        }
    }

    let public_line = public
        .to_openssh()
        .map_err(|e| format!("序列化公钥失败：{e}"))?;
    let private_pem = key
        .to_openssh(LineEnding::LF)
        .map_err(|e| format!("序列化私钥失败：{e}"))?;
    let id = uuid::Uuid::new_v4().to_string();
    let payload = serde_json::json!({
        "name": name,
        "kind": key_kind(&key.algorithm()),
        "fingerprint": fp,
        "public_key": public_line,
        "private_key": private_pem.as_str(),
    });
    let row = vault
        .encrypt_item(&id, "key", &payload)
        .map_err(|e| e.to_string())?;
    vault.store().put_item(&row).map_err(|e| e.to_string())?;
    Ok(KeyView {
        id,
        name: name.to_string(),
        kind: key_kind(&key.algorithm()),
        fingerprint: fp,
        public_key: public_line,
        created_at: row.updated_at,
    })
}

fn key_view(item: &PlainItem, updated_at: i64) -> KeyView {
    let f = |n: &str| str_field(&item.payload, n).unwrap_or_default();
    KeyView {
        id: item.id.clone(),
        name: f("name"),
        kind: f("kind"),
        fingerprint: f("fingerprint"),
        public_key: f("public_key"),
        created_at: updated_at,
    }
}

#[tauri::command]
pub fn key_import(state: State<'_, AppState>, input: KeyImportInput) -> Result<KeyView, String> {
    let text = match input.path.as_deref().filter(|p| !p.is_empty()) {
        Some(path) if input.private_key.trim().is_empty() => {
            std::fs::read_to_string(path).map_err(|e| format!("无法读取 {path}：{e}"))?
        }
        _ => input.private_key.clone(),
    };
    let key = russh::keys::decode_secret_key(
        &ssh::normalize_private_key(&text),
        input.passphrase.as_deref().filter(|p| !p.is_empty()),
    )
    .map_err(|e| {
        let msg = e.to_string();
        if msg.to_lowercase().contains("encrypted") || msg.contains("passphrase") {
            "这把私钥有口令，请填写口令".to_string()
        } else {
            format!("私钥无法解析：{msg}")
        }
    })?;
    let vault = state.vault.lock().map_err(|e| e.to_string())?;
    let name = if input.name.trim().is_empty() {
        "imported-key".to_string()
    } else {
        input.name.trim().to_string()
    };
    store_private_key(&vault, &name, &key)
}

#[tauri::command]
pub fn key_rename(state: State<'_, AppState>, id: String, name: String) -> Result<(), String> {
    let vault = state.vault.lock().map_err(|e| e.to_string())?;
    let row = vault
        .store()
        .get_item(&id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "密钥不存在".to_string())?;
    let mut item = vault.decrypt_item(&row).map_err(|e| e.to_string())?;
    item.payload["name"] = serde_json::Value::String(name);
    let row = vault
        .encrypt_item(&id, "key", &item.payload)
        .map_err(|e| e.to_string())?;
    vault.store().put_item(&row).map_err(|e| e.to_string())
}

// ---- ~/.ssh/config import --------------------------------------------------

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SshConfigCandidate {
    #[serde(flatten)]
    pub entry: crate::sshconfig::SshConfigEntry,
    /// A saved host already points at the same user@host:port.
    pub exists: bool,
}

#[tauri::command]
pub fn ssh_config_scan(state: State<'_, AppState>) -> Result<Vec<SshConfigCandidate>, String> {
    let entries = crate::sshconfig::read_user_config()?;
    let vault = state.vault.lock().map_err(|e| e.to_string())?;
    let rows = vault.store().list_items().map_err(|e| e.to_string())?;
    let mut existing = Vec::new();
    for row in rows.iter().filter(|r| r.kind == "host") {
        let item = vault.decrypt_item(row).map_err(|e| e.to_string())?;
        let h = to_host_view(&item);
        existing.push((h.address.to_lowercase(), h.port, h.username));
    }
    Ok(entries
        .into_iter()
        .map(|e| SshConfigCandidate {
            exists: existing
                .iter()
                .any(|(a, p, u)| *a == e.hostname.to_lowercase() && *p == e.port && *u == e.user),
            entry: e,
        })
        .collect())
}

#[derive(Debug, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ImportReport {
    pub hosts: usize,
    pub keys: usize,
    pub notes: Vec<String>,
}

/// Imports the chosen aliases as hosts. Identity files are read from disk and
/// moved into the key vault; one that needs a passphrase is left for the
/// agent, which usually already holds it.
#[tauri::command]
pub fn ssh_config_import(
    state: State<'_, AppState>,
    aliases: Vec<String>,
) -> Result<ImportReport, String> {
    let entries = crate::sshconfig::read_user_config()?;
    let chosen: Vec<_> = entries
        .into_iter()
        .filter(|e| aliases.contains(&e.alias))
        .collect();
    let vault = state.vault.lock().map_err(|e| e.to_string())?;
    let mut report = ImportReport::default();

    // Ids are assigned up front so a ProxyJump can point at a host imported
    // in the same batch.
    let ids: std::collections::HashMap<String, String> = chosen
        .iter()
        .map(|e| (e.alias.clone(), uuid::Uuid::new_v4().to_string()))
        .collect();
    let mut existing_by_name = std::collections::HashMap::new();
    for row in vault
        .store()
        .list_items()
        .map_err(|e| e.to_string())?
        .iter()
        .filter(|r| r.kind == "host")
    {
        let item = vault.decrypt_item(row).map_err(|e| e.to_string())?;
        existing_by_name.insert(to_host_view(&item).name, item.id.clone());
    }

    for e in &chosen {
        let mut key_id = None;
        if let Some(path) = &e.identity_file {
            // The default IdentityFile often does not exist; that is fine.
            if let Ok(text) = std::fs::read_to_string(path) {
                match russh::keys::decode_secret_key(&text, None) {
                    Ok(key) => {
                        let name = std::path::Path::new(path)
                            .file_name()
                            .map(|n| n.to_string_lossy().to_string())
                            .unwrap_or_else(|| e.alias.clone());
                        let before = vault.store().list_items().map(|r| r.len()).unwrap_or(0);
                        let view = store_private_key(&vault, &name, &key)?;
                        if vault.store().list_items().map(|r| r.len()).unwrap_or(0) > before {
                            report.keys += 1;
                        }
                        key_id = Some(view.id);
                    }
                    Err(_) => report.notes.push(format!(
                        "{}：{} 需要口令，未导入，将尝试 ssh-agent",
                        e.alias, path
                    )),
                }
            }
        }
        let jump_host_id = e.proxy_jump.as_ref().and_then(|j| {
            let alias = j.split('@').next_back().unwrap_or(j);
            let alias = alias.split(':').next().unwrap_or(alias);
            ids.get(alias)
                .cloned()
                .or_else(|| existing_by_name.get(alias).cloned())
        });
        if e.proxy_jump.is_some() && jump_host_id.is_none() {
            report.notes.push(format!(
                "{}：跳板机 {} 不在已选列表里，已忽略",
                e.alias,
                e.proxy_jump.as_deref().unwrap_or("")
            ));
        }
        let payload = serde_json::json!({
            "name": e.alias,
            "protocol": "ssh",
            "address": e.hostname,
            "port": e.port,
            "username": e.user,
            "tags": ["ssh-config"],
            "key_id": key_id,
            "jump_host_id": jump_host_id,
            "use_agent": true,
        });
        let id = &ids[&e.alias];
        let row = vault
            .encrypt_item(id, "host", &payload)
            .map_err(|e| e.to_string())?;
        vault.store().put_item(&row).map_err(|e| e.to_string())?;
        report.hosts += 1;
    }
    Ok(report)
}

// ---- Transfers ---------------------------------------------------------------

/// Starts a download in the background and returns its id at once. Progress
/// arrives on `transfer-event`.
#[tauri::command]
pub async fn sftp_download(
    state: State<'_, AppState>,
    session_id: String,
    remote: String,
    local: String,
) -> Result<String, String> {
    start_transfer(&state, session_id, "download", remote, local).await
}

/// Uploads local files or directories into `remote_dir`.
#[tauri::command]
pub async fn sftp_upload(
    state: State<'_, AppState>,
    session_id: String,
    locals: Vec<String>,
    remote_dir: String,
) -> Result<Vec<String>, String> {
    let mut ids = Vec::new();
    for local in locals {
        ids.push(
            start_transfer(
                &state,
                session_id.clone(),
                "upload",
                local,
                remote_dir.clone(),
            )
            .await?,
        );
    }
    Ok(ids)
}

/// Downloads into the user's Downloads folder without asking where.
#[tauri::command]
pub async fn sftp_download_to_downloads(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    session_id: String,
    remote: String,
) -> Result<String, String> {
    let dir = app
        .path()
        .download_dir()
        .map_err(|e| format!("找不到下载目录：{e}"))?;
    let name = remote
        .trim_end_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or("download")
        .to_string();
    let local = crate::transfer::unique_local_path(&dir, &name);
    start_transfer(
        &state,
        session_id,
        "download",
        remote,
        local.to_string_lossy().to_string(),
    )
    .await
}

async fn start_transfer(
    state: &AppState,
    session_id: String,
    direction: &str,
    from: String,
    to: String,
) -> Result<String, String> {
    let session = {
        let mgr = state.ssh.lock().map_err(|e| e.to_string())?;
        mgr.get(&session_id)
            .ok_or_else(|| "会话不存在".to_string())?
    };
    let id = uuid::Uuid::new_v4().to_string();
    let cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    state
        .transfers
        .lock()
        .map_err(|e| e.to_string())?
        .insert(id.clone(), cancel.clone());

    // Both a local path (upload) and a remote one (download) end in the name.
    let name = std::path::Path::new(from.trim_end_matches('/'))
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    let app = state.app.clone();
    let direction = direction.to_string();
    let tid = id.clone();
    tokio::spawn(async move {
        let base = crate::transfer::TransferEvent {
            id: tid.clone(),
            session_id: session_id.clone(),
            direction: direction.clone(),
            name,
            bytes: 0,
            total: 0,
            state: "running".into(),
            error: None,
        };
        let _ = app.emit("transfer-event", &base);
        let emit_app = app.clone();
        let progress_base = base.clone();
        let report = move |bytes: u64, total: u64| {
            let mut ev = progress_base.clone();
            ev.bytes = bytes;
            ev.total = total;
            let _ = emit_app.emit("transfer-event", &ev);
        };

        let result = async {
            let sftp = crate::sftp::open_sftp(session.handle()).await?;
            if direction == "upload" {
                crate::transfer::upload(&sftp, std::path::Path::new(&from), &to, &cancel, report)
                    .await
            } else {
                crate::transfer::download(&sftp, &from, std::path::Path::new(&to), &cancel, report)
                    .await
            }
        }
        .await;

        let mut done = base.clone();
        match result {
            Ok(()) => done.state = "done".into(),
            Err(e) if cancel.load(std::sync::atomic::Ordering::SeqCst) => {
                done.state = "cancelled".into();
                done.error = Some(e);
            }
            Err(e) => {
                done.state = "failed".into();
                done.error = Some(e);
            }
        }
        let _ = app.emit("transfer-event", &done);
        if let Some(state) = app.try_state::<AppState>() {
            if let Ok(mut t) = state.transfers.lock() {
                t.remove(&tid);
            }
        }
    });
    Ok(id)
}

#[tauri::command]
pub fn transfer_cancel(state: State<'_, AppState>, id: String) -> Result<(), String> {
    let map = state.transfers.lock().map_err(|e| e.to_string())?;
    if let Some(flag) = map.get(&id) {
        flag.store(true, std::sync::atomic::Ordering::SeqCst);
    }
    Ok(())
}

#[tauri::command]
pub fn key_delete(state: State<'_, AppState>, id: String) -> Result<(), String> {
    let vault = state.vault.lock().map_err(|e| e.to_string())?;
    vault.store().delete_item(&id).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn app_info() -> serde_json::Value {
    serde_json::json!({
        "name": "loki",
        "version": env!("CARGO_PKG_VERSION"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crypto::KdfParams;
    use crate::store::Store;
    use crate::vault::Vault;

    fn vault_with(id: &str, payload: serde_json::Value) -> Vault {
        let mut vault = Vault::new(Store::open_in_memory().unwrap()).unwrap();
        // `create` enforces the production KDF floor, so weaker test params
        // are refused; the default costs about a second.
        vault.create("test-password", KdfParams::default()).unwrap();
        let row = vault.encrypt_item(id, "provider", &payload).unwrap();
        vault.store().put_item(&row).unwrap();
        vault
    }

    #[test]
    fn conversations_are_refused_while_the_vault_is_locked() {
        // A vault that was never created or unlocked has no key.
        let vault = Vault::new(Store::open_in_memory().unwrap()).unwrap();
        assert!(require_unlocked(&vault).is_err());
        // The real gate for saving/reading: no key, no ciphertext.
        assert!(vault
            .encrypt_item("c1", "ai_conversation", &serde_json::json!({}))
            .is_err());
    }

    #[test]
    fn pruning_keeps_only_the_most_recent_conversations() {
        let mut vault = Vault::new(Store::open_in_memory().unwrap()).unwrap();
        vault.create("test-password", KdfParams::default()).unwrap();

        let limit = crate::ai::conversation::MAX_CONVERSATIONS;
        let total = limit + 3;
        for i in 0..total {
            let payload = serde_json::json!({
                "id": format!("c{i}"),
                "title": format!("对话 {i}"),
                "hostId": "h1",
                "hostLabel": "prod",
                "turns": [],
            });
            let row = vault
                .encrypt_item(&format!("c{i}"), "ai_conversation", &payload)
                .unwrap();
            // `encrypt_item` stamps now(); make the order explicit so "oldest"
            // is not a race between rows written in the same second.
            let mut row = row;
            row.updated_at = 1_000 + i as i64;
            vault.store().put_item(&row).unwrap();
        }

        prune_conversations(&vault).unwrap();

        let rows = vault.store().list_items().unwrap();
        let conversations: Vec<_> = rows
            .iter()
            .filter(|r| r.kind == "ai_conversation")
            .collect();
        assert_eq!(conversations.len(), limit);
        // The three oldest are gone; the newest survive.
        assert!(conversations
            .iter()
            .all(|r| r.id != "c0" && r.id != "c1" && r.id != "c2"));
        assert!(conversations
            .iter()
            .any(|r| r.id == format!("c{}", total - 1)));
    }

    #[test]
    fn recovery_export_refuses_anything_but_a_code_and_writes_privately() {
        let dir = std::env::temp_dir().join(format!("loki-rc-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("code.txt");
        let p = path.to_string_lossy().to_string();

        // Arbitrary text is rejected and nothing is written.
        assert!(recovery_code_export(p.clone(), "rm -rf /".into()).is_err());
        assert!(!path.exists());

        let code = crate::crypto::RecoveryCode::generate().unwrap().display;
        recovery_code_export(p.clone(), code.clone()).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains(&code));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600);
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn dates_render_in_utc() {
        assert_eq!(format_utc(0), "1970-01-01 00:00 UTC");
        assert_eq!(format_utc(1_793_235_720), "2026-10-29 01:02 UTC");
        // A leap day, to exercise the calendar arithmetic.
        assert_eq!(format_utc(1_709_251_140), "2024-02-29 23:59 UTC");
    }

    #[test]
    fn stored_key_is_only_reused_for_the_same_server() {
        let vault = vault_with(
            "p1",
            serde_json::json!({
                "base_url": "https://api.openai.com/v1",
                "models": ["gpt-4o"],
                "api_key": "sk-secret",
            }),
        );
        // Same server, different path: the user just fixed the version.
        assert_eq!(
            reusable_key(&vault, "p1", "https://api.openai.com/v2").unwrap(),
            "sk-secret"
        );
        // Another host must never receive the stored key.
        assert_eq!(
            reusable_key(&vault, "p1", "https://proxy.example.com/v1").unwrap(),
            ""
        );
        // Unknown id: nothing to reuse.
        assert_eq!(
            reusable_key(&vault, "nope", "https://api.openai.com/v1").unwrap(),
            ""
        );
    }

    #[test]
    fn legacy_rows_compare_against_their_upgraded_url() {
        // A 0.7 row stored the URL without /v1 and a single `model`.
        let vault = vault_with(
            "old",
            serde_json::json!({
                "base_url": "https://api.anthropic.com",
                "model": "claude",
                "api_key": "ak",
            }),
        );
        assert_eq!(
            reusable_key(&vault, "old", "https://api.anthropic.com/v1").unwrap(),
            "ak"
        );
    }
}
