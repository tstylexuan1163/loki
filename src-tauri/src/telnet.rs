//! Telnet: a raw TCP connection with just enough protocol to keep a terminal usable.
//!
//! Telnet is not a shell: the server may negotiate options, and the client has to
//! answer or the session hangs. This module handles option negotiation and passes
//! everything else through untouched, so it behaves like the SSH terminal from the
//! UI's point of view.

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

/// Telnet command bytes (RFC 854).
const IAC: u8 = 255;
const DONT: u8 = 254;
const DO: u8 = 253;
const WONT: u8 = 252;
const WILL: u8 = 251;
const SB: u8 = 250;
const SE: u8 = 240;

/// Options we are willing to turn on when asked.
///
/// Deliberately narrow: ECHO and NAWS are what a login screen needs. Anything
/// else is refused, which is always a legal answer in telnet.
const ACCEPT_WILL: &[u8] = &[
    1,  /* ECHO */
    3,  /* SUPPRESS GO AHEAD */
    31, /* NAWS */
];
const ACCEPT_DO: &[u8] = &[1 /* ECHO */, 3 /* SUPPRESS GO AHEAD */];

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
/// sent back. `IAC IAC` means a literal 0xFF, which is data, not a command.
pub fn strip_commands(input: &[u8], neg: &mut Negotiation) -> (Vec<u8>, Vec<u8>) {
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
            break;
        }

        let cmd = input[i + 1];
        if cmd == IAC {
            // Escaped 0xFF: ordinary data.
            out.push(IAC);
            i += 2;
            continue;
        }

        // Sub-negotiation carries an option byte then values until IAC SE.
        if cmd == SB {
            let started = i;
            i += 2;
            while i + 1 < input.len() && !(input[i] == IAC && input[i + 1] == SE) {
                i += 1;
            }
            i = (i + 2).min(input.len());
            // We do not act on sub-negotiations, but do not leak them either.
            let _ = started;
            continue;
        }

        if i + 2 >= input.len() {
            break;
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
            WONT | DONT => {}
            _ => {}
        }
        i += 3;
    }

    (out, replies)
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

/// A live telnet connection.
pub struct TelnetConnection {
    stream: TcpStream,
}

impl TelnetConnection {
    pub async fn connect(host: &str, port: u16) -> Result<Self, String> {
        let stream = TcpStream::connect((host, port))
            .await
            .map_err(|e| format!("无法连接 {host}:{port}：{e}"))?;
        // Login screens can be idle for a while before the first byte.
        let _ = stream.set_nodelay(true);
        Ok(Self { stream })
    }

    /// Sends user input. Telnet wants a bare CR for Enter, but modern servers and
    /// the PTY path both accept CRLF, so the bytes are passed through unchanged.
    pub async fn write(&mut self, data: &[u8]) -> Result<(), String> {
        self.stream
            .write_all(data)
            .await
            .map_err(|e| format!("发送失败：{e}"))
    }

    /// Sends the window size.
    pub async fn resize(&mut self, cols: u16, rows: u16) -> Result<(), String> {
        let frame = naws_frame(cols, rows);
        self.stream
            .write_all(&frame)
            .await
            .map_err(|e| format!("发送窗口大小失败：{e}"))
    }

    /// Reads one chunk, stripping telnet commands and answering them.
    ///
    /// Returns `None` at end of stream.
    pub async fn read(&mut self, neg: &mut Negotiation) -> Result<Option<Vec<u8>>, String> {
        let mut buf = [0u8; 8192];
        let n = self
            .stream
            .read(&mut buf)
            .await
            .map_err(|e| format!("读取失败：{e}"))?;
        if n == 0 {
            return Ok(None);
        }

        let (data, replies) = strip_commands(&buf[..n], neg);
        if !replies.is_empty() {
            let _ = self.stream.write_all(&replies).await;
        }
        Ok(Some(data))
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
