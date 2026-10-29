//! Local shell tabs: a PTY on this machine, driven by the same terminal pane
//! as SSH.
//!
//! The reader runs on its own OS thread because PTY reads block. When the
//! shell exits the read returns EOF (the slave end was dropped after spawn, so
//! the shell holds the only other copy) and the thread reports the exit code.

use portable_pty::{native_pty_system, ChildKiller, CommandBuilder, MasterPty, PtySize};
use std::io::{Read, Write};
use std::sync::Mutex;

pub struct LocalPty {
    master: Mutex<Box<dyn MasterPty + Send>>,
    writer: Mutex<Box<dyn Write + Send>>,
    killer: Mutex<Box<dyn ChildKiller + Send + Sync>>,
    pub shell: String,
}

/// The user's home directory: `HOME` on Unix, `USERPROFILE` on Windows.
pub fn home_dir() -> Option<String> {
    ["HOME", "USERPROFILE"]
        .iter()
        .filter_map(|k| std::env::var(k).ok())
        .find(|v| !v.is_empty())
}

/// The user's login shell.
///
/// `$SHELL` is missing when the app is started from Finder, so fall back to
/// the platform default rather than to `sh`, which most people never use.
pub fn default_shell() -> String {
    if let Ok(s) = std::env::var("SHELL") {
        if !s.is_empty() && std::path::Path::new(&s).exists() {
            return s;
        }
    }
    if cfg!(target_os = "macos") {
        "/bin/zsh".into()
    } else if cfg!(windows) {
        "powershell.exe".into()
    } else {
        "/bin/bash".into()
    }
}

impl LocalPty {
    /// Starts the shell. `on_data` gets every chunk of output; `on_exit` runs
    /// once, after the last chunk, with the exit code if there is one.
    pub fn spawn(
        cols: u16,
        rows: u16,
        on_data: impl FnMut(Vec<u8>) + Send + 'static,
        on_exit: impl FnOnce(Option<u32>) + Send + 'static,
    ) -> Result<Self, String> {
        let pty = native_pty_system();
        let pair = pty
            .openpty(PtySize {
                rows: rows.max(1),
                cols: cols.max(1),
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| format!("无法创建本地终端：{e}"))?;

        let shell = default_shell();
        let mut cmd = CommandBuilder::new(&shell);
        if !cfg!(windows) {
            // A login shell reads the profile, which is where PATH usually
            // gets set; without it Homebrew tools are "not found".
            cmd.arg("-l");
        }
        if let Some(home) = home_dir() {
            cmd.cwd(home);
        }
        cmd.env("TERM", "xterm-256color");
        cmd.env("COLORTERM", "truecolor");
        if std::env::var("LANG").map(|v| v.is_empty()).unwrap_or(true) {
            cmd.env("LANG", "en_US.UTF-8");
        }

        let mut child = pair
            .slave
            .spawn_command(cmd)
            .map_err(|e| format!("无法启动 {shell}：{e}"))?;
        // Only the shell may hold the slave end, or EOF never arrives.
        drop(pair.slave);

        let killer = child.clone_killer();
        let mut reader = pair
            .master
            .try_clone_reader()
            .map_err(|e| format!("无法读取本地终端：{e}"))?;
        let writer = pair
            .master
            .take_writer()
            .map_err(|e| format!("无法写入本地终端：{e}"))?;

        let mut on_data = on_data;
        std::thread::Builder::new()
            .name("loki-local-pty".into())
            .spawn(move || {
                let mut buf = [0u8; 16 * 1024];
                loop {
                    match reader.read(&mut buf) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => on_data(buf[..n].to_vec()),
                    }
                }
                let code = child.wait().ok().map(|s| s.exit_code());
                on_exit(code);
            })
            .map_err(|e| format!("无法启动读取线程：{e}"))?;

        Ok(Self {
            master: Mutex::new(pair.master),
            writer: Mutex::new(writer),
            killer: Mutex::new(killer),
            shell,
        })
    }

    pub fn write(&self, data: &[u8]) -> Result<(), String> {
        let mut w = self.writer.lock().map_err(|e| e.to_string())?;
        w.write_all(data)
            .and_then(|_| w.flush())
            .map_err(|e| format!("写入失败：{e}"))
    }

    pub fn resize(&self, cols: u16, rows: u16) -> Result<(), String> {
        let m = self.master.lock().map_err(|e| e.to_string())?;
        m.resize(PtySize {
            rows: rows.max(1),
            cols: cols.max(1),
            pixel_width: 0,
            pixel_height: 0,
        })
        .map_err(|e| format!("调整终端尺寸失败：{e}"))
    }

    pub fn kill(&self) {
        if let Ok(mut k) = self.killer.lock() {
            let _ = k.kill();
        }
    }
}

impl Drop for LocalPty {
    fn drop(&mut self) {
        self.kill();
    }
}

// Spawns a real POSIX shell, so Unix only.
#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::sync::mpsc;
    use std::time::Duration;

    #[test]
    fn runs_a_command_and_reports_exit() {
        let (data_tx, data_rx) = mpsc::channel::<Vec<u8>>();
        let (exit_tx, exit_rx) = mpsc::channel::<Option<u32>>();
        let pty = LocalPty::spawn(
            80,
            24,
            move |b| {
                let _ = data_tx.send(b);
            },
            move |c| {
                let _ = exit_tx.send(c);
            },
        )
        .expect("spawn");

        pty.write(b"echo LOKI_$((40+2)); exit 3\n").unwrap();
        let code = exit_rx
            .recv_timeout(Duration::from_secs(15))
            .expect("shell should exit");
        let out: Vec<u8> = data_rx.try_iter().flatten().collect();
        let text = String::from_utf8_lossy(&out);
        assert!(text.contains("LOKI_42"), "output was {text:?}");
        assert_eq!(code, Some(3));
    }
}
