//! Telnet: a raw TCP connection with just enough protocol to keep a terminal usable.
//!
//! Telnet is not a shell: the server may negotiate options, and the client has to
//! answer or the session hangs. This module handles option negotiation and passes
//! everything else through untouched, so it behaves like the SSH terminal from the
//! UI's point of view.

use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};
use tokio::net::TcpStream;

/// Telnet command bytes (RFC 854).
const IAC: u8 = 255;
const DONT: u8 = 254;
const DO: u8 = 253;
const WONT: u8 = 252;
const WILL: u8 = 251;
const SB: u8 = 250;
const SE: u8 = 240;

const NAWS: u8 = 31;

/// Options we let the server turn on for its side (`WILL`): it echoes, and
/// it skips go-ahead.
///
/// Deliberately narrow; anything else is refused, which is always a legal
/// answer in telnet.
const ACCEPT_WILL: &[u8] = &[1 /* ECHO */, 3 /* SUPPRESS GO AHEAD */];
/// Options we agree to do ourselves (`DO`). Not ECHO: the terminal never
/// echoes locally, and claiming to would leave typed text invisible. NAWS is
/// how a switch learns the window width instead of wrapping at 80 columns.
const ACCEPT_DO: &[u8] = &[3 /* SUPPRESS GO AHEAD */, NAWS];

/// A sub-negotiation longer than this without its `IAC SE` is junk; drop it
/// rather than hold back output forever.
const MAX_PENDING: usize = 4096;

/// What the negotiation produced, so the caller can log or display it.
#[derive(Debug, Default, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Negotiation {
    /// Options the server asked us to enable.
    pub server_will: Vec<u8>,
    /// Options the server asked us to enable on its behalf.
    pub server_do: Vec<u8>,
    /// Options we refused.
    pub refused: Vec<u8>,
}

/// Removes IAC escapes from a data chunk and records any commands it carried.
///
/// Returns the bytes that should reach the terminal, plus replies that must be
/// sent back. `IAC IAC` means a literal 0xFF, which is data, not a command. A
/// command cut off at the end of the chunk is dropped; [`Decoder`] keeps it
/// for the next read instead.
pub fn strip_commands(input: &[u8], neg: &mut Negotiation) -> (Vec<u8>, Vec<u8>) {
    let (out, replies, _) = decode(input, neg);
    (out, replies)
}

/// Strips telnet commands across reads: TCP may split `IAC WILL ECHO` between
/// two segments, and the tail must not reach the screen as stray bytes.
#[derive(Default)]
pub struct Decoder {
    pending: Vec<u8>,
}

impl Decoder {
    pub fn feed(&mut self, input: &[u8], neg: &mut Negotiation) -> (Vec<u8>, Vec<u8>) {
        let mut buf = std::mem::take(&mut self.pending);
        buf.extend_from_slice(input);
        let (out, replies, used) = decode(&buf, neg);
        if buf.len() - used <= MAX_PENDING {
            self.pending = buf[used..].to_vec();
        }
        (out, replies)
    }
}

/// The decoder proper. The third value is where an incomplete command starts
/// (the input length when there is none).
fn decode(input: &[u8], neg: &mut Negotiation) -> (Vec<u8>, Vec<u8>, usize) {
    let mut out = Vec::with_capacity(input.len());
    let mut replies = Vec::new();
    let mut i = 0;

    while i < input.len() {
        if input[i] != IAC {
            out.push(input[i]);
            i += 1;
            continue;
        }

        if i + 1 >= input.len() {
            return (out, replies, i);
        }

        let cmd = input[i + 1];
        if cmd == IAC {
            // Escaped 0xFF: ordinary data.
            out.push(IAC);
            i += 2;
            continue;
        }

        // Sub-negotiation carries an option byte then values until IAC SE.
        // We do not act on them, but do not leak them either.
        if cmd == SB {
            let mut j = i + 2;
            while j + 1 < input.len() && !(input[j] == IAC && input[j + 1] == SE) {
                j += 1;
            }
            if j + 1 >= input.len() {
                return (out, replies, i);
            }
            i = j + 2;
            continue;
        }

        // Only the four option verbs carry an option byte; NOP, GA, AYT and
        // the rest are two bytes, and reading a third would eat real text.
        if !matches!(cmd, WILL | WONT | DO | DONT) {
            i += 2;
            continue;
        }
        if i + 2 >= input.len() {
            return (out, replies, i);
        }
        let option = input[i + 2];
        match cmd {
            WILL => {
                if ACCEPT_WILL.contains(&option) {
                    if !neg.server_will.contains(&option) {
                        neg.server_will.push(option);
                    }
                    replies.extend_from_slice(&[IAC, DO, option]);
                } else {
                    if !neg.refused.contains(&option) {
                        neg.refused.push(option);
                    }
                    replies.extend_from_slice(&[IAC, DONT, option]);
                }
            }
            DO => {
                if ACCEPT_DO.contains(&option) {
                    if !neg.server_do.contains(&option) {
                        neg.server_do.push(option);
                    }
                    replies.extend_from_slice(&[IAC, WILL, option]);
                } else {
                    if !neg.refused.contains(&option) {
                        neg.refused.push(option);
                    }
                    replies.extend_from_slice(&[IAC, WONT, option]);
                }
            }
            // An answer to something we never asked: ignore it.
            _ => {}
        }
        i += 3;
    }

    (out, replies, input.len())
}

/// Doubles 0xFF in outgoing data so the server reads it as a byte, not IAC.
fn escape_iac(data: &[u8]) -> std::borrow::Cow<'_, [u8]> {
    if !data.contains(&IAC) {
        return std::borrow::Cow::Borrowed(data);
    }
    let mut out = Vec::with_capacity(data.len() + 4);
    for &b in data {
        if b == IAC {
            out.push(IAC);
        }
        out.push(b);
    }
    std::borrow::Cow::Owned(out)
}

/// Tells the server our window size (NAWS). Servers that ignore it are unaffected.
pub fn naws_frame(cols: u16, rows: u16) -> Vec<u8> {
    let mut frame = vec![IAC, SB, 31];
    // Telnet encodes 0xFF in the payload as IAC IAC.
    for byte in [
        (cols >> 8) as u8,
        (cols & 0xff) as u8,
        (rows >> 8) as u8,
        (rows & 0xff) as u8,
    ] {
        if byte == IAC {
            frame.push(IAC);
        }
        frame.push(byte);
    }
    frame.push(IAC);
    frame.push(SE);
    frame
}

/// A live telnet connection, before it is split for the terminal.
pub struct TelnetConnection {
    reader: OwnedReadHalf,
    writer: TelnetWriter,
}

impl TelnetConnection {
    pub async fn connect(host: &str, port: u16) -> Result<Self, String> {
        let stream = TcpStream::connect((host, port))
            .await
            .map_err(|e| format!("无法连接 {host}:{port}：{e}"))?;
        // Login screens can be idle for a while before the first byte.
        let _ = stream.set_nodelay(true);
        let (reader, writer) = stream.into_split();
        Ok(Self {
            reader,
            writer: TelnetWriter {
                half: Arc::new(tokio::sync::Mutex::new(writer)),
                size: Arc::new(std::sync::Mutex::new((80, 24))),
            },
        })
    }

    /// Sends the window size.
    pub async fn resize(&mut self, cols: u16, rows: u16) -> Result<(), String> {
        self.writer.resize(cols, rows).await
    }

    /// Splits into a reader for the output task and a writer for keystrokes.
    ///
    /// They must not share a lock: a reader waiting for the server would make
    /// every keystroke wait with it.
    pub fn split(self) -> (TelnetReader, TelnetWriter) {
        (
            TelnetReader {
                half: self.reader,
                decoder: Decoder::default(),
                writer: self.writer.clone(),
            },
            self.writer,
        )
    }
}

/// The sending half: user input, window size, and negotiation replies.
#[derive(Clone)]
pub struct TelnetWriter {
    half: Arc<tokio::sync::Mutex<OwnedWriteHalf>>,
    /// The last size the terminal reported, for when the server asks for it.
    size: Arc<std::sync::Mutex<(u16, u16)>>,
}

impl TelnetWriter {
    /// Sends user input. Telnet wants a bare CR for Enter, but modern servers and
    /// the PTY path both accept CRLF, so the bytes are passed through unchanged.
    pub async fn write(&self, data: &[u8]) -> Result<(), String> {
        self.raw(&escape_iac(data))
            .await
            .map_err(|e| format!("发送失败：{e}"))
    }

    /// Sends the window size.
    pub async fn resize(&self, cols: u16, rows: u16) -> Result<(), String> {
        if let Ok(mut s) = self.size.lock() {
            *s = (cols, rows);
        }
        self.raw(&naws_frame(cols, rows))
            .await
            .map_err(|e| format!("发送窗口大小失败：{e}"))
    }

    fn size(&self) -> (u16, u16) {
        self.size.lock().map(|s| *s).unwrap_or((80, 24))
    }

    async fn raw(&self, bytes: &[u8]) -> std::io::Result<()> {
        self.half.lock().await.write_all(bytes).await
    }
}

/// The receiving half: strips telnet commands and answers them.
pub struct TelnetReader {
    half: OwnedReadHalf,
    decoder: Decoder,
    writer: TelnetWriter,
}

impl TelnetReader {
    /// Waits for the next raw chunk; `None` at end of stream.
    ///
    /// Only this step may sit under a timeout: it is cancel-safe, whereas
    /// cancelling [`Self::process`] halfway would lose a decoded chunk.
    pub async fn recv(&mut self) -> Result<Option<Vec<u8>>, String> {
        let mut buf = vec![0u8; 8192];
        let n = self
            .half
            .read(&mut buf)
            .await
            .map_err(|e| format!("读取失败：{e}"))?;
        if n == 0 {
            return Ok(None);
        }
        buf.truncate(n);
        Ok(Some(buf))
    }

    /// Strips telnet commands from a raw chunk, answers them, and returns the
    /// bytes meant for the screen.
    pub async fn process(&mut self, raw: &[u8], neg: &mut Negotiation) -> Vec<u8> {
        let had_naws = neg.server_do.contains(&NAWS);
        let (data, mut replies) = self.decoder.feed(raw, neg);
        // Agreeing to NAWS means sending the size right after the WILL.
        if !had_naws && neg.server_do.contains(&NAWS) {
            let (cols, rows) = self.writer.size();
            replies.extend_from_slice(&naws_frame(cols, rows));
        }
        if !replies.is_empty() {
            let _ = self.writer.raw(&replies).await;
        }
        data
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_data_passes_through() {
        let mut neg = Negotiation::default();
        let (data, replies) = strip_commands(b"hello\r\n", &mut neg);
        assert_eq!(data, b"hello\r\n");
        assert!(replies.is_empty());
    }

    #[test]
    fn doubled_iac_becomes_one_data_byte() {
        let mut neg = Negotiation::default();
        let (data, _) = strip_commands(&[b'a', IAC, IAC, b'b'], &mut neg);
        assert_eq!(data, vec![b'a', IAC, b'b']);
    }

    #[test]
    fn a_command_split_across_reads_does_not_leak() {
        let mut neg = Negotiation::default();
        let mut dec = Decoder::default();
        let (data, replies) = dec.feed(&[b'a', IAC, WILL], &mut neg);
        assert_eq!(data, b"a");
        assert!(replies.is_empty());
        let (data, replies) = dec.feed(&[1, b'b'], &mut neg);
        assert_eq!(data, b"b", "the option byte is not screen output");
        assert_eq!(replies, vec![IAC, DO, 1]);

        // Same for a sub-negotiation cut before its IAC SE.
        let (data, _) = dec.feed(&[IAC, SB, 24, 1, IAC], &mut neg);
        assert!(data.is_empty());
        let (data, _) = dec.feed(&[SE, b'o', b'k'], &mut neg);
        assert_eq!(data, b"ok");
    }

    #[test]
    fn two_byte_commands_keep_the_next_character() {
        let mut neg = Negotiation::default();
        // IAC NOP and IAC GA, as keepalives and prompts send them.
        let (data, replies) = strip_commands(&[IAC, 241, b'x', IAC, 249, b'y'], &mut neg);
        assert_eq!(data, b"xy");
        assert!(replies.is_empty());
    }

    #[test]
    fn naws_is_agreed_and_local_echo_refused() {
        let mut neg = Negotiation::default();
        let (_, replies) = strip_commands(&[IAC, DO, NAWS, IAC, DO, 1], &mut neg);
        assert_eq!(replies, vec![IAC, WILL, NAWS, IAC, WONT, 1]);
        assert_eq!(neg.server_do, vec![NAWS]);
    }

    #[test]
    fn outgoing_ff_is_doubled() {
        assert_eq!(&*escape_iac(b"ab"), b"ab");
        assert_eq!(&*escape_iac(&[1, IAC, 2]), &[1, IAC, IAC, 2]);
    }

    #[test]
    fn will_echo_is_accepted_with_do() {
        let mut neg = Negotiation::default();
        let (data, replies) = strip_commands(&[IAC, WILL, 1], &mut neg);
        assert!(data.is_empty(), "negotiation bytes are not screen output");
        assert_eq!(replies, vec![IAC, DO, 1]);
        assert_eq!(neg.server_will, vec![1]);
    }

    #[test]
    fn unknown_options_are_refused() {
        let mut neg = Negotiation::default();
        // 24 is terminal type: we decline rather than pretend to implement it.
        let (_, replies) = strip_commands(&[IAC, WILL, 24], &mut neg);
        assert_eq!(replies, vec![IAC, DONT, 24]);
        assert_eq!(neg.refused, vec![24]);
    }

    #[test]
    fn sub_negotiation_is_consumed_not_displayed() {
        let mut neg = Negotiation::default();
        // IAC SB 24 0 'x' ... IAC SE followed by text.
        let input = [IAC, SB, 24, 0, b'x', b'y', IAC, SE, b'o', b'k'];
        let (data, _) = strip_commands(&input, &mut neg);
        assert_eq!(data, b"ok");
    }

    #[test]
    fn naws_encodes_dimensions() {
        let frame = naws_frame(120, 32);
        assert_eq!(&frame[..3], &[IAC, SB, 31]);
        assert_eq!(&frame[3..7], &[0, 120, 0, 32]);
        assert_eq!(&frame[7..], &[IAC, SE]);
    }

    #[test]
    fn naws_escapes_ff_bytes() {
        // A 0xFF anywhere in the payload must be doubled so it is not read as IAC.
        // cols = 0xFF00 encodes as [0xFF, 0x00]; the leading 0xFF becomes IAC IAC.
        let frame = naws_frame(0xFF00, 1);
        assert_eq!(&frame[3..6], &[IAC, IAC, 0x00]);
        // The trailing SE must still be intact, not swallowed by the escape.
        assert_eq!(&frame[frame.len() - 2..], &[IAC, SE]);
    }

    #[test]
    fn truncated_command_at_end_is_dropped() {
        let mut neg = Negotiation::default();
        let (data, _) = strip_commands(&[b'a', IAC], &mut neg);
        assert_eq!(data, b"a");
    }
}
