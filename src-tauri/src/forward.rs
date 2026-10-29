//! Port forwarding over an existing SSH connection.
//!
//! Local forwards listen on `127.0.0.1` and pipe each accepted connection to a
//! remote host:port through the session. SOCKS5 forwards do the same for
//! whatever address the client asks for.

use crate::ssh::LiveSession;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use uuid::Uuid;

/// A forward the user configured.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ForwardSpec {
    /// `local` (listen here, dial a fixed remote target) or `socks` (SOCKS5 on a local port).
    pub kind: String,
    /// Port to listen on, locally.
    pub local_port: u16,
    /// Only for `local` forwards.
    pub remote_host: Option<String>,
    pub remote_port: Option<u16>,
}

/// A running forward, as the UI sees it.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ForwardView {
    pub id: String,
    pub session_id: String,
    pub kind: String,
    /// The port actually bound (may differ from the request if it was 0).
    pub local_port: u16,
    pub remote: Option<String>,
}

/// A forward that is listening. Dropping the stop flag (setting it) closes it.
pub struct Running {
    pub view: ForwardView,
    pub stop: Arc<AtomicBool>,
}

pub struct ForwardManager {
    forwards: HashMap<String, Running>,
}

impl ForwardManager {
    pub fn new() -> Self {
        Self {
            forwards: HashMap::new(),
        }
    }

    pub fn list(&self) -> Vec<ForwardView> {
        self.forwards.values().map(|r| r.view.clone()).collect()
    }

    pub fn remove(&mut self, id: &str) -> bool {
        if let Some(running) = self.forwards.remove(id) {
            running.stop.store(true, Ordering::SeqCst);
            true
        } else {
            false
        }
    }

    /// Stops every forward belonging to a session.
    pub fn stop_for_session(&mut self, session_id: &str) {
        let ids: Vec<String> = self
            .forwards
            .values()
            .filter(|r| r.view.session_id == session_id)
            .map(|r| r.view.id.clone())
            .collect();
        for id in ids {
            self.remove(&id);
        }
    }

    pub fn insert(&mut self, running: Running) {
        self.forwards.insert(running.view.id.clone(), running);
    }
}

impl Default for ForwardManager {
    fn default() -> Self {
        Self::new()
    }
}

/// Starts a forward and returns the manager entry for it.
///
/// The listener is bound before this returns, so the caller can report the real
/// port (useful when the user asked for 0 to let the OS choose).
pub async fn start(
    session: Arc<LiveSession>,
    spec: ForwardSpec,
) -> Result<(ForwardView, Arc<AtomicBool>), String> {
    let listener = TcpListener::bind(("127.0.0.1", spec.local_port))
        .await
        .map_err(|e| format!("无法监听本地端口 {}：{e}", spec.local_port))?;
    let local_port = listener
        .local_addr()
        .map(|a| a.port())
        .map_err(|e| e.to_string())?;

    let stop = Arc::new(AtomicBool::new(false));
    let forward_id = Uuid::new_v4().to_string();

    let remote = if spec.kind == "local" {
        Some(format!(
            "{}:{}",
            spec.remote_host.clone().unwrap_or_default(),
            spec.remote_port.unwrap_or(0)
        ))
    } else {
        None
    };

    let view = ForwardView {
        id: forward_id.clone(),
        session_id: session.info.id.clone(),
        kind: spec.kind.clone(),
        local_port,
        remote: remote.clone(),
    };

    let stop_for_task = stop.clone();
    // Hoisted out of the accept loop: the per-connection task needs its own copy.
    let remote_host = spec.remote_host.clone().unwrap_or_default();
    let remote_port = spec.remote_port.unwrap_or(0);
    let kind_for_task = spec.kind.clone();

    tokio::spawn(async move {
        loop {
            if stop_for_task.load(Ordering::SeqCst) {
                break;
            }
            let (mut inbound, _) = match listener.accept().await {
                Ok(v) => v,
                Err(_) => break,
            };

            let session = session.clone();
            let kind = kind_for_task.clone();
            let host = remote_host.clone();
            let port = remote_port;
            tokio::spawn(async move {
                let (host, port) = if kind == "socks" {
                    match socks5_handshake(&mut inbound).await {
                        Ok(dest) => dest,
                        Err(_) => return,
                    }
                } else {
                    (host.clone(), port)
                };
                let channel = match session
                    .handle()
                    .channel_open_direct_tcpip(host, port as u32, "127.0.0.1", 0)
                    .await
                {
                    Ok(c) => c,
                    Err(_) => return,
                };

                let mut remote_stream = channel.into_stream();
                let _ = tokio::io::copy_bidirectional(&mut inbound, &mut remote_stream).await;
            });
        }
    });

    Ok((view, stop))
}

/// Minimal SOCKS5 negotiation: no-auth, CONNECT only.
///
/// Returns the requested destination so the caller can open the SSH channel.
async fn socks5_handshake(client: &mut TcpStream) -> Result<(String, u16), String> {
    let mut header = [0u8; 2];
    client
        .read_exact(&mut header)
        .await
        .map_err(|e| e.to_string())?;
    if header[0] != 5 {
        return Err("not socks5".into());
    }
    let mut methods = vec![0u8; header[1] as usize];
    client
        .read_exact(&mut methods)
        .await
        .map_err(|e| e.to_string())?;
    if !methods.contains(&0) {
        // Only "no authentication required" is offered.
        let _ = client.write_all(&[5, 0xFF]).await;
        return Err("no acceptable auth method".into());
    }
    client.write_all(&[5, 0]).await.map_err(|e| e.to_string())?;

    let mut req = [0u8; 4];
    client
        .read_exact(&mut req)
        .await
        .map_err(|e| e.to_string())?;
    if req[1] != 1 {
        // Only CONNECT is supported.
        let _ = client.write_all(&[5, 7, 0, 1, 0, 0, 0, 0, 0, 0]).await;
        return Err("only CONNECT is supported".into());
    }

    let host = match req[3] {
        1 => {
            let mut buf = [0u8; 4];
            client
                .read_exact(&mut buf)
                .await
                .map_err(|e| e.to_string())?;
            std::net::Ipv4Addr::from(buf).to_string()
        }
        3 => {
            let mut len = [0u8; 1];
            client
                .read_exact(&mut len)
                .await
                .map_err(|e| e.to_string())?;
            let mut name = vec![0u8; len[0] as usize];
            client
                .read_exact(&mut name)
                .await
                .map_err(|e| e.to_string())?;
            String::from_utf8_lossy(&name).to_string()
        }
        4 => {
            let mut buf = [0u8; 16];
            client
                .read_exact(&mut buf)
                .await
                .map_err(|e| e.to_string())?;
            std::net::Ipv6Addr::from(buf).to_string()
        }
        _ => return Err("unknown address type".into()),
    };

    let mut port = [0u8; 2];
    client
        .read_exact(&mut port)
        .await
        .map_err(|e| e.to_string())?;
    let port = u16::from_be_bytes(port);

    // Success, bound address 0.0.0.0:0 (the client does not use it here).
    client
        .write_all(&[5, 0, 0, 1, 0, 0, 0, 0, 0, 0])
        .await
        .map_err(|e| e.to_string())?;

    Ok((host, port))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stop_for_session_clears_only_that_session() {
        let mut mgr = ForwardManager::new();
        let mk = |id: &str, session: &str| Running {
            view: ForwardView {
                id: id.into(),
                session_id: session.into(),
                kind: "local".into(),
                local_port: 0,
                remote: None,
            },
            stop: Arc::new(AtomicBool::new(false)),
        };
        mgr.insert(mk("a", "s1"));
        mgr.insert(mk("b", "s1"));
        mgr.insert(mk("c", "s2"));

        mgr.stop_for_session("s1");
        assert_eq!(mgr.list().len(), 1);
        assert_eq!(mgr.list()[0].id, "c");
    }

    #[tokio::test]
    async fn local_forward_binds_a_port() {
        // Binding with port 0 lets the OS choose; that is what the UI shows when
        // the user leaves the port blank.
        let listener = TcpListener::bind(("127.0.0.1", 0u16)).await.unwrap();
        assert!(listener.local_addr().unwrap().port() > 0);
    }
}
