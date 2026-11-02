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
        writer: crate::telnet::TelnetWriter,
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
            DirectSession::Telnet { writer, .. } => writer.write(data).await,
            DirectSession::Serial { handle, .. } => handle.write(data),
            DirectSession::Local { pty } => pty.write(data),
        }
    }

    /// A serial line has no window size; telnet uses NAWS.
    pub async fn resize(&self, cols: u16, rows: u16) -> Result<(), String> {
        match self {
            DirectSession::Telnet { writer, .. } => writer.resize(cols, rows).await,
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

/// The buffer SSH sessions use too: output before a pane attached (replayed
/// once), the recent screen for the assistant, and its live tap.
pub type Output = Arc<std::sync::Mutex<crate::ssh::OutputBuffer>>;

struct Entry {
    info: DirectInfo,
    session: Arc<DirectSession>,
    output: Output,
    /// Attached by `session_record_start`; the reader writes every chunk here.
    recorder: Arc<std::sync::Mutex<Option<crate::recorder::Recorder>>>,
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

    /// The session's display info, e.g. to name an exported file.
    pub fn info(&self, id: &str) -> Option<DirectInfo> {
        self.entries.get(id).map(|e| e.info.clone())
    }

    /// Where a recording can be attached for this session.
    pub fn recorder_slot(
        &self,
        id: &str,
    ) -> Option<Arc<std::sync::Mutex<Option<crate::recorder::Recorder>>>> {
        self.entries.get(id).map(|e| e.recorder.clone())
    }

    /// Every recorder slot, for shutdown.
    pub fn recorder_slots(&self) -> Vec<Arc<std::sync::Mutex<Option<crate::recorder::Recorder>>>> {
        self.entries.values().map(|e| e.recorder.clone()).collect()
    }

    /// Takes output produced before the UI attached, draining the buffer so a
    /// second call cannot duplicate it.
    pub fn take_backlog(&self, id: &str) -> Vec<u8> {
        self.entries
            .get(id)
            .and_then(|e| e.output.lock().ok().map(|mut b| b.take_pending()))
            .unwrap_or_default()
    }

    /// The session's output buffer, for the assistant to watch and read.
    pub fn output(&self, id: &str) -> Option<Output> {
        self.entries.get(id).map(|e| e.output.clone())
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
        let output = Output::default();
        let info = DirectInfo {
            id: id.clone(),
            status: "connected".to_string(),
            kind: "telnet".to_string(),
            target: format!("telnet {host}:{port}"),
            negotiation: Some(Negotiation::default()),
        };

        let stop = Arc::new(AtomicBool::new(false));
        let (mut reader, writer) = conn.split();
        let session = Arc::new(DirectSession::Telnet {
            writer,
            stop: stop.clone(),
        });

        let output_for_reader = output.clone();
        let recorder = Arc::new(std::sync::Mutex::new(None::<crate::recorder::Recorder>));
        let recorder_for_reader = recorder.clone();
        self.entries.insert(
            id.clone(),
            Entry {
                info: info.clone(),
                session,
                output,
                recorder,
            },
        );

        // Reader task: strip telnet commands, forward the rest. It owns the
        // read half outright, so keystrokes never wait behind a pending read.
        let sid = id.clone();
        tokio::spawn(async move {
            let mut neg = Negotiation::default();
            loop {
                if stop.load(Ordering::SeqCst) {
                    break;
                }
                // A read timeout keeps the loop responsive to `stop`.
                let result =
                    tokio::time::timeout(std::time::Duration::from_millis(250), reader.recv())
                        .await;

                match result {
                    Ok(Ok(Some(raw))) => {
                        let data = reader.process(&raw, &mut neg).await;
                        if data.is_empty() {
                            // Everything in the chunk was negotiation. Yielding
                            // matters: a peer that answers instantly would
                            // otherwise spin this loop at millions of turns a
                            // second and starve the runtime.
                            tokio::task::yield_now().await;
                            continue;
                        }
                        push_output(&output_for_reader, &data);
                        write_recording(&recorder_for_reader, &data);
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
            close_output(&output_for_reader);
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
        let output = Output::default();
        let output_for_reader = output.clone();

        let recorder = Arc::new(std::sync::Mutex::new(None::<crate::recorder::Recorder>));
        let recorder_for_reader = recorder.clone();
        let end_events = events.clone();
        let end_sid = id.clone();
        let end_output = output.clone();
        let (handle, stop) = conn.start(
            move |bytes| {
                push_output(&output_for_reader, &bytes);
                write_recording(&recorder_for_reader, &bytes);
                let payload = base64_encode(&bytes);
                let _ = events.send(TermEvent::Data {
                    session_id: sid.clone(),
                    data: payload,
                });
            },
            // An unplugged adapter ends the read with an error; without this
            // the tab would keep saying "connected" and swallow keystrokes.
            move || {
                close_output(&end_output);
                let _ = end_events.send(TermEvent::Closed {
                    session_id: end_sid,
                });
            },
        );

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
                output,
                recorder,
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
        let output = Output::default();
        let output_for_reader = output.clone();
        let recorder = Arc::new(std::sync::Mutex::new(None::<crate::recorder::Recorder>));
        let recorder_for_reader = recorder.clone();
        let sid = id.clone();
        let data_events = events.clone();
        let exit_sid = id.clone();
        let exit_output = output.clone();

        let pty = crate::local::LocalPty::spawn(
            cols,
            rows,
            move |bytes| {
                push_output(&output_for_reader, &bytes);
                write_recording(&recorder_for_reader, &bytes);
                let _ = data_events.send(TermEvent::Data {
                    session_id: sid.clone(),
                    data: base64_encode(&bytes),
                });
            },
            move |code| {
                close_output(&exit_output);
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
                output,
                recorder,
            },
        );
        Ok(info)
    }
}

/// Appends one chunk to the session's recorder, if one is attached.
///
/// Recording happens here rather than in the UI so a long `show running-config`
/// survives the scrollback limit and the pane being closed.
fn write_recording(slot: &std::sync::Mutex<Option<crate::recorder::Recorder>>, data: &[u8]) {
    if let Ok(mut guard) = slot.lock() {
        if let Some(rec) = guard.as_mut() {
            rec.write(data);
        }
    }
}

fn push_output(output: &Output, data: &[u8]) {
    if let Ok(mut b) = output.lock() {
        b.push(data);
    }
}

fn close_output(output: &Output) {
    if let Ok(mut b) = output.lock() {
        b.close();
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

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    #[tokio::test]
    async fn telnet_keystrokes_do_not_wait_for_the_reader() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let conn = DirectManager::connect_telnet("127.0.0.1", port, 132, 40)
            .await
            .unwrap();
        let (mut server, _) = listener.accept().await.unwrap();

        // The initial NAWS frame arrives first; skip it.
        let mut buf = [0u8; 64];
        let n = server.read(&mut buf).await.unwrap();
        assert_eq!(&buf[..3], &[255, 250, 31], "got {:?}", &buf[..n]);

        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel::<TermEvent>();
        let mut mgr = DirectManager::new();
        mgr.register_telnet("t".into(), "127.0.0.1".into(), port, conn, tx);
        let session = mgr.get("t").unwrap();

        // The server is silent, so the reader sits in a pending read. A
        // keystroke used to queue behind it for up to 250 ms each.
        tokio::time::sleep(std::time::Duration::from_millis(30)).await;
        for _ in 0..5 {
            let started = std::time::Instant::now();
            session.write(b"x").await.unwrap();
            assert!(
                started.elapsed() < std::time::Duration::from_millis(100),
                "write waited {:?}",
                started.elapsed()
            );
        }
        let mut got = Vec::new();
        while got.len() < 5 {
            let n = server.read(&mut buf).await.unwrap();
            got.extend_from_slice(&buf[..n]);
        }
        assert_eq!(got, b"xxxxx");

        // A switch asking for the window size gets WILL NAWS and the size.
        server.write_all(&[255, 253, 31]).await.unwrap();
        let mut reply = Vec::new();
        while reply.len() < 12 {
            let n = server.read(&mut buf).await.unwrap();
            reply.extend_from_slice(&buf[..n]);
        }
        assert_eq!(&reply[..3], &[255, 251, 31]);
        assert_eq!(&reply[3..12], &[255, 250, 31, 0, 132, 0, 40, 255, 240]);

        session.stop();
    }

    /// Reads from a fake device until `\r`, returning the line.
    async fn read_line(server: &mut tokio::net::TcpStream) -> String {
        let mut line = Vec::new();
        let mut byte = [0u8; 1];
        loop {
            if server.read(&mut byte).await.unwrap() == 0 || byte[0] == b'\r' {
                return String::from_utf8_lossy(&line).into_owned();
            }
            line.push(byte[0]);
        }
    }

    #[tokio::test]
    async fn the_assistant_types_into_a_telnet_switch() {
        use crate::ai::interactive::{type_line, Ended, Terminal};
        use std::time::Duration;

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let conn = DirectManager::connect_telnet("127.0.0.1", port, 80, 24)
            .await
            .unwrap();
        let (mut server, _) = listener.accept().await.unwrap();
        let mut naws = [0u8; 9];
        server.read_exact(&mut naws).await.unwrap();

        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel::<TermEvent>();
        let mut mgr = DirectManager::new();
        mgr.register_telnet("t".into(), "127.0.0.1".into(), port, conn, tx);
        let term = Terminal::Direct {
            session: mgr.get("t").unwrap(),
            output: mgr.output("t").unwrap(),
        };

        // A switch that echoes, pages its answer, then hangs up on `quit`.
        let device = tokio::spawn(async move {
            server.write_all(b"\r\n<sw01>").await.unwrap();
            let cmd = read_line(&mut server).await;
            assert_eq!(cmd, "display interface brief");
            server
                .write_all(b"display interface brief\r\nGE1/0/1 UP\r\n  ---- More ----")
                .await
                .unwrap();
            let mut key = [0u8; 1];
            server.read_exact(&mut key).await.unwrap();
            assert_eq!(&key, b" ");
            server
                .write_all(b"\x1b[16D                \x1b[16DGE1/0/2 DOWN\r\n<sw01>")
                .await
                .unwrap();
            assert_eq!(read_line(&mut server).await, "quit");
        });

        // Wait for the banner, as the user would before typing.
        for _ in 0..100 {
            if String::from_utf8_lossy(&term.screen()).contains("<sw01>") {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }

        let out = type_line(&term, "display interface brief", Duration::from_secs(5))
            .await
            .unwrap();
        assert_eq!(out.ended, Ended::Prompt("<sw01>".into()));
        assert_eq!(out.pages, 1);
        assert_eq!(out.output, "GE1/0/1 UP\nGE1/0/2 DOWN");

        // The device hanging up ends the wait at once, not at the timeout.
        let started = std::time::Instant::now();
        let out = type_line(&term, "quit", Duration::from_secs(5))
            .await
            .unwrap();
        assert_eq!(out.ended, Ended::Closed);
        assert!(started.elapsed() < Duration::from_secs(2));
        device.await.unwrap();
    }
}
