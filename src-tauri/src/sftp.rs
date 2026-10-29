//! SFTP: a file panel beside the terminal.
//!
//! Each SSH session lazily opens one SFTP subsystem channel and keeps the
//! handle for the lifetime of the connection. Everything here runs server-side;
//! the WebView only ever sees names, sizes and permissions.

use russh_sftp::client::SftpSession;
use serde::Serialize;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileEntry {
    pub name: String,
    pub path: String,
    /// `dir`, `file` or `link`.
    pub kind: String,
    pub size: u64,
    /// Permission bits as an octal string such as `644`.
    pub mode: Option<String>,
    /// Seconds since the epoch, when the server reports it.
    pub modified: Option<u64>,
}

/// One open SFTP channel, keyed by session id.
pub type SftpMap = HashMap<String, Arc<Mutex<SftpSession>>>;

/// Opens the subsystem on an existing SSH session.
pub async fn open_sftp(
    handle: &russh::client::Handle<crate::ssh::ClientHandler>,
) -> Result<SftpSession, String> {
    let mut channel = handle
        .channel_open_session()
        .await
        .map_err(|e| format!("无法打开 SFTP 通道：{e}"))?;
    channel
        .request_subsystem(true, "sftp")
        .await
        .map_err(|e| format!("服务器拒绝了 SFTP 子系统：{e}"))?;

    // Wait for the server's confirmation before handing the channel to the SFTP
    // client. The library otherwise starts counting its handshake timeout while
    // the subsystem reply is still queued ahead of the SFTP version packet.
    loop {
        match tokio::time::timeout(std::time::Duration::from_secs(10), channel.wait()).await {
            Ok(Some(russh::ChannelMsg::Success)) => break,
            Ok(Some(russh::ChannelMsg::Failure)) => {
                return Err("服务器拒绝了 SFTP 子系统请求".to_string())
            }
            Ok(Some(russh::ChannelMsg::Close)) | Ok(None) => {
                return Err("服务器在启用 SFTP 前关闭了通道".to_string())
            }
            Ok(Some(_)) => continue,
            Err(_) => return Err("等待 SFTP 子系统确认超时".to_string()),
        }
    }

    // Without a deadline a server that opens the channel but never speaks SFTP
    // would hang the whole panel; the handshake is a few packets round-trip.
    let stream = channel.into_stream();
    tokio::time::timeout(std::time::Duration::from_secs(10), SftpSession::new(stream))
        .await
        .map_err(|_| "SFTP 握手超时".to_string())?
        .map_err(|e| format!("SFTP 握手失败：{e}"))
}

/// The user's home directory on the remote host, used as the panel's root.
pub async fn home_dir(sftp: &SftpSession) -> Result<String, String> {
    sftp.canonicalize(".")
        .await
        .map_err(|e| format!("无法确定远端主目录：{e}"))
}

/// Lists one directory. `.` and `..` are dropped: the UI navigates upward itself.
pub async fn list_dir(sftp: &SftpSession, path: &str) -> Result<Vec<FileEntry>, String> {
    let mut out = Vec::new();
    let dir = sftp
        .read_dir(path)
        .await
        .map_err(|e| format!("无法读取目录 {path}：{e}"))?;

    for entry in dir {
        let name = entry.file_name();
        if name == "." || name == ".." {
            continue;
        }
        let attrs = entry.metadata();
        let is_dir = attrs.is_dir();
        let is_link = attrs.is_symlink();
        let full = join_path(path, &name);

        out.push(FileEntry {
            name,
            path: full,
            kind: if is_link {
                "link".to_string()
            } else if is_dir {
                "dir".to_string()
            } else {
                "file".to_string()
            },
            size: attrs.len(),
            mode: attrs.permissions.map(|p| format!("{:o}", p & 0o7777)),
            modified: attrs.mtime.map(|m| m as u64),
        });
    }

    // Directories first, then names, like every file manager people already use.
    out.sort_by(|a, b| {
        b.kind
            .eq("dir")
            .cmp(&a.kind.eq("dir"))
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    Ok(out)
}

/// Joins without producing `//` or dropping a trailing slash rule.
pub fn join_path(dir: &str, name: &str) -> String {
    if dir == "/" {
        format!("/{name}")
    } else if dir.ends_with('/') {
        format!("{dir}{name}")
    } else {
        format!("{dir}/{name}")
    }
}

/// The parent of a directory, staying at `/`.
pub fn parent_path(path: &str) -> String {
    let trimmed = path.trim_end_matches('/');
    match trimmed.rfind('/') {
        Some(0) => "/".to_string(),
        Some(i) => trimmed[..i].to_string(),
        None => path.to_string(),
    }
}

/// Reads a file into memory.
///
/// The panel is for editing config files, not transferring disks; a size cap
/// keeps a 4 GB log from being pulled into the WebView.
pub const MAX_READ_BYTES: usize = 4 * 1024 * 1024;

pub async fn read_file(sftp: &SftpSession, path: &str) -> Result<Vec<u8>, String> {
    let meta = sftp
        .metadata(path)
        .await
        .map_err(|e| format!("无法读取 {path} 的信息：{e}"))?;
    if meta.len() as usize > MAX_READ_BYTES {
        return Err(format!(
            "文件太大（{} MB），仅在编辑器里打开小于 {} MB 的文件",
            meta.len() / 1024 / 1024,
            MAX_READ_BYTES / 1024 / 1024
        ));
    }
    sftp.read(path)
        .await
        .map_err(|e| format!("读取 {path} 失败：{e}"))
}

/// Creates or overwrites a file.
///
/// `SftpSession::write` opens with `WRITE` only, which fails on a path that does
/// not exist yet, so the flags are set explicitly here.
pub async fn write_file(sftp: &SftpSession, path: &str, data: &[u8]) -> Result<(), String> {
    use russh_sftp::protocol::OpenFlags;
    use tokio::io::AsyncWriteExt;

    let mut file = sftp
        .open_with_flags(
            path,
            OpenFlags::WRITE | OpenFlags::CREATE | OpenFlags::TRUNCATE,
        )
        .await
        .map_err(|e| format!("打开 {path} 失败：{e}"))?;
    file.write_all(data)
        .await
        .map_err(|e| format!("写入 {path} 失败：{e}"))?;
    file.close()
        .await
        .map_err(|e| format!("关闭 {path} 失败：{e}"))
}

pub async fn remove(sftp: &SftpSession, path: &str, is_dir: bool) -> Result<(), String> {
    if is_dir {
        sftp.remove_dir(path)
            .await
            .map_err(|e| format!("删除目录 {path} 失败：{e}"))
    } else {
        sftp.remove_file(path)
            .await
            .map_err(|e| format!("删除 {path} 失败：{e}"))
    }
}

pub async fn mkdir(sftp: &SftpSession, path: &str) -> Result<(), String> {
    sftp.create_dir(path)
        .await
        .map_err(|e| format!("创建目录 {path} 失败：{e}"))
}

pub async fn rename(sftp: &SftpSession, from: &str, to: &str) -> Result<(), String> {
    sftp.rename(from, to)
        .await
        .map_err(|e| format!("重命名失败：{e}"))
}

// ---- Session-scoped helpers (used by the AI tools) -----------------------
//
// The command layer keeps its own cache in `AppState`; these open a channel on
// demand and close it again, which is right for occasional tool calls.

async fn with_sftp<T, F, Fut>(session: &crate::ssh::LiveSession, f: F) -> Result<T, String>
where
    F: FnOnce(SftpSession) -> Fut,
    Fut: std::future::Future<Output = Result<T, String>>,
{
    let sftp = open_sftp(session.handle()).await?;
    f(sftp).await
}

pub async fn list_for_session(
    session: &crate::ssh::LiveSession,
    path: &str,
) -> Result<Vec<FileEntry>, String> {
    with_sftp(session, |sftp| async move {
        let home = home_dir(&sftp).await?;
        let target = if path.is_empty() {
            home
        } else {
            path.to_string()
        };
        list_dir(&sftp, &target).await
    })
    .await
}

pub async fn read_for_session(
    session: &crate::ssh::LiveSession,
    path: &str,
) -> Result<String, String> {
    with_sftp(session, |sftp| async move {
        let bytes = read_file(&sftp, path).await?;
        String::from_utf8(bytes).map_err(|_| "不是文本文件".to_string())
    })
    .await
}

pub async fn write_for_session(
    session: &crate::ssh::LiveSession,
    path: &str,
    content: &str,
) -> Result<(), String> {
    with_sftp(session, |sftp| async move {
        write_file(&sftp, path, content.as_bytes()).await
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn join_handles_roots_and_trailing_slashes() {
        assert_eq!(join_path("/", "etc"), "/etc");
        assert_eq!(join_path("/home/u", "file"), "/home/u/file");
        assert_eq!(join_path("/home/u/", "file"), "/home/u/file");
        assert_eq!(join_path("relative", "x"), "relative/x");
    }

    #[test]
    fn parent_walks_up_and_stops_at_root() {
        assert_eq!(parent_path("/home/u/file"), "/home/u");
        assert_eq!(parent_path("/home"), "/");
        assert_eq!(parent_path("/"), "/");
        // A trailing slash must not produce an empty segment.
        assert_eq!(parent_path("/home/u/"), "/home");
    }
}
