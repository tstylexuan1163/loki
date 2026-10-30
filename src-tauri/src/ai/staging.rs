//! Putting an attachment on the server.
//!
//! The steps — create the folder with a tight mode, write via a `.part` file,
//! rename, then verify the remote hash — are here rather than in the command so
//! a live test can drive them against a real sshd.

use russh_sftp::client::SftpSession;

use crate::ai::attach;
use crate::ssh::LiveSession;

/// Creates a directory, tolerating one that already exists.
pub async fn ensure_dir(sftp: &SftpSession, path: &str, mode: u32) -> Result<(), String> {
    match sftp.create_dir(path).await {
        Ok(()) => {}
        Err(e) => {
            if sftp.metadata(path).await.is_err() {
                return Err(format!("创建 {path} 失败：{e}"));
            }
        }
    }
    set_mode(sftp, path, mode).await
}

/// Applies a permission mask. A refusal is not fatal: the caller decides how to
/// report it.
pub async fn set_mode(sftp: &SftpSession, path: &str, mode: u32) -> Result<(), String> {
    use russh_sftp::client::fs::Metadata;
    let attrs = Metadata {
        permissions: Some(mode),
        ..Default::default()
    };
    sftp.set_metadata(path, attrs)
        .await
        .map_err(|e| format!("设置 {path} 权限失败：{e}"))
}

/// Deletes a directory tree; returns how many entries went away.
pub async fn remove_tree(sftp: &SftpSession, path: &str) -> u64 {
    let mut removed = 0u64;
    if let Ok(entries) = crate::sftp::list_dir(sftp, path).await {
        for entry in entries {
            if entry.name == "." || entry.name == ".." {
                continue;
            }
            let child = format!("{}/{}", path.trim_end_matches('/'), entry.name);
            if entry.kind == "dir" {
                removed += Box::pin(remove_tree(sftp, &child)).await;
            } else if sftp.remove_file(&child).await.is_ok() {
                removed += 1;
            }
        }
    }
    if sftp.remove_dir(path).await.is_ok() {
        removed += 1;
    }
    removed
}

/// Creates `~/.loki/uploads/<batch>` with 700 on each level and returns its path.
pub async fn prepare_batch(
    sftp: &SftpSession,
    home: &str,
    batch_name: &str,
) -> Result<String, String> {
    let root = format!("{home}/{}", attach::STAGING_ROOT);
    let uploads = format!("{root}/{}", attach::STAGING_SUBDIR);
    let dir = format!("{uploads}/{batch_name}");
    ensure_dir(sftp, &root, 0o700).await?;
    ensure_dir(sftp, &uploads, 0o700).await?;
    ensure_dir(sftp, &dir, 0o700).await?;
    Ok(dir)
}

/// Writes one file into `dir` and checks it on the server.
///
/// `report(sent, total)` is called as chunks go out (at most ten times a
/// second, and once at the end), so a large file shows real progress rather
/// than sitting at 0% until it is done.
///
/// Returns the remote path and whether the hash matched. A server without
/// `sha256sum` or `shasum` yields `false`, not an error: the copy may still be
/// fine, we just could not prove it.
pub async fn upload_file(
    session: &LiveSession,
    sftp: &SftpSession,
    dir: &str,
    name: &str,
    bytes: &[u8],
    report: impl FnMut(u64, u64),
) -> Result<(String, bool), String> {
    use russh_sftp::protocol::OpenFlags;
    use tokio::io::AsyncWriteExt;

    let remote_path = format!("{dir}/{name}");
    let part = format!("{remote_path}.part");

    let mut progress = crate::transfer::Progress::new(bytes.len() as u64, report);
    let mut dst = sftp
        .open_with_flags(
            &part,
            OpenFlags::WRITE | OpenFlags::CREATE | OpenFlags::TRUNCATE,
        )
        .await
        .map_err(|e| format!("无法写入 {part}：{e}"))?;
    for chunk in bytes.chunks(crate::transfer::CHUNK) {
        dst.write_all(chunk)
            .await
            .map_err(|e| format!("写入 {part} 失败：{e}"))?;
        progress.add(chunk.len() as u64);
    }
    dst.shutdown()
        .await
        .map_err(|e| format!("关闭 {part} 失败：{e}"))?;
    progress.finish();

    crate::sftp::rename(sftp, &part, &remote_path).await?;
    let _ = set_mode(sftp, &remote_path, 0o600).await;

    let cmd = format!(
        "sha256sum {} 2>/dev/null || shasum -a 256 {}",
        attach::shell_escape(&remote_path),
        attach::shell_escape(&remote_path)
    );
    let want = crate::ai::attach::sha256_hex(bytes);
    let verified = match crate::ssh::exec_with_timeout(
        session,
        &cmd,
        std::time::Duration::from_secs(15),
        8192,
    )
    .await
    {
        Ok(out) => out
            .split_whitespace()
            .next()
            .map(|h| h.eq_ignore_ascii_case(&want))
            .unwrap_or(false),
        Err(_) => false,
    };

    Ok((remote_path, verified))
}
