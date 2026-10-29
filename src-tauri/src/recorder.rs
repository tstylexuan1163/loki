//! Session recording and terminal export.
//!
//! A recorder writes what the remote side sent, straight from the output pump —
//! not from the WebView, so scrolling back or closing a pane cannot lose data
//! and long output (a switch configuration) survives the scrollback limit.
//!
//! "Plain" mode strips escape sequences so the file is readable in any editor;
//! the raw mode keeps them for replay with colour.

use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};

// ---- Escape-sequence stripping -------------------------------------------

#[derive(Default, Clone, Copy, PartialEq, Eq)]
enum State {
    #[default]
    Ground,
    /// Saw ESC; the next byte decides which sequence this is.
    Esc,
    /// Inside `ESC [ …` — ends on a byte in 0x40..=0x7e.
    Csi,
    /// Inside an OSC string — ends on BEL or `ESC \`.
    Osc,
    /// Saw ESC inside an OSC string.
    OscEsc,
    /// `ESC ( B` / `ESC ) 0` — a charset designation carries one more byte.
    EscCharset,
}

/// Turns terminal output into readable text.
///
/// Stateful on purpose: escape sequences and a trailing `\r` may be split across
/// reads, and a stateless filter would leak a fragment into the file.
#[derive(Default)]
pub struct PlainText {
    state: State,
    /// A CR is held until we learn whether an LF follows: CRLF becomes LF, a
    /// lone CR (progress redraw) is dropped.
    pending_cr: bool,
    /// Last byte written, so a backspace can erase it.
    last: Option<u8>,
}

impl PlainText {
    pub fn push(&mut self, input: &[u8], out: &mut Vec<u8>) {
        for &b in input {
            match self.state {
                State::Ground => match b {
                    0x1b => {
                        self.flush_cr(out);
                        self.state = State::Esc;
                    }
                    b'\r' => {
                        // Wait for a possible LF.
                        self.pending_cr = true;
                    }
                    b'\n' => {
                        self.pending_cr = false;
                        self.emit(b'\n', out);
                    }
                    0x08 => {
                        self.pending_cr = false;
                        self.backspace(out);
                    }
                    // Other C0 controls (BEL, NUL, …) carry nothing readable.
                    0x00..=0x1f | 0x7f => {
                        self.pending_cr = false;
                    }
                    _ => {
                        self.flush_cr(out);
                        self.emit(b, out);
                    }
                },
                State::Esc => {
                    self.state = match b {
                        b'[' => State::Csi,
                        b']' => State::Osc,
                        // Charset designation: `ESC ( B` has one payload byte.
                        b'(' | b')' | b'*' | b'+' => State::EscCharset,
                        // `ESC =`, `ESC 7`, `ESC M` … nothing follows.
                        _ => State::Ground,
                    };
                }
                State::Csi => {
                    if (0x40..=0x7e).contains(&b) {
                        self.state = State::Ground;
                    }
                }
                State::Osc => match b {
                    0x07 => self.state = State::Ground,
                    0x1b => self.state = State::OscEsc,
                    _ => {}
                },
                State::EscCharset => self.state = State::Ground,
                State::OscEsc => {
                    self.state = match b {
                        b'\\' => State::Ground,
                        0x1b => State::OscEsc,
                        _ => State::Osc,
                    };
                }
            }
        }
    }

    /// Ends the stream: any held CR is discarded, which is what a lone CR means.
    pub fn finish(&mut self, _out: &mut Vec<u8>) {
        self.pending_cr = false;
        self.state = State::Ground;
    }

    fn emit(&mut self, b: u8, out: &mut Vec<u8>) {
        out.push(b);
        self.last = Some(b);
    }

    /// A pending CR is a carriage return with no newline: dropped.
    fn flush_cr(&mut self, _out: &mut Vec<u8>) {
        self.pending_cr = false;
    }

    /// Erases the previous character, never crossing a line break.
    ///
    /// Only the byte this filter just produced can be erased; a backspace that
    /// arrives after a flush cannot reach back into the file (documented).
    fn backspace(&mut self, out: &mut Vec<u8>) {
        if let Some(b) = self.last {
            if b != b'\n' {
                out.pop();
                self.last = out.last().copied();
            }
        }
    }
}

/// Convenience for tests and one-shot conversions.
pub fn strip_ansi(input: &[u8]) -> Vec<u8> {
    let mut p = PlainText::default();
    let mut out = Vec::with_capacity(input.len());
    p.push(input, &mut out);
    p.finish(&mut out);
    out
}

// ---- Recorder ------------------------------------------------------------

/// An open recording file.
pub struct Recorder {
    path: PathBuf,
    writer: BufWriter<File>,
    /// `None` in raw mode.
    plain: Option<PlainText>,
    /// Bytes written after the header, i.e. what the user thinks of as the file.
    payload: u64,
    /// When this recording began, for the status line.
    started_at: String,
    finished: bool,
}

impl Recorder {
    /// Creates the file and writes its header.
    pub fn start(path: &Path, header: &str, plain: bool) -> std::io::Result<Self> {
        let file = File::create(path)?;
        let mut writer = BufWriter::new(file);
        writer.write_all(header.as_bytes())?;
        writer.flush()?;
        Ok(Self {
            path: path.to_path_buf(),
            writer,
            plain: if plain {
                Some(PlainText::default())
            } else {
                None
            },
            payload: 0,
            started_at: local_stamp(),
            finished: false,
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn bytes(&self) -> u64 {
        self.payload
    }

    pub fn started_at(&self) -> &str {
        &self.started_at
    }

    /// Appends one chunk of terminal output and makes it visible immediately.
    ///
    /// Flushing per chunk is deliberate. The pump only calls in when bytes
    /// arrive, so a "flush if a second has passed" rule leaves the tail in the
    /// buffer while the session is quiet — the file then looks truncated until
    /// more output happens to come. Chunks are already 4–16 KB and the recorder
    /// keeps up easily (a 289 KB burst costs ~13 ms), so the simplicity wins.
    pub fn write(&mut self, data: &[u8]) {
        if self.finished {
            return;
        }
        let mut buf = Vec::with_capacity(data.len());
        match self.plain.as_mut() {
            Some(p) => p.push(data, &mut buf),
            None => buf.extend_from_slice(data),
        }
        if buf.is_empty() {
            return;
        }
        if self.writer.write_all(&buf).is_ok() {
            self.payload += buf.len() as u64;
        }
        let _ = self.writer.flush();
    }

    /// Writes the footer, flushes, and reports the payload size.
    pub fn finish(&mut self, footer: &str) -> u64 {
        if self.finished {
            return self.payload;
        }
        self.finished = true;
        if let Some(p) = self.plain.as_mut() {
            let mut tail = Vec::new();
            p.finish(&mut tail);
            if !tail.is_empty() {
                let _ = self.writer.write_all(&tail);
                self.payload += tail.len() as u64;
            }
        }
        // Make sure the last line is terminated before the footer.
        if self.payload > 0 {
            let _ = self.writer.write_all(b"\n");
        }
        let _ = self.writer.write_all(footer.as_bytes());
        let _ = self.writer.flush();
        self.payload
    }
}

/// `2026-10-29 22:50:11` in local time, for headers and file names.
pub fn local_stamp() -> String {
    chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string()
}

/// `YYYYMMDD-HHMM` in local time, for suggested file names.
pub fn file_stamp() -> String {
    chrono::Local::now().format("%Y%m%d-%H%M").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colours_are_removed() {
        assert_eq!(strip_ansi(b"\x1b[31mred\x1b[0m plain"), b"red plain");
    }

    #[test]
    fn cursor_movement_is_removed() {
        assert_eq!(strip_ansi(b"a\x1b[2Kb\x1b[10;20Hc"), b"abc");
    }

    #[test]
    fn osc_title_is_removed() {
        // BEL-terminated and ESC-\ terminated forms.
        assert_eq!(strip_ansi(b"\x1b]0;title\x07after"), b"after");
        assert_eq!(strip_ansi(b"\x1b]2;title\x1b\\after"), b"after");
    }

    #[test]
    fn crlf_becomes_lf_and_lone_cr_disappears() {
        assert_eq!(strip_ansi(b"one\r\ntwo"), b"one\ntwo");
        // A progress-bar style CR with no LF is dropped, not turned into a break.
        assert_eq!(strip_ansi(b"10%\r20%"), b"10%20%");
    }

    #[test]
    fn charset_designation_is_removed() {
        assert_eq!(strip_ansi(b"\x1b(Bhello"), b"hello");
    }

    #[test]
    fn charset_switch_keeps_the_text() {
        assert_eq!(strip_ansi(b"\x1b)0utf8 \x1b(Bok"), b"utf8 ok");
    }

    #[test]
    fn sequence_split_across_chunks_still_works() {
        let mut p = PlainText::default();
        let mut out = Vec::new();
        p.push(b"a\x1b[3", &mut out);
        p.push(b"1mb", &mut out);
        p.finish(&mut out);
        assert_eq!(out, b"ab");
    }

    #[test]
    fn crlf_split_across_chunks_does_not_produce_two_breaks() {
        let mut p = PlainText::default();
        let mut out = Vec::new();
        p.push(b"line\r", &mut out);
        p.push(b"\nnext", &mut out);
        p.finish(&mut out);
        assert_eq!(out, b"line\nnext");
    }

    #[test]
    fn backspace_erases_within_the_chunk() {
        assert_eq!(strip_ansi(b"abc\x08d"), b"abd");
        // Never eats a newline.
        assert_eq!(strip_ansi(b"a\n\x08b"), b"a\nb");
    }

    #[test]
    fn recorder_writes_header_payload_and_footer() {
        let dir = std::env::temp_dir().join(format!("loki-rec-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("session.txt");

        let mut rec = Recorder::start(&path, "# 头\n", true).unwrap();
        rec.write(b"\x1b[32mgreen\x1b[0m\r\n");
        rec.write(b"second\r\n");
        let bytes = rec.finish("# 尾\n");
        assert!(bytes > 0);

        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.starts_with("# 头\n"), "{text:?}");
        assert!(text.contains("green\nsecond"), "{text:?}");
        assert!(text.ends_with("# 尾\n"), "{text:?}");
        // No escape byte survived.
        assert!(!text.as_bytes().contains(&0x1b), "{text:?}");

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn raw_recorder_keeps_control_bytes() {
        let dir = std::env::temp_dir().join(format!("loki-raw-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("raw.txt");

        let mut rec = Recorder::start(&path, "", false).unwrap();
        rec.write(b"\x1b[31mred\x1b[0m");
        rec.finish("");
        let bytes = std::fs::read(&path).unwrap();
        assert!(bytes.contains(&0x1b), "raw mode must keep escapes");

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn stamps_have_the_expected_shape() {
        let s = local_stamp();
        assert_eq!(s.len(), 19, "{s}");
        let f = file_stamp();
        assert_eq!(f.len(), 13, "{f}");
    }
}
