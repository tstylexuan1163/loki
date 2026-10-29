//! Telnet and serial sessions.
//!
//! These do not share a type with SSH, so they get their own manager. Both feed
//! the same `term-event` channel the SSH terminal uses, which is why the UI needs
//! only one terminal component.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use crate::ssh::TermEvent;
use crate::telnet::Negotiation;

/// How to reach a non-SSH target.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum DirectTarget {
    Telnet {
        host: String,
        port: u16,
    },
    Serial {
        path: String,
        baud: u32,
        #[serde(default = "default_data_bits")]
        data_bits: u8,
        /// `none` | `odd` | `even`
        #[serde(default = "default_parity")]
        parity: String,
        #[serde(default = "default_stop_bits")]
        stop_bits: u8,
    },
}

fn default_data_bits() -> u8 {
    8
}
fn default_parity() -> String {
    "none".to_string()
}
fn default_stop_bits() -> u8 {
    1
}

/// Framing chosen for a serial session.
#[derive(Debug, Clone)]
pub struct SerialOptions {
    pub path: String,
    pub baud: u32,
    pub data_bits: u8,
    pub parity: String,
    pub stop_bits: u8,
}

/// A connection the terminal can write to and resize.
pub enum DirectSession {
    Telnet {
        conn: tokio::sync::Mutex<crate::telnet::TelnetConnection>,
        stop: Arc<AtomicBool>,
    },
    Serial {
        handle: Arc<crate::serial::SerialHandle>,
        stop: Arc<AtomicBool>,
    },
    Local {
        pty: crate::local::LocalPty,
    },
}

impl DirectSession {
    pub fn stop(&self) {
        match self {
            DirectSession::Telnet { stop, .. } => stop.store(true, Ordering::SeqCst),
            DirectSession::Serial { stop, .. } => stop.store(true, Ordering::SeqCst),
            DirectSession::Local { pty } => pty.kill(),
        }
    }

    pub async fn write(&self, data: &[u8]) -> Result<(), String> {
        match self {
            DirectSession::Telnet { conn, .. } => conn.lock().await.write(data).await,
            DirectSession::Serial { handle, .. } => handle.write(data),
            DirectSession::Local { pty } => pty.write(data),
        }
    }

    /// A serial line has no window size; telnet uses NAWS.
    pub async fn resize(&self, cols: u16, rows: u16) -> Result<(), String> {
        match self {
            DirectSession::Telnet { conn, .. } => conn.lock().await.resize(cols, rows).await,
            DirectSession::Serial { .. } => Ok(()),
            DirectSession::Local { pty } => pty.resize(cols, rows),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DirectInfo {
    pub id: String,
    pub status: String,
    /// `telnet`, `serial` or `local`.
    pub kind: String,
    /// Human description, e.g. `telnet 10.0.0.1:23`.
    pub target: String,
    /// What the telnet negotiation agreed on, if any.
    pub negotiation: Option<Negotiation>,
}

struct Entry {
    info: DirectInfo,
    session: Arc<DirectSession>,
    /// Output produced before a pane attached, replayed once on request.
    backlog: Arc<std::sync::Mutex<Vec<u8>>>,
}

pub struct DirectManager {
    entries: HashMap<String, Entry>,
}

impl DirectManager {
    pub fn new() -> Self {
        Self {
            entries: HashMap::new(),
        }
    }

    pub fn list(&self) -> Vec<DirectInfo> {
        self.entries.values().map(|e| e.info.clone()).collect()
    }

    pub fn get(&self, id: &str) -> Option<Arc<DirectSession>> {
        self.entries.get(id).map(|e| e.session.clone())
    }

    pub fn remove(&mut self, id: &str) -> bool {
        if let Some(entry) = self.entries.remove(id) {
            entry.session.stop();
            true
        } else {
            false
        }
    }

    /// Takes output produced before the UI attached, draining the buffer so a
    /// second call cannot duplicate it.
    pub fn take_backlog(&self, id: &str) -> Vec<u8> {
        self.entries
            .get(id)
            .and_then(|e| e.backlog.lock().ok().map(|mut b| std::mem::take(&mut *b)))
            .unwrap_or_default()
    }

    pub fn stop_all(&mut self) {
        for entry in self.entries.values() {
            entry.session.stop();
        }
        self.entries.clear();
    }

    /// Connects to a telnet server.
    ///
    /// Separate from [`DirectManager::register_telnet`] so the caller does not
    /// hold the manager's lock across the connect: a lock held over an await is
    /// not `Send` and would block every other command meanwhile.
    pub async fn connect_telnet(
        host: &str,
        port: u16,
        cols: u16,
        rows: u16,
    ) -> Result<crate::telnet::TelnetConnection, String> {
        let mut conn = crate::telnet::TelnetConnection::connect(host, port).await?;
        // Tell the server our size before anything else so full-screen apps lay
        // out correctly from the first frame.
        let _ = conn.resize(cols, rows).await;
        Ok(conn)
    }

    /// Registers an already-connected telnet session and starts its reader.
    pub fn register_telnet(
        &mut self,
        id: String,
        host: String,
        port: u16,
        conn: crate::telnet::TelnetConnection,
        events: tokio::sync::mpsc::UnboundedSender<TermEvent>,
    ) -> DirectInfo {
        let backlog = Arc::new(std::sync::Mutex::new(Vec::<u8>::new()));
        let info = DirectInfo {
            id: id.clone(),
            status: "connected".to_string(),
            kind: "telnet".to_string(),
            target: format!("telnet {host}:{port}"),
            negotiation: Some(Negotiation::default()),
        };

        let stop = Arc::new(AtomicBool::new(false));
        let session = Arc::new(DirectSession::Telnet {
            conn: tokio::sync::Mutex::new(conn),
            stop: stop.clone(),
        });

        let session_for_reader = session.clone();
        let backlog_for_reader = backlog.clone();
        self.entries.insert(
            id.clone(),
            Entry {
                info: info.clone(),
                session,
                backlog,
            },
        );

        // Reader task: strip telnet commands, forward the rest. The connection
        // lives in an Arc<Mutex>, so each read takes the lock briefly and the
        // writer never waits on a blocked read for long.
        let sid = id.clone();
        tokio::spawn(async move {
            let mut neg = Negotiation::default();
            loop {
                if stop.load(Ordering::SeqCst) {
                    break;
                }
                let DirectSession::Telnet { conn, .. } = &*session_for_reader else {
                    break;
                };
                let result = {
                    let mut guard = conn.lock().await;
                    // A read timeout keeps the loop responsive to `stop`.
                    tokio::time::timeout(
                        std::time::Duration::from_millis(250),
                        guard.read(&mut neg),
                    )
                    .await
                };

                match result {
                    Ok(Ok(Some(data))) => {
                        if data.is_empty() {
                            // Everything in the chunk was negotiation. Yielding
                            // matters: a peer that answers instantly would
                            // otherwise spin this loop at millions of turns a
                            // second and starve the runtime.
                            tokio::task::yield_now().await;
                            continue;
                        }
                        push_bounded(&backlog_for_reader, &data);
                        let payload = base64_encode(&data);
                        if events
                            .send(TermEvent::Data {
                                session_id: sid.clone(),
                                data: payload,
                            })
                            .is_err()
                        {
                            break;
                        }
                    }
                    Ok(Ok(None)) => break,
                    Ok(Err(_)) => break,
                    // Timed out waiting for bytes; loop and re-check `stop`.
                    Err(_) => continue,
                }
            }
            let _ = events.send(TermEvent::Closed { session_id: sid });
        });

        info
    }

    /// Opens a serial port and forwards its bytes to `events`.
    pub fn open_serial(
        &mut self,
        id: String,
        opts: SerialOptions,
        events: tokio::sync::mpsc::UnboundedSender<TermEvent>,
    ) -> Result<DirectInfo, String> {
        let SerialOptions {
            path,
            baud,
            data_bits,
            parity,
            stop_bits,
        } = opts;
        let parity = parity.as_str();
        let config = crate::serial::SerialConfig {
            baud,
            data_bits: match data_bits {
                5 => crate::serial::DataBits::Five,
                6 => crate::serial::DataBits::Six,
                7 => crate::serial::DataBits::Seven,
                _ => crate::serial::DataBits::Eight,
            },
            parity: match parity {
                "odd" => crate::serial::Parity::Odd,
                "even" => crate::serial::Parity::Even,
                _ => crate::serial::Parity::None,
            },
            stop_bits: if stop_bits == 2 {
                crate::serial::StopBits::Two
            } else {
                crate::serial::StopBits::One
            },
        };

        let conn = crate::serial::SerialConnection::open(&path, config)?;
        let sid = id.clone();
        let backlog = Arc::new(std::sync::Mutex::new(Vec::<u8>::new()));
        let backlog_for_reader = backlog.clone();

        let (handle, stop) = conn.start(move |bytes| {
            push_bounded(&backlog_for_reader, &bytes);
            let payload = base64_encode(&bytes);
            let _ = events.send(TermEvent::Data {
                session_id: sid.clone(),
                data: payload,
            });
        });

        let info = DirectInfo {
            id: id.clone(),
            status: "connected".to_string(),
            kind: "serial".to_string(),
            target: format!("{path} @ {baud}"),
            negotiation: None,
        };

        self.entries.insert(
            id,
            Entry {
                info: info.clone(),
                session: Arc::new(DirectSession::Serial {
                    handle: Arc::new(handle),
                    stop,
                }),
                backlog,
            },
        );

        Ok(info)
    }

    /// Starts a shell on this machine.
    pub fn open_local(
        &mut self,
        id: String,
        cols: u16,
        rows: u16,
        events: tokio::sync::mpsc::UnboundedSender<TermEvent>,
    ) -> Result<DirectInfo, String> {
        let backlog = Arc::new(std::sync::Mutex::new(Vec::<u8>::new()));
        let backlog_for_reader = backlog.clone();
        let sid = id.clone();
        let data_events = events.clone();
        let exit_sid = id.clone();

        let pty = crate::local::LocalPty::spawn(
            cols,
            rows,
            move |bytes| {
                push_bounded(&backlog_for_reader, &bytes);
                let _ = data_events.send(TermEvent::Data {
                    session_id: sid.clone(),
                    data: base64_encode(&bytes),
                });
            },
            move |code| {
                if let Some(code) = code {
                    let _ = events.send(TermEvent::Exit {
                        session_id: exit_sid.clone(),
                        code,
                    });
                }
                let _ = events.send(TermEvent::Closed {
                    session_id: exit_sid,
                });
            },
        )?;

        let shell_name = std::path::Path::new(&pty.shell)
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| pty.shell.clone());
        let info = DirectInfo {
            id: id.clone(),
            status: "connected".to_string(),
            kind: "local".to_string(),
            target: shell_name,
            negotiation: None,
        };
        self.entries.insert(
            id,
            Entry {
                info: info.clone(),
                session: Arc::new(DirectSession::Local { pty }),
                backlog,
            },
        );
        Ok(info)
    }
}

/// Replay buffers stop growing at 256 KB; a noisy console must not grow them
/// forever while no pane is attached.
fn push_bounded(buf: &std::sync::Mutex<Vec<u8>>, data: &[u8]) {
    if let Ok(mut b) = buf.lock() {
        let room = (256 * 1024usize).saturating_sub(b.len());
        b.extend_from_slice(&data[..data.len().min(room)]);
    }
}

fn base64_encode(data: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(data)
}

impl Default for DirectManager {
    fn default() -> Self {
        Self::new()
    }
}
