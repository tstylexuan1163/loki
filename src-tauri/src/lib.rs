//! loki — an SSH client with an AI assistant beside every terminal.

pub mod ai;
pub mod commands;
pub mod crypto;
pub mod direct;
pub mod forward;
pub mod local;
pub mod serial;
pub mod sftp;
pub mod ssh;
pub mod sshconfig;
pub mod stats;
pub mod store;
pub mod sync;
pub mod telnet;
pub mod transfer;
pub mod vault;

use ssh::SshManager;
use std::sync::Mutex;
use tauri::Manager;
use vault::Vault;

pub struct AppState {
    pub vault: Mutex<Vault>,
    pub ssh: Mutex<SshManager>,
    /// One lazily-opened SFTP channel per session.
    pub sftp: Mutex<sftp::SftpMap>,
    pub forwards: Mutex<forward::ForwardManager>,
    /// Channel the running turn listens on for tool approvals.
    pub ai_approvals: Mutex<Option<tokio::sync::mpsc::UnboundedSender<ai::Approval>>>,
    /// Telnet, serial and local-shell sessions, which do not share SSH's types.
    pub direct: Mutex<direct::DirectManager>,
    /// Cancel flags for running SFTP transfers.
    pub transfers: Mutex<transfer::TransferMap>,
    /// Kept so background tasks can emit events without a command in flight.
    pub app: tauri::AppHandle,
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("loki_lib=info,warn")),
        )
        .init();

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            // Everything private lives under the app's data dir. LOKI_DATA_DIR
            // points a development build at a throwaway vault instead.
            let data_dir = match std::env::var_os("LOKI_DATA_DIR") {
                Some(dir) if !dir.is_empty() => std::path::PathBuf::from(dir),
                _ => app.path().app_data_dir()?,
            };
            let db_path = data_dir.join("loki.db");
            tracing::info!(db = %db_path.display(), "opening local store");

            let store = store::Store::open(&db_path)?;
            let vault = Vault::new(store)?;

            app.manage(AppState {
                vault: Mutex::new(vault),
                ssh: Mutex::new(SshManager::new()),
                sftp: Mutex::new(sftp::SftpMap::new()),
                forwards: Mutex::new(forward::ForwardManager::new()),
                ai_approvals: Mutex::new(None),
                direct: Mutex::new(direct::DirectManager::new()),
                transfers: Mutex::new(transfer::TransferMap::new()),
                app: app.handle().clone(),
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::app_info,
            commands::vault_status,
            commands::vault_create,
            commands::vault_unlock,
            commands::vault_recover,
            commands::vault_change_password,
            commands::vault_lock,
            commands::hosts_list,
            commands::host_save,
            commands::host_delete,
            commands::host_duplicate,
            commands::known_hosts_list,
            commands::known_host_delete,
            commands::local_open,
            commands::session_stats,
            commands::key_import,
            commands::key_rename,
            commands::ssh_config_scan,
            commands::ssh_config_import,
            commands::sftp_download,
            commands::sftp_download_to_downloads,
            commands::sftp_upload,
            commands::transfer_cancel,
            commands::serial_ports,
            commands::telnet_open,
            commands::serial_open,
            commands::direct_write,
            commands::direct_resize,
            commands::direct_close,
            commands::sync_connect,
            commands::sync_status,
            commands::sync_run,
            commands::sync_disconnect,
            commands::ai_providers_list,
            commands::ai_provider_save,
            commands::ai_provider_delete,
            commands::ai_send,
            commands::ai_approve,
            commands::forward_start,
            commands::forward_list,
            commands::forward_stop,
            commands::sftp_list,
            commands::sftp_read,
            commands::sftp_write,
            commands::sftp_mkdir,
            commands::sftp_delete,
            commands::sftp_rename,
            commands::keys_list,
            commands::key_generate,
            commands::key_delete,
            commands::sessions_list,
            commands::session_open,
            commands::term_write,
            commands::term_backlog,
            commands::term_resize,
            commands::session_close,
        ])
        .run(tauri::generate_context!())
        .expect("error while running loki");
}
