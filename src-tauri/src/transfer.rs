//! File transfers over SFTP: upload from and download to the local disk.
//!
//! Each transfer opens its own SFTP channel so a large copy never blocks the
//! file panel's listing, which shares a cached channel. Files are written to a
//! `.part` name first and renamed into place at the end, so an interrupted
//! download never leaves a truncated file under the real name.

use russh_sftp::client::SftpSession;
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const CHUNK: usize = 256 * 1024;

/// What the UI receives about a running transfer.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TransferEvent {
    pub id: String,
    pub session_id: String,
    /// `upload` or `download`.
    pub direction: String,
    pub name: String,
    pub bytes: u64,
    pub total: u64,
    /// `running`, `done`, `failed` or `cancelled`.
    pub state: String,
    pub error: Option<String>,
}

/// One file to copy, plus the directories that must exist first.
struct Plan {
    dirs: Vec<String>,
    files: Vec<(String, String, u64)>,
    total: u64,
}

/// Reports progress at most ten times a second, plus once at the end.
pub struct Progress<F: FnMut(u64, u64)> {
    report: F,
    last: Instant,
    pub bytes: u64,
    pub total: u64,
}

impl<F: FnMut(u64, u64)> Progress<F> {
    pub fn new(total: u64, report: F) -> Self {
        Self {
            report,
            last: Instant::now() - Duration::from_secs(1),
            bytes: 0,
            total,
        }
    }

    fn add(&mut self, n: u64) {
        self.bytes += n;
        if self.last.elapsed() >= Duration::from_millis(100) {
            self.last = Instant::now();
            (self.report)(self.bytes, self.total);
        }
    }

    fn finish(&mut self) {
        (self.report)(self.bytes, self.total);
    }
}

fn cancelled(flag: &AtomicBool) -> Result<(), String> {
    if flag.load(Ordering::SeqCst) {
        Err("已取消".into())
    } else {
        Ok(())
    }
}

/// Builds the copy list for a remote file or directory.
async fn plan_download(sftp: &SftpSession, remote: &str, local: &Path) -> Result<Plan, String> {
    let meta = sftp
        .metadata(remote)
        .await
        .map_err(|e| format!("无法读取 {remote}：{e}"))?;
    let mut plan = Plan {
        dirs: vec![],
        files: vec![],
        total: 0,
    };
    if !meta.is_dir() {
        plan.total = meta.len();
        plan.files.push((
            remote.to_string(),
            local.to_string_lossy().to_string(),
            meta.len(),
        ));
        return Ok(plan);
    }

    // Breadth-first so parents are created before children.
    let mut queue = vec![(remote.to_string(), local.to_path_buf())];
    while let Some((rdir, ldir)) = queue.pop() {
        plan.dirs.push(ldir.to_string_lossy().to_string());
        let entries = sftp
            .read_dir(&rdir)
            .await
            .map_err(|e| format!("无法读取目录 {rdir}：{e}"))?;
        for entry in entries {
            let name = entry.file_name();
            if name == "." || name == ".." {
                continue;
            }
            let attrs = entry.metadata();
            let rpath = crate::sftp::join_path(&rdir, &name);
            let lpath = ldir.join(&name);
            // Links are skipped: following them can loop, and copying them as
            // files would silently change what they are.
            if attrs.is_symlink() {
                continue;
            }
            if attrs.is_dir() {
                queue.push((rpath, lpath));
            } else {
                plan.total += attrs.len();
                plan.files
                    .push((rpath, lpath.to_string_lossy().to_string(), attrs.len()));
            }
        }
    }
    Ok(plan)
}

/// Copies a remote file or directory to `local`.
pub async fn download(
    sftp: &SftpSession,
    remote: &str,
    local: &Path,
    cancel: &AtomicBool,
    report: impl FnMut(u64, u64),
) -> Result<(), String> {
    let plan = plan_download(sftp, remote, local).await?;
    let mut progress = Progress::new(plan.total, report);
    for dir in &plan.dirs {
        tokio::fs::create_dir_all(dir)
            .await
            .map_err(|e| format!("无法创建本地目录 {dir}：{e}"))?;
    }
    for (rpath, lpath, _) in &plan.files {
        cancelled(cancel)?;
        let part = format!("{lpath}.part");
        let result = async {
            let mut src = sftp
                .open(rpath)
                .await
                .map_err(|e| format!("打开 {rpath} 失败：{e}"))?;
            let mut dst = tokio::fs::File::create(&part)
                .await
                .map_err(|e| format!("无法写入 {lpath}：{e}"))?;
            let mut buf = vec![0u8; CHUNK];
            loop {
                cancelled(cancel)?;
                let n = src
                    .read(&mut buf)
                    .await
                    .map_err(|e| format!("读取 {rpath} 失败：{e}"))?;
                if n == 0 {
                    break;
                }
                dst.write_all(&buf[..n])
                    .await
                    .map_err(|e| format!("写入 {lpath} 失败：{e}"))?;
                progress.add(n as u64);
            }
            dst.flush().await.map_err(|e| e.to_string())?;
            tokio::fs::rename(&part, lpath)
                .await
                .map_err(|e| format!("无法保存 {lpath}：{e}"))
        }
        .await;
        if result.is_err() {
            let _ = tokio::fs::remove_file(&part).await;
        }
        result?;
    }
    progress.finish();
    Ok(())
}

fn plan_upload(local: &Path, remote_dir: &str) -> Result<Plan, String> {
    let meta =
        std::fs::metadata(local).map_err(|e| format!("无法读取 {}：{e}", local.display()))?;
    let name = local
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .ok_or("无效的本地路径")?;
    let mut plan = Plan {
        dirs: vec![],
        files: vec![],
        total: 0,
    };
    let root = crate::sftp::join_path(remote_dir, &name);
    if meta.is_file() {
        plan.total = meta.len();
        plan.files
            .push((local.to_string_lossy().to_string(), root, meta.len()));
        return Ok(plan);
    }

    let mut queue = vec![(local.to_path_buf(), root)];
    while let Some((ldir, rdir)) = queue.pop() {
        plan.dirs.push(rdir.clone());
        let entries =
            std::fs::read_dir(&ldir).map_err(|e| format!("无法读取 {}：{e}", ldir.display()))?;
        for entry in entries.flatten() {
            let ft = match entry.file_type() {
                Ok(t) => t,
                Err(_) => continue,
            };
            let name = entry.file_name().to_string_lossy().to_string();
            let rpath = crate::sftp::join_path(&rdir, &name);
            if ft.is_dir() {
                queue.push((entry.path(), rpath));
            } else if ft.is_file() {
                let size = entry.metadata().map(|m| m.len()).unwrap_or(0);
                plan.total += size;
                plan.files
                    .push((entry.path().to_string_lossy().to_string(), rpath, size));
            }
        }
    }
    Ok(plan)
}

/// Copies a local file or directory into `remote_dir`.
pub async fn upload(
    sftp: &SftpSession,
    local: &Path,
    remote_dir: &str,
    cancel: &AtomicBool,
    report: impl FnMut(u64, u64),
) -> Result<(), String> {
    use russh_sftp::protocol::OpenFlags;

    let plan = plan_upload(local, remote_dir)?;
    let mut progress = Progress::new(plan.total, report);
    for dir in &plan.dirs {
        // An existing directory is fine; anything else surfaces on the first
        // file write below with a clearer message.
        let _ = sftp.create_dir(dir).await;
    }
    for (lpath, rpath, _) in &plan.files {
        cancelled(cancel)?;
        let mut src = tokio::fs::File::open(lpath)
            .await
            .map_err(|e| format!("无法读取 {lpath}：{e}"))?;
        let mut dst = sftp
            .open_with_flags(
                rpath,
                OpenFlags::WRITE | OpenFlags::CREATE | OpenFlags::TRUNCATE,
            )
            .await
            .map_err(|e| format!("无法写入 {rpath}：{e}"))?;
        let mut buf = vec![0u8; CHUNK];
        loop {
            cancelled(cancel)?;
            let n = src
                .read(&mut buf)
                .await
                .map_err(|e| format!("读取 {lpath} 失败：{e}"))?;
            if n == 0 {
                break;
            }
            dst.write_all(&buf[..n])
                .await
                .map_err(|e| format!("写入 {rpath} 失败：{e}"))?;
            progress.add(n as u64);
        }
        dst.shutdown()
            .await
            .map_err(|e| format!("关闭 {rpath} 失败：{e}"))?;
    }
    progress.finish();
    Ok(())
}

/// Removes a remote directory and everything in it.
///
/// Symlinks are removed as links, never followed, and the root is refused
/// outright: one mis-click must not be able to empty a server.
pub async fn remove_recursive(sftp: &SftpSession, path: &str) -> Result<(), String> {
    if path.trim_end_matches('/').is_empty() {
        return Err("不能删除根目录".into());
    }
    let mut dirs = vec![path.to_string()];
    let mut order = Vec::new();
    while let Some(dir) = dirs.pop() {
        let entries = sftp
            .read_dir(&dir)
            .await
            .map_err(|e| format!("无法读取目录 {dir}：{e}"))?;
        for entry in entries {
            let name = entry.file_name();
            if name == "." || name == ".." {
                continue;
            }
            let full = crate::sftp::join_path(&dir, &name);
            let attrs = entry.metadata();
            if attrs.is_dir() && !attrs.is_symlink() {
                dirs.push(full);
            } else {
                sftp.remove_file(&full)
                    .await
                    .map_err(|e| format!("删除 {full} 失败：{e}"))?;
            }
        }
        order.push(dir);
    }
    for dir in order.into_iter().rev() {
        sftp.remove_dir(&dir)
            .await
            .map_err(|e| format!("删除目录 {dir} 失败：{e}"))?;
    }
    Ok(())
}

/// Picks a name in `dir` that does not exist yet: `a.txt`, `a (1).txt`, ...
pub fn unique_local_path(dir: &Path, name: &str) -> PathBuf {
    let candidate = dir.join(name);
    if !candidate.exists() {
        return candidate;
    }
    let (stem, ext) = match name.rfind('.') {
        Some(i) if i > 0 => (&name[..i], &name[i..]),
        _ => (name, ""),
    };
    for n in 1..1000 {
        let p = dir.join(format!("{stem} ({n}){ext}"));
        if !p.exists() {
            return p;
        }
    }
    candidate
}

/// The running transfers' cancel flags, keyed by transfer id.
pub type TransferMap = std::collections::HashMap<String, Arc<AtomicBool>>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unique_path_appends_a_counter() {
        let dir = std::env::temp_dir().join(format!("loki-unique-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(unique_local_path(&dir, "a.txt"), dir.join("a.txt"));
        std::fs::write(dir.join("a.txt"), b"x").unwrap();
        assert_eq!(unique_local_path(&dir, "a.txt"), dir.join("a (1).txt"));
        std::fs::write(dir.join(".env"), b"x").unwrap();
        // A dotfile has no extension to keep.
        assert_eq!(unique_local_path(&dir, ".env"), dir.join(".env (1)"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn upload_plan_walks_directories() {
        let dir = std::env::temp_dir().join(format!("loki-plan-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        std::fs::write(dir.join("a"), b"12").unwrap();
        std::fs::write(dir.join("sub/b"), b"345").unwrap();
        let plan = plan_upload(&dir, "/srv").unwrap();
        let root = format!("/srv/{}", dir.file_name().unwrap().to_string_lossy());
        assert_eq!(plan.total, 5);
        assert_eq!(plan.files.len(), 2);
        assert!(plan.dirs.contains(&root));
        assert!(plan.dirs.contains(&format!("{root}/sub")));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
