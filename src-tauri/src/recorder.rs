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

/// Longest line kept before it is written out anyway: output with no newline
/// at all (a binary dump) must not grow without bound.
const MAX_LINE: usize = 16 * 1024;

/// Turns terminal output into readable text.
///
/// The current line is kept as a row of cells with a cursor, the way a
/// terminal would draw it, and written out when a newline ends it. That is
/// what makes pagers come out clean: a switch prints ` --More-- `, then on the
/// next key sends `\r`, spaces and `\r` (Cisco) or `ESC [ 42 D` plus `ESC [ K`
/// (Huawei, H3C) to wipe it, and the configuration line that follows simply
/// overwrites those cells. Progress bars redrawn with `\r` keep only their last
/// state, and a shell's `\b \b` erases a character.
///
/// Limits (plain text, not an emulator): cursor moves to another row are
/// ignored, wide CJK characters count as one cell, and trailing spaces on a
/// line are trimmed. Sequences may be split across reads; the state carries
/// over.
#[derive(Default)]
pub struct PlainText {
    state: State,
    /// Parameter bytes of the CSI being read, e.g. `42` in `ESC [ 42 D`.
    params: Vec<u8>,
    /// The line being drawn, and where the cursor is in it.
    line: Vec<char>,
    col: usize,
    /// Bytes of a UTF-8 character that has not fully arrived yet.
    utf8: Vec<u8>,
}

impl PlainText {
    pub fn push(&mut self, input: &[u8], out: &mut Vec<u8>) {
        for &b in input {
            match self.state {
                State::Ground => self.ground(b, out),
                State::Esc => {
                    self.state = match b {
                        b'[' => {
                            self.params.clear();
                            State::Csi
                        }
                        b']' => State::Osc,
                        // Charset designation: `ESC ( B` has one payload byte.
                        b'(' | b')' | b'*' | b'+' => State::EscCharset,
                        // `ESC =`, `ESC 7`, `ESC M` … nothing follows.
                        _ => State::Ground,
                    };
                }
                State::Csi => {
                    if (0x40..=0x7e).contains(&b) {
                        self.csi(b);
                        self.state = State::Ground;
                    } else if self.params.len() < 32 {
                        self.params.push(b);
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

    /// Ends the stream: an unfinished last line (a prompt) is written as is.
    pub fn finish(&mut self, out: &mut Vec<u8>) {
        self.state = State::Ground;
        self.utf8.clear();
        if self.line.iter().any(|c| *c != ' ') {
            self.emit_line(out, false);
        }
        self.line.clear();
        self.col = 0;
    }

    fn ground(&mut self, b: u8, out: &mut Vec<u8>) {
        match b {
            0x1b => {
                self.utf8.clear();
                self.state = State::Esc;
            }
            b'\r' => self.col = 0,
            b'\n' => self.emit_line(out, true),
            0x08 => self.col = self.col.saturating_sub(1),
            b'\t' => self.put('\t', out),
            // Other C0 controls (BEL, NUL, …) carry nothing readable.
            0x00..=0x1f | 0x7f => {}
            0x20..=0x7e => {
                self.utf8.clear();
                self.put(b as char, out);
            }
            _ => {
                self.utf8.push(b);
                match std::str::from_utf8(&self.utf8) {
                    Ok(s) => {
                        let ch = s.chars().next().unwrap_or('\u{fffd}');
                        self.utf8.clear();
                        self.put(ch, out);
                    }
                    // Invalid, or longer than any character: give up on it.
                    Err(e) if e.error_len().is_some() || self.utf8.len() >= 4 => {
                        self.utf8.clear();
                        self.put('\u{fffd}', out);
                    }
                    // Incomplete: wait for the rest.
                    Err(_) => {}
                }
            }
        }
    }

    /// Writes one character at the cursor, overwriting what is there.
    fn put(&mut self, ch: char, out: &mut Vec<u8>) {
        if self.col >= MAX_LINE {
            self.emit_line(out, true);
        }
        while self.line.len() < self.col {
            self.line.push(' ');
        }
        if self.col < self.line.len() {
            self.line[self.col] = ch;
        } else {
            self.line.push(ch);
        }
        self.col += 1;
    }

    /// Acts on the CSI sequences that change the current line; the rest
    /// (colours, other rows, modes) are dropped.
    fn csi(&mut self, final_byte: u8) {
        // `ESC [ ? 25 l` and friends are private modes, never line edits.
        if self
            .params
            .first()
            .is_some_and(|b| matches!(b, b'?' | b'>' | b'='))
        {
            return;
        }
        let first: Option<usize> = std::str::from_utf8(&self.params)
            .ok()
            .and_then(|s| s.split(';').next())
            .and_then(|s| s.parse().ok());
        let n = first.unwrap_or(1).max(1);
        match final_byte {
            b'D' => self.col = self.col.saturating_sub(n),
            b'C' => self.col = (self.col + n).min(MAX_LINE - 1),
            b'G' => self.col = (n - 1).min(MAX_LINE - 1),
            b'K' => match first.unwrap_or(0) {
                0 => self.line.truncate(self.col),
                1 => {
                    for c in self.line.iter_mut().take(self.col + 1) {
                        *c = ' ';
                    }
                }
                _ => self.line.clear(),
            },
            // Delete characters at the cursor (readline editing).
            b'P' => {
                let end = (self.col + n).min(self.line.len());
                if self.col < end {
                    self.line.drain(self.col..end);
                }
            }
            // Erase characters in place.
            b'X' => {
                for c in self.line.iter_mut().skip(self.col).take(n) {
                    *c = ' ';
                }
            }
            // Insert blanks at the cursor.
            b'@' if self.col <= self.line.len() => {
                let at = self.col;
                self.line
                    .splice(at..at, std::iter::repeat_n(' ', n.min(MAX_LINE)));
                self.line.truncate(MAX_LINE);
            }
            _ => {}
        }
    }

    /// Writes the line without its trailing blanks, and starts a new one.
    fn emit_line(&mut self, out: &mut Vec<u8>, newline: bool) {
        let end = self
            .line
            .iter()
            .rposition(|c| *c != ' ')
            .map(|i| i + 1)
            .unwrap_or(0);
        let mut buf = [0u8; 4];
        for ch in &self.line[..end] {
            out.extend_from_slice(ch.encode_utf8(&mut buf).as_bytes());
        }
        if newline {
            out.push(b'\n');
        }
        self.line.clear();
        self.col = 0;
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
    fn moves_to_other_rows_are_dropped() {
        assert_eq!(strip_ansi(b"a\x1b[10;20Hb\x1b[?25lc"), b"abc");
    }

    #[test]
    fn osc_title_is_removed() {
        // BEL-terminated and ESC-\ terminated forms.
        assert_eq!(strip_ansi(b"\x1b]0;title\x07after"), b"after");
        assert_eq!(strip_ansi(b"\x1b]2;title\x1b\\after"), b"after");
    }

    #[test]
    fn crlf_becomes_lf_and_a_lone_cr_redraws_the_line() {
        assert_eq!(strip_ansi(b"one\r\ntwo"), b"one\ntwo");
        // A progress bar redrawn with CR keeps only its last state.
        assert_eq!(strip_ansi(b"10%\r20%\r100%\n"), b"100%\n");
    }

    #[test]
    fn cisco_more_prompt_leaves_no_trace() {
        // ` --More-- `, then CR + spaces + CR to wipe it, then the next line.
        let out = strip_ansi(b"line one\r\n --More-- \r          \rinterface Gi0/1\r\n");
        assert_eq!(out, b"line one\ninterface Gi0/1\n");
    }

    #[test]
    fn huawei_more_prompt_leaves_no_trace() {
        // `---- More ----` wiped with cursor-left and erase-to-end.
        let out = strip_ansi(b"  ---- More ----\x1b[42D\x1b[K interface GE0/0/1\r\n");
        assert_eq!(out, b" interface GE0/0/1\n");
        let out = strip_ansi(b"  ---- More ----\x1b[16D                \x1b[16Dsysname core\r\n");
        assert_eq!(out, b"sysname core\n");
    }

    #[test]
    fn line_editing_sequences_apply() {
        // Erase whole line, delete chars, cursor absolute.
        assert_eq!(strip_ansi(b"junk\x1b[2K\rok\n"), b"ok\n");
        assert_eq!(strip_ansi(b"abcdef\x1b[4D\x1b[2P\n"), b"abef\n");
        assert_eq!(strip_ansi(b"abc\x1b[1GX\n"), b"Xbc\n");
    }

    #[test]
    fn multibyte_text_survives_and_split_characters_join() {
        assert_eq!(strip_ansi("配置 OK\r\n".as_bytes()), "配置 OK\n".as_bytes());
        let bytes = "中".as_bytes();
        let mut p = PlainText::default();
        let mut out = Vec::new();
        p.push(&bytes[..1], &mut out);
        p.push(&bytes[1..], &mut out);
        p.finish(&mut out);
        assert_eq!(out, "中".as_bytes());
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
    fn backspace_moves_back_and_the_next_character_overwrites() {
        assert_eq!(strip_ansi(b"abc\x08d"), b"abd");
        // A shell erases with `\b \b`.
        assert_eq!(strip_ansi(b"abc\x08 \x08\n"), b"ab\n");
        // Never crosses a line break.
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
