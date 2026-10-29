//! SSH sessions: connect, authenticate, open a PTY shell, and stream both ways.
//!
//! Security note: `check_server_key` in russh defaults to rejecting every key.
//! We keep that strictness — a host key is accepted only when it is already in
//! the vault's known-hosts list, or when the caller passes the exact
//! fingerprint the user just confirmed. A *changed* key for a known host is
//! reported separately so the UI can warn loudly instead of offering the same
//! one-click prompt as a first contact.

use crate::commands::HostView;
use base64::Engine;
use russh::client::{self, Handle, KeyboardInteractiveAuthResponse, Msg};
use russh::keys::ssh_key::PublicKey;
use russh::keys::{HashAlg, PrivateKeyWithHashAlg};
use russh::{ChannelMsg, MethodKind};
use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use tokio::sync::mpsc;

/// Terminal events pushed from Rust to the WebView.
///
/// `rename_all` only renames the variants; the fields need
/// `rename_all_fields`, or the WebView receives `session_id` while it filters
/// on `sessionId` and silently drops every frame.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum TermEvent {
    /// Shell output. Base64 keeps arbitrary bytes intact across the IPC hop.
    Data {
        session_id: String,
        data: String,
    },
    Exit {
        session_id: String,
        code: u32,
    },
    Closed {
        session_id: String,
    },
}

/// What the UI knows about a session. No credentials, ever.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Session {
    pub id: String,
    pub title: String,
    pub status: String,
    pub host_id: String,
    /// The server's host key fingerprint, so the user can verify it.
    pub fingerprint: Option<String>,
}

/// Caps the replay buffer so a chatty session cannot grow it without bound.
const BACKLOG_LIMIT: usize = 256 * 1024;
/// How much recent output the assistant may look at.
const TAIL_LIMIT: usize = 64 * 1024;

/// Output kept on the Rust side of a terminal.
///
/// Two buffers with different jobs: `pending` holds what arrived before the
/// pane attached and is handed over exactly once; `tail` is a rolling window of
/// the most recent output for `read_terminal`, which must see what is on the
/// screen *now*, not the login banner.
#[derive(Default)]
pub struct OutputBuffer {
    pending: Vec<u8>,
    attached: bool,
    tail: VecDeque<u8>,
}

impl OutputBuffer {
    pub fn push(&mut self, data: &[u8]) {
        if !self.attached {
            let room = BACKLOG_LIMIT.saturating_sub(self.pending.len());
            self.pending
                .extend_from_slice(&data[..data.len().min(room)]);
        }
        self.tail.extend(data.iter().copied());
        let excess = self.tail.len().saturating_sub(TAIL_LIMIT);
        if excess > 0 {
            self.tail.drain(..excess);
        }
    }

    /// Hands over what arrived before the pane existed, once.
    pub fn take_pending(&mut self) -> Vec<u8> {
        self.attached = true;
        std::mem::take(&mut self.pending)
    }

    pub fn tail(&self) -> Vec<u8> {
        self.tail.iter().copied().collect()
    }
}

/// A live shell: the write half for input, plus the connection handle.
pub struct LiveSession {
    pub info: Session,
    pub(crate) handle: Arc<Handle<ClientHandler>>,
    /// A jump host's connection, kept alive for as long as this session uses it.
    _jump: Option<Arc<Handle<ClientHandler>>>,
    writer: tokio::sync::Mutex<russh::ChannelWriteHalf<Msg>>,
    output: Arc<Mutex<OutputBuffer>>,
    closed: Arc<AtomicBool>,
    /// Attached by `session_record_start`; the pump writes every chunk here.
    recorder: Arc<Mutex<Option<crate::recorder::Recorder>>>,
}

impl LiveSession {
    /// Where a recording can be attached; the pump writes to it as it runs.
    pub fn recorder_slot(&self) -> Arc<Mutex<Option<crate::recorder::Recorder>>> {
        self.recorder.clone()
    }

    /// The underlying connection, for opening extra subsystems (SFTP, forwarding).
    pub fn handle(&self) -> &Arc<Handle<ClientHandler>> {
        &self.handle
    }

    /// A copy of the recent output, for `read_terminal`.
    ///
    /// Unlike [`LiveSession::take_backlog`] this does not drain: the assistant
    /// looking at the screen must not steal what the UI has not drawn yet.
    pub fn peek_backlog(&self) -> Vec<u8> {
        self.output.lock().map(|b| b.tail()).unwrap_or_default()
    }

    /// Takes the output received before the pane attached.
    ///
    /// Draining (rather than copying) means a pane that fetches twice cannot
    /// print the same banner twice; the live event stream covers everything
    /// after this call.
    pub fn take_backlog(&self) -> Vec<u8> {
        self.output
            .lock()
            .map(|mut b| b.take_pending())
            .unwrap_or_default()
    }

    /// Whether the shell channel has ended.
    pub fn is_closed(&self) -> bool {
        self.closed.load(Ordering::SeqCst)
    }

    /// Sends keystrokes or a paste to the remote shell.
    ///
    /// `ChannelWriteHalf` has no `AsyncWrite` impl; its `data` method takes a
    /// reader and handles windowing, so the bytes go through a `&[u8]` reader.
    pub async fn write(&self, data: &[u8]) -> Result<(), String> {
        let w = self.writer.lock().await;
        w.data(data).await.map_err(|e| format!("写入失败：{e}"))
    }

    /// Tells the remote PTY the terminal changed size.
    pub async fn resize(&self, cols: u32, rows: u32) -> Result<(), String> {
        let w = self.writer.lock().await;
        w.window_change(cols.max(1), rows.max(1), 0, 0)
            .await
            .map_err(|e| format!("调整终端尺寸失败：{e}"))
    }
}

/// A host key already trusted for some `host:port`.
#[derive(Debug, Clone)]
pub struct KnownKey {
    pub key_type: String,
    pub fingerprint: String,
}

/// The key a server presented, recorded by the handler.
#[derive(Debug, Clone)]
pub struct SeenKey {
    pub host: String,
    pub port: u16,
    pub key_type: String,
    pub fingerprint: String,
}

/// Client handler that captures the server key and refuses unknown hosts.
pub struct ClientHandler {
    seen: Arc<Mutex<Option<SeenKey>>>,
    host: String,
    port: u16,
    known: Vec<KnownKey>,
    /// The fingerprint the user has just accepted, if any.
    accept: Option<String>,
}

impl client::Handler for ClientHandler {
    type Error = russh::Error;

    async fn check_server_key(&mut self, key: &PublicKey) -> Result<bool, Self::Error> {
        let fp = fingerprint(key);
        let key_type = key.algorithm().as_str().to_string();
        *self.seen.lock().unwrap() = Some(SeenKey {
            host: self.host.clone(),
            port: self.port,
            key_type,
            fingerprint: fp.clone(),
        });
        // Only an exact match is accepted: either a key the vault already
        // trusts, or the one the user confirmed for this attempt.
        Ok(self.known.iter().any(|k| k.fingerprint == fp) || self.accept.as_deref() == Some(&fp))
    }
}

/// `SHA256:...` in the same form OpenSSH prints.
pub fn fingerprint(key: &PublicKey) -> String {
    key.fingerprint(HashAlg::Sha256).to_string()
}

/// How to authenticate. The caller builds this from the vault; the password or
/// key never leaves this process.
#[derive(Clone)]
pub enum Credential {
    Password(String),
    /// An `OpenSSH` private key in PEM/OpenSSH format, plus an optional passphrase.
    Key {
        pem: String,
        passphrase: Option<String>,
    },
    /// Whatever identities the local ssh-agent holds.
    Agent,
}

/// Where to connect, and how to prove who we are.
#[derive(Clone)]
pub struct Hop {
    pub address: String,
    pub port: u16,
    pub username: String,
    /// Tried in order until one succeeds.
    pub credentials: Vec<Credential>,
    pub known: Vec<KnownKey>,
}

/// The outcome of an attempted connection.
pub enum ConnectOutcome {
    /// The shell is up. `trusted` is the key that was newly accepted on this
    /// attempt and should be saved, if any.
    Live(Arc<LiveSession>),
    /// A server presented a key the vault does not trust yet. `previous` is
    /// set when the host was known under a different key of the same type,
    /// which is what a man-in-the-middle looks like.
    UnknownHostKey {
        fingerprint: String,
        key_type: String,
        host: String,
        port: u16,
        previous: Option<String>,
    },
    /// Nothing in the credential list worked and no password was available;
    /// the UI should ask for one.
    NeedPassword { host: String, username: String },
}

pub struct SshManager {
    sessions: HashMap<String, Arc<LiveSession>>,
}

impl SshManager {
    pub fn new() -> Self {
        Self {
            sessions: HashMap::new(),
        }
    }

    pub fn list(&self) -> Vec<Session> {
        self.sessions.values().map(|s| s.info.clone()).collect()
    }

    pub fn get(&self, id: &str) -> Option<Arc<LiveSession>> {
        self.sessions.get(id).cloned()
    }

    pub fn remove(&mut self, id: &str) -> Option<Arc<LiveSession>> {
        self.sessions.remove(id)
    }

    pub fn insert(&mut self, session: Arc<LiveSession>) {
        self.sessions.insert(session.info.id.clone(), session);
    }

    pub fn drain(&mut self) -> Vec<Arc<LiveSession>> {
        self.sessions.drain().map(|(_, s)| s).collect()
    }
}

impl Default for SshManager {
    fn default() -> Self {
        Self::new()
    }
}

/// Connection settings shared by every hop.
fn client_config() -> Arc<client::Config> {
    Arc::new(client::Config {
        inactivity_timeout: None,
        // A NAT box that forgets idle flows is the usual reason a terminal
        // "freezes"; a keepalive every 30 s keeps the mapping warm and lets a
        // dead peer be noticed after ~2 minutes.
        keepalive_interval: Some(std::time::Duration::from_secs(30)),
        keepalive_max: 3,
        nodelay: true,
        ..Default::default()
    })
}

/// What a single handshake produced.
enum HopResult {
    Authed(Handle<ClientHandler>, SeenKey),
    HostKey(SeenKey),
    NeedPassword,
}

const CONNECT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(15);

/// Handshakes with one hop, either directly or through an earlier hop's tunnel.
async fn open_hop(
    hop: &Hop,
    accept: Option<&str>,
    via: Option<&Handle<ClientHandler>>,
) -> Result<HopResult, String> {
    let seen = Arc::new(Mutex::new(None));
    let handler = ClientHandler {
        seen: seen.clone(),
        host: hop.address.clone(),
        port: hop.port,
        known: hop.known.clone(),
        accept: accept.map(String::from),
    };
    let addr = format!("{}:{}", hop.address, hop.port);

    let connected = match via {
        None => {
            tokio::time::timeout(
                CONNECT_TIMEOUT,
                client::connect(client_config(), addr.clone(), handler),
            )
            .await
        }
        Some(jump) => {
            // The next hop rides on a direct-tcpip channel of the previous
            // one; the channel is a byte stream, so a second handshake runs
            // over it exactly as it would over TCP.
            let channel = jump
                .channel_open_direct_tcpip(hop.address.clone(), hop.port as u32, "127.0.0.1", 0)
                .await
                .map_err(|e| format!("跳板机无法连到 {addr}：{e}"))?;
            tokio::time::timeout(
                CONNECT_TIMEOUT,
                client::connect_stream(client_config(), channel.into_stream(), handler),
            )
            .await
        }
    };

    let mut handle = match connected {
        Err(_) => return Err(format!("连接 {addr} 超时")),
        Ok(Err(e)) => {
            // A refused host key lands here; report it so the UI can ask.
            if let Some(key) = seen.lock().unwrap().clone() {
                let trusted = hop.known.iter().any(|k| k.fingerprint == key.fingerprint)
                    || accept == Some(key.fingerprint.as_str());
                if !trusted {
                    return Ok(HopResult::HostKey(key));
                }
            }
            return Err(describe_connect_error(&addr, &e));
        }
        Ok(Ok(h)) => h,
    };

    let key = seen
        .lock()
        .unwrap()
        .clone()
        .ok_or_else(|| "服务器没有提供主机密钥".to_string())?;

    match authenticate(&mut handle, &hop.username, &hop.credentials).await? {
        AuthOutcome::Success => Ok(HopResult::Authed(handle, key)),
        AuthOutcome::NeedPassword => Ok(HopResult::NeedPassword),
        AuthOutcome::Failed(reason) => Err(reason),
    }
}

fn describe_connect_error(addr: &str, e: &russh::Error) -> String {
    let text = e.to_string();
    let lower = text.to_lowercase();
    if lower.contains("refused") {
        format!("{addr} 拒绝连接：端口没有开放或 sshd 没有运行")
    } else if lower.contains("no route") || lower.contains("unreachable") {
        format!("{addr} 不可达：检查网络或地址")
    } else if lower.contains("lookup") || lower.contains("resolve") || lower.contains("nodename") {
        format!("无法解析 {addr}：检查主机名")
    } else {
        format!("连接 {addr} 失败：{text}")
    }
}

enum AuthOutcome {
    Success,
    NeedPassword,
    Failed(String),
}

/// Tries each credential in turn.
///
/// Every attempt costs one of the server's `MaxAuthTries`, so the order is the
/// one `ssh` itself uses: explicit key, agent, password.
async fn authenticate(
    handle: &mut Handle<ClientHandler>,
    user: &str,
    credentials: &[Credential],
) -> Result<AuthOutcome, String> {
    let mut notes: Vec<String> = Vec::new();
    let mut had_password = false;

    for cred in credentials {
        match cred {
            Credential::Key { pem, passphrase } => {
                let pem = normalize_private_key(pem);
                let key = match russh::keys::decode_secret_key(&pem, passphrase.as_deref()) {
                    Ok(k) => k,
                    Err(e) => {
                        notes.push(format!("私钥无法解析（{e}）"));
                        continue;
                    }
                };
                let hash = handle
                    .best_supported_rsa_hash()
                    .await
                    .ok()
                    .flatten()
                    .flatten();
                let key = PrivateKeyWithHashAlg::new(Arc::new(key), hash);
                let res = handle
                    .authenticate_publickey(user.to_string(), key)
                    .await
                    .map_err(|e| format!("认证过程出错：{e}"))?;
                if res.success() {
                    return Ok(AuthOutcome::Success);
                }
                notes.push("私钥被拒绝".into());
            }
            Credential::Agent => match try_agent(handle, user).await {
                Ok(true) => return Ok(AuthOutcome::Success),
                Ok(false) => notes.push("ssh-agent 中的密钥都被拒绝".into()),
                // No agent is a normal situation, not worth a note per attempt.
                Err(_) => {}
            },
            Credential::Password(pw) => {
                had_password = true;
                let res = handle
                    .authenticate_password(user.to_string(), pw.clone())
                    .await
                    .map_err(|e| format!("认证过程出错：{e}"))?;
                if res.success() {
                    return Ok(AuthOutcome::Success);
                }
                // PAM-only servers turn `password` off and ask the same
                // question through keyboard-interactive instead.
                let ki_allowed = match &res {
                    russh::client::AuthResult::Failure {
                        remaining_methods, ..
                    } => remaining_methods.contains(&MethodKind::KeyboardInteractive),
                    _ => false,
                };
                if ki_allowed && keyboard_interactive(handle, user, pw).await? {
                    return Ok(AuthOutcome::Success);
                }
                notes.push("密码被拒绝".into());
            }
        }
    }

    if !had_password {
        return Ok(AuthOutcome::NeedPassword);
    }
    Ok(AuthOutcome::Failed(if notes.is_empty() {
        "认证失败".into()
    } else {
        format!("认证失败：{}", notes.join("；"))
    }))
}

/// Answers keyboard-interactive prompts with the saved password.
///
/// Only a prompt that looks like a password question gets the password; a
/// one-time code prompt is left unanswered rather than guessed.
async fn keyboard_interactive(
    handle: &mut Handle<ClientHandler>,
    user: &str,
    password: &str,
) -> Result<bool, String> {
    let mut reply = handle
        .authenticate_keyboard_interactive_start(user.to_string(), None)
        .await
        .map_err(|e| format!("认证过程出错：{e}"))?;
    for _ in 0..10 {
        match reply {
            KeyboardInteractiveAuthResponse::Success => return Ok(true),
            KeyboardInteractiveAuthResponse::Failure { .. } => return Ok(false),
            KeyboardInteractiveAuthResponse::InfoRequest { prompts, .. } => {
                if prompts.len() > 1
                    || prompts
                        .first()
                        .is_some_and(|p| !p.prompt.to_lowercase().contains("password"))
                {
                    return Ok(false);
                }
                let answers = prompts.iter().map(|_| password.to_string()).collect();
                reply = handle
                    .authenticate_keyboard_interactive_respond(answers)
                    .await
                    .map_err(|e| format!("认证过程出错：{e}"))?;
            }
        }
    }
    Ok(false)
}

/// Finds the agent socket.
///
/// An app started from Finder or the Dock does not inherit the shell's
/// environment, so `SSH_AUTH_SOCK` is usually missing; launchd still knows
/// where the system agent listens.
#[cfg(unix)]
async fn agent_socket() -> Option<String> {
    if let Ok(sock) = std::env::var("SSH_AUTH_SOCK") {
        if !sock.is_empty() && std::path::Path::new(&sock).exists() {
            return Some(sock);
        }
    }
    #[cfg(target_os = "macos")]
    {
        let out = tokio::process::Command::new("launchctl")
            .args(["getenv", "SSH_AUTH_SOCK"])
            .output()
            .await
            .ok()?;
        let sock = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if !sock.is_empty() && std::path::Path::new(&sock).exists() {
            return Some(sock);
        }
    }
    None
}

#[cfg(unix)]
async fn try_agent(handle: &mut Handle<ClientHandler>, user: &str) -> Result<bool, String> {
    use russh::keys::agent::client::AgentClient;

    let sock = agent_socket().await.ok_or("没有找到 ssh-agent")?;
    let agent = AgentClient::connect_uds(&sock)
        .await
        .map_err(|e| format!("无法连接 ssh-agent：{e}"))?;
    try_identities(handle, user, agent).await
}

/// Windows: the OpenSSH agent service listens on a named pipe; PuTTY users
/// run Pageant instead, so it is the fallback.
#[cfg(windows)]
async fn try_agent(handle: &mut Handle<ClientHandler>, user: &str) -> Result<bool, String> {
    use russh::keys::agent::client::AgentClient;

    let pipe = std::env::var("SSH_AUTH_SOCK")
        .ok()
        .filter(|p| p.starts_with(r"\\.\pipe\"))
        .unwrap_or_else(|| r"\\.\pipe\openssh-ssh-agent".to_string());
    match AgentClient::connect_named_pipe(&pipe).await {
        Ok(agent) => try_identities(handle, user, agent).await,
        Err(_) => {
            let agent = AgentClient::connect_pageant().await;
            try_identities(handle, user, agent).await
        }
    }
}

#[cfg(not(any(unix, windows)))]
async fn try_agent(_handle: &mut Handle<ClientHandler>, _user: &str) -> Result<bool, String> {
    Err("此平台暂不支持 ssh-agent".into())
}

/// Offers each identity the agent holds until the server accepts one.
#[cfg(any(unix, windows))]
async fn try_identities<R>(
    handle: &mut Handle<ClientHandler>,
    user: &str,
    mut agent: russh::keys::agent::client::AgentClient<R>,
) -> Result<bool, String>
where
    R: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static,
{
    let identities = agent
        .request_identities()
        .await
        .map_err(|e| format!("读取 ssh-agent 密钥失败：{e}"))?;
    let hash = handle
        .best_supported_rsa_hash()
        .await
        .ok()
        .flatten()
        .flatten();
    for key in identities {
        let res = handle
            .authenticate_publickey_with(user.to_string(), key, hash, &mut agent)
            .await;
        if let Ok(r) = res {
            if r.success() {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

/// The result of [`connect_chain`]: the live session plus every key accepted on
/// this attempt, which the caller persists as known hosts.
pub struct Connected {
    pub outcome: ConnectOutcome,
    pub accepted: Vec<SeenKey>,
}

/// Connects through zero or more jump hosts to `target`, then opens a PTY shell.
///
/// `accept` is the fingerprint the user has just confirmed; it applies to
/// whichever hop presents it, so a prompt for a jump host and one for the
/// target are answered the same way.
#[allow(clippy::too_many_arguments)]
pub async fn connect_chain(
    jump: Option<&Hop>,
    target: &Hop,
    host_id: &str,
    title: &str,
    accept: Option<&str>,
    cols: u32,
    rows: u32,
    events: mpsc::UnboundedSender<TermEvent>,
) -> Result<Connected, String> {
    let mut accepted = Vec::new();
    let is_new =
        |hop: &Hop, key: &SeenKey| !hop.known.iter().any(|k| k.fingerprint == key.fingerprint);

    let jump_handle = match jump {
        None => None,
        Some(j) => match open_hop(j, accept, None).await? {
            HopResult::HostKey(key) => return Ok(host_key_outcome(j, key)),
            HopResult::NeedPassword => {
                return Ok(Connected {
                    outcome: ConnectOutcome::NeedPassword {
                        host: j.address.clone(),
                        username: j.username.clone(),
                    },
                    accepted,
                })
            }
            HopResult::Authed(h, key) => {
                if is_new(j, &key) {
                    accepted.push(key);
                }
                Some(Arc::new(h))
            }
        },
    };

    let (handle, key) = match open_hop(target, accept, jump_handle.as_deref()).await? {
        HopResult::HostKey(key) => {
            let mut out = host_key_outcome(target, key);
            out.accepted = accepted;
            return Ok(out);
        }
        HopResult::NeedPassword => {
            return Ok(Connected {
                outcome: ConnectOutcome::NeedPassword {
                    host: target.address.clone(),
                    username: target.username.clone(),
                },
                accepted,
            })
        }
        HopResult::Authed(h, key) => (h, key),
    };
    if is_new(target, &key) {
        accepted.push(key.clone());
    }

    let session = open_shell(
        handle,
        jump_handle,
        host_id,
        title,
        Some(key.fingerprint),
        cols,
        rows,
        events,
    )
    .await?;
    Ok(Connected {
        outcome: ConnectOutcome::Live(session),
        accepted,
    })
}

fn host_key_outcome(hop: &Hop, key: SeenKey) -> Connected {
    // Same algorithm, different key: the server changed or someone is in the
    // middle. A different algorithm is just a key we have not seen.
    let previous = hop
        .known
        .iter()
        .find(|k| k.key_type == key.key_type)
        .map(|k| k.fingerprint.clone());
    Connected {
        outcome: ConnectOutcome::UnknownHostKey {
            fingerprint: key.fingerprint,
            key_type: key.key_type,
            host: key.host,
            port: key.port,
            previous,
        },
        accepted: Vec::new(),
    }
}

/// Terminal modes sent with the PTY request. Without them some servers start
/// the shell in a mode where Ctrl-C or UTF-8 line editing misbehaves.
fn terminal_modes() -> Vec<(russh::Pty, u32)> {
    use russh::Pty;
    vec![
        (Pty::VINTR, 3),
        (Pty::VERASE, 127),
        (Pty::VEOF, 4),
        (Pty::VSUSP, 26),
        (Pty::ICRNL, 1),
        (Pty::IUTF8, 1),
        (Pty::ISIG, 1),
        (Pty::ICANON, 1),
        (Pty::ECHO, 1),
        (Pty::ECHOE, 1),
        (Pty::ECHOK, 1),
        (Pty::IEXTEN, 1),
        (Pty::OPOST, 1),
        (Pty::ONLCR, 1),
        (Pty::CS8, 1),
        (Pty::TTY_OP_ISPEED, 38400),
        (Pty::TTY_OP_OSPEED, 38400),
    ]
}

#[allow(clippy::too_many_arguments)]
async fn open_shell(
    handle: Handle<ClientHandler>,
    jump: Option<Arc<Handle<ClientHandler>>>,
    host_id: &str,
    title: &str,
    server_fp: Option<String>,
    cols: u32,
    rows: u32,
    events: mpsc::UnboundedSender<TermEvent>,
) -> Result<Arc<LiveSession>, String> {
    let channel = handle
        .channel_open_session()
        .await
        .map_err(|e| format!("无法打开会话通道：{e}"))?;
    channel
        .request_pty(
            false,
            "xterm-256color",
            cols.max(1),
            rows.max(1),
            0,
            0,
            &terminal_modes(),
        )
        .await
        .map_err(|e| format!("无法申请 PTY：{e}"))?;
    // UTF-8 by default; servers that do not allow LANG via AcceptEnv ignore it.
    let _ = channel.set_env(false, "LANG", "en_US.UTF-8").await;
    channel
        .request_shell(false)
        .await
        .map_err(|e| format!("无法启动 shell：{e}"))?;

    // Split so output can be pumped while the write half stays available for
    // keyboard input. Using `into_stream` would consume the channel and leave
    // no way to type.
    let (mut read_half, write_half) = channel.split();

    let session_id = uuid::Uuid::new_v4().to_string();
    let sid = session_id.clone();
    let output = Arc::new(Mutex::new(OutputBuffer::default()));
    let output_for_pump = output.clone();
    let recorder = Arc::new(Mutex::new(None::<crate::recorder::Recorder>));
    let recorder_for_pump = recorder.clone();
    let closed = Arc::new(AtomicBool::new(false));
    let closed_for_pump = closed.clone();
    tokio::spawn(async move {
        while let Some(msg) = read_half.wait().await {
            match msg {
                // stdout and stderr are shown inline in the same terminal.
                ChannelMsg::Data { data } | ChannelMsg::ExtendedData { data, .. } => {
                    if let Ok(mut buf) = output_for_pump.lock() {
                        buf.push(&data);
                    }
                    // Record from the pump, not the UI: long output outlives
                    // the scrollback and the pane.
                    if let Ok(mut slot) = recorder_for_pump.lock() {
                        if let Some(rec) = slot.as_mut() {
                            rec.write(&data);
                        }
                    }
                    let payload = base64::engine::general_purpose::STANDARD.encode(&data);
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
                ChannelMsg::ExitStatus { exit_status } => {
                    let _ = events.send(TermEvent::Exit {
                        session_id: sid.clone(),
                        code: exit_status,
                    });
                }
                ChannelMsg::Eof | ChannelMsg::Close => break,
                _ => {}
            }
        }
        // The stream is over: close any recording so the file gets its footer
        // even when the server dropped us rather than the user closing the tab.
        if let Ok(mut slot) = recorder_for_pump.lock() {
            if let Some(mut rec) = slot.take() {
                rec.finish(&format!("# 结束: {}\n", crate::recorder::local_stamp()));
            }
        }
        // Exactly one terminal event, whether the stream ended cleanly or not.
        closed_for_pump.store(true, Ordering::SeqCst);
        let _ = events.send(TermEvent::Closed {
            session_id: sid.clone(),
        });
    });

    Ok(Arc::new(LiveSession {
        info: Session {
            id: session_id,
            title: title.to_string(),
            status: "connected".into(),
            host_id: host_id.to_string(),
            fingerprint: server_fp,
        },
        handle: Arc::new(handle),
        _jump: jump,
        writer: tokio::sync::Mutex::new(write_half),
        output,
        closed,
        recorder,
    }))
}

/// Connects with a single credential, trusting only `pin`.
///
/// The convenience form the integration tests use; the app goes through
/// [`connect_chain`] with its known-hosts list.
pub async fn connect(
    host: &HostView,
    credential: &Credential,
    pin: Option<String>,
    cols: u32,
    rows: u32,
    events: mpsc::UnboundedSender<TermEvent>,
) -> Result<ConnectOutcome, String> {
    let hop = Hop {
        address: host.address.clone(),
        port: host.port,
        username: host.username.clone(),
        credentials: vec![credential.clone()],
        known: Vec::new(),
    };
    let title = format!("{}@{}", host.username, host.address);
    let res = connect_chain(
        None,
        &hop,
        &host.id,
        &title,
        pin.as_deref(),
        cols,
        rows,
        events,
    )
    .await?;
    Ok(res.outcome)
}

/// Runs one command on a fresh exec channel and returns its combined output.
///
/// A separate channel keeps the user's interactive shell (and its scrollback)
/// untouched, which matters when the assistant runs something mid-session.
pub async fn exec_command(session: &LiveSession, command: &str) -> Result<String, String> {
    exec_with_timeout(session, command, std::time::Duration::from_secs(60), 30_000).await
}

/// [`exec_command`] with an explicit deadline and output cap.
pub async fn exec_with_timeout(
    session: &LiveSession,
    command: &str,
    timeout: std::time::Duration,
    max_output: usize,
) -> Result<String, String> {
    use tokio::io::AsyncReadExt;

    let channel = session
        .handle()
        .channel_open_session()
        .await
        .map_err(|e| format!("无法打开执行通道：{e}"))?;
    channel
        .exec(true, command)
        .await
        .map_err(|e| format!("无法执行命令：{e}"))?;

    let mut stream = channel.into_stream();
    let mut out = Vec::new();
    let mut buf = [0u8; 8192];

    // A command that never exits would block the caller forever.
    let read = async {
        loop {
            match stream.read(&mut buf).await {
                Ok(0) => break,
                Ok(n) => out.extend_from_slice(&buf[..n]),
                Err(_) => break,
            }
        }
    };
    let _ = tokio::time::timeout(timeout, read).await;

    Ok(cap_output(
        String::from_utf8_lossy(&out).to_string(),
        max_output,
    ))
}

/// Caps what goes back to the model: `cat` on a huge file must not eat the
/// whole context window.
fn cap_output(text: String, max: usize) -> String {
    if text.len() <= max {
        return text;
    }
    let total = text.len();
    // `String::truncate` panics off a char boundary; walk back to one.
    let mut cut = max;
    while !text.is_char_boundary(cut) {
        cut -= 1;
    }
    format!("{}\n…[输出被截断，共 {total} 字节]", &text[..cut])
}

/// Repairs the ways a pasted private key gets mangled on its way into a text
/// box: macOS "smart dashes" turn the `-----` armour into em or en dashes,
/// Windows editors add `\r`, and copies from web pages pick up stray spaces.
/// Only the armour lines are rebuilt; the base64 body is left as typed.
pub fn normalize_private_key(text: &str) -> String {
    let is_dash = |c: char| matches!(c, '-' | '\u{2010}'..='\u{2015}' | '\u{2212}');
    let mut out = String::with_capacity(text.len());
    for line in text.trim().lines() {
        let line = line.trim();
        let core = line.trim_matches(is_dash).trim();
        if line.starts_with(is_dash) && (core.starts_with("BEGIN ") || core.starts_with("END ")) {
            out.push_str("-----");
            out.push_str(core);
            out.push_str("-----");
        } else {
            out.push_str(line);
        }
        out.push('\n');
    }
    out
}

/// Closes the connection and releases the session.
pub async fn close(session: Arc<LiveSession>) {
    let _ = session
        .handle
        .disconnect(russh::Disconnect::ByApplication, "", "en")
        .await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn term_events_use_the_field_names_the_webview_reads() {
        let ev = TermEvent::Data {
            session_id: "s".into(),
            data: "d".into(),
        };
        let json = serde_json::to_value(&ev).unwrap();
        assert_eq!(json["type"], "data");
        assert_eq!(json["sessionId"], "s");
        let closed = serde_json::to_value(TermEvent::Closed {
            session_id: "s".into(),
        })
        .unwrap();
        assert_eq!(closed["sessionId"], "s");
    }

    #[test]
    fn smart_dashes_in_pasted_keys_are_repaired() {
        let mangled = "\u{2014}\u{2014}-BEGIN OPENSSH PRIVATE KEY\u{2014}\u{2014}-\r\nabc+/=\r\n \u{2013}\u{2013}\u{2013}END OPENSSH PRIVATE KEY\u{2014}- \n";
        assert_eq!(
            normalize_private_key(mangled),
            "-----BEGIN OPENSSH PRIVATE KEY-----\nabc+/=\n-----END OPENSSH PRIVATE KEY-----\n"
        );
        // A well-formed key passes through unchanged apart from the final newline.
        let good = "-----BEGIN X-----\nAAAA\n-----END X-----";
        assert_eq!(normalize_private_key(good), format!("{good}\n"));
    }

    #[test]
    fn cap_output_never_splits_a_character() {
        // Each "日" is three bytes; a cap of 4 lands inside the second one.
        let out = cap_output("日日日".to_string(), 4);
        assert!(out.starts_with("日\n"), "{out}");
    }

    #[test]
    fn pending_is_handed_over_once_and_tail_keeps_rolling() {
        let mut buf = OutputBuffer::default();
        buf.push(b"banner");
        assert_eq!(buf.take_pending(), b"banner");
        buf.push(b" prompt");
        // After attaching, the pane receives events live; nothing is queued.
        assert!(buf.take_pending().is_empty());
        assert_eq!(buf.tail(), b"banner prompt");
    }

    #[test]
    fn tail_is_bounded() {
        let mut buf = OutputBuffer::default();
        buf.push(&vec![b'x'; TAIL_LIMIT + 10]);
        buf.push(b"end");
        let tail = buf.tail();
        assert_eq!(tail.len(), TAIL_LIMIT);
        assert!(tail.ends_with(b"end"));
    }
}
