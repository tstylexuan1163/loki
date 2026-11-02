//! Typing into the user's terminal, for devices that only have an
//! interactive shell.
//!
//! `run_command` opens a fresh exec channel per command: clean, but a switch
//! often refuses exec, a configuration view does not survive between
//! channels, and a `---- More ----` pager waits forever. Here the command is
//! typed into the session the user sees, its output is watched until the
//! device prints a prompt again, pages are turned with a space, and a question
//! only a person may answer (a password, `[Y/N]`) hands control back.

use std::future::Future;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::time::Instant;

/// Pages turned before giving up on a pager that never ends.
const MAX_PAGES: usize = 500;
/// Raw output collected before the command is interrupted.
const MAX_RAW: usize = 256 * 1024;
/// What goes back to the model.
const MAX_OUTPUT: usize = 30_000;

/// A session the assistant can type into.
#[derive(Clone)]
pub enum Terminal {
    Ssh(Arc<crate::ssh::LiveSession>),
    /// Telnet, serial or a local shell.
    Direct {
        session: Arc<crate::direct::DirectSession>,
        output: crate::direct::Output,
    },
}

impl Terminal {
    pub async fn send(&self, data: &[u8]) -> Result<(), String> {
        match self {
            Terminal::Ssh(s) => s.write(data).await,
            Terminal::Direct { session, .. } => session.write(data).await,
        }
    }

    pub fn open_tap(&self) -> Result<crate::ssh::Tap, String> {
        match self {
            Terminal::Ssh(s) => s.open_tap(),
            Terminal::Direct { output, .. } => crate::ssh::Tap::open(output),
        }
    }

    /// The most recent output, as the screen shows it now.
    pub fn screen(&self) -> Vec<u8> {
        match self {
            Terminal::Ssh(s) => s.peek_backlog(),
            Terminal::Direct { output, .. } => output.lock().map(|b| b.tail()).unwrap_or_default(),
        }
    }
}

/// The keys `terminal_key` may press. Nothing else can be typed that way, so
/// the model has no route to entering a password.
pub fn key_bytes(name: &str) -> Option<&'static [u8]> {
    Some(match name {
        "ctrl_c" => b"\x03",
        "enter" => b"\r",
        "space" => b" ",
        "q" => b"q",
        "y" => b"y",
        "n" => b"n",
        _ => return None,
    })
}

/// How a command's output ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ended {
    /// The device printed a prompt: the command finished.
    Prompt(String),
    /// The device is asking something only the user may answer.
    Input(String),
    /// Output stopped without a recognisable prompt.
    Idle,
    /// The time limit passed; the command may still be running.
    Timeout,
    /// The session closed.
    Closed,
}

#[derive(Debug)]
pub struct Outcome {
    pub output: String,
    pub ended: Ended,
    pub pages: usize,
    pub truncated: bool,
}

impl Outcome {
    /// The text the model reads: the output, then one line on how it ended.
    pub fn report(&self) -> String {
        let mut out = if self.output.trim().is_empty() {
            "（没有输出）".to_string()
        } else {
            self.output.clone()
        };
        out.push_str("\n---\n");
        out.push_str(&match &self.ended {
            Ended::Prompt(p) => format!("已回到提示符：{p}"),
            Ended::Input(line) => format!(
                "设备在等待输入：{line}\n这需要用户本人回答。请告诉用户需要做什么，不要替用户回答密码。\
确认类问题（y/n）在用户同意后可以用 terminal_key 回答。"
            ),
            Ended::Idle => "输出已停止，但没有识别到提示符。可以用 read_terminal 查看屏幕。".into(),
            Ended::Timeout => {
                "已超时，命令可能仍在运行。需要中断时用 terminal_key 发送 ctrl_c。".into()
            }
            Ended::Closed => "会话已关闭。".into(),
        });
        if self.pages > 0 {
            out.push_str(&format!("\n（自动翻页 {} 次）", self.pages));
        }
        if self.truncated {
            out.push_str("\n（输出过长，已中断并截断）");
        }
        out
    }
}

/// Waiting thresholds, shortened in tests.
#[derive(Clone, Copy)]
pub struct Timing {
    /// Silence after which the last line is judged (prompt, question, pager).
    pub quiet: Duration,
    /// Silence after which an unrecognised last line ends the wait.
    pub idle: Duration,
}

impl Default for Timing {
    fn default() -> Self {
        Self {
            quiet: Duration::from_millis(300),
            idle: Duration::from_secs(5),
        }
    }
}

/// Types one command line into `term` and waits for the device.
pub async fn type_line(
    term: &Terminal,
    command: &str,
    timeout: Duration,
) -> Result<Outcome, String> {
    let command = command.trim_end_matches(['\r', '\n']);
    if command.contains(['\r', '\n']) {
        return Err("一次只能输入一行命令".into());
    }
    if let Some(why) = refuse_reason(&term.screen()) {
        return Err(why);
    }
    let mut tap = term.open_tap()?;
    let input = format!("{command}\r");
    drive(
        |data| async move { term.send(&data).await },
        &mut tap.rx,
        input.as_bytes(),
        Some(command),
        timeout,
        Timing::default(),
    )
    .await
}

/// Presses one of the keys `terminal_key` allows and waits for the device.
pub async fn press(term: &Terminal, key: &str, timeout: Duration) -> Result<Outcome, String> {
    let bytes = key_bytes(key).ok_or_else(|| format!("不支持的按键：{key}"))?;
    let mut tap = term.open_tap()?;
    drive(
        |data| async move { term.send(&data).await },
        &mut tap.rx,
        bytes,
        None,
        timeout,
        Timing::default(),
    )
    .await
}

/// Why typing must not start: the screen is waiting for a person.
pub fn refuse_reason(screen: &[u8]) -> Option<String> {
    let lines = render(screen);
    let last = lines.last().map(String::as_str).unwrap_or("");
    match classify(last) {
        LineKind::Password | LineKind::Login => Some(format!(
            "终端正在等待登录（{}），需要用户自己输入，不能由助手代填。",
            last.trim()
        )),
        LineKind::More => {
            Some("终端停在分页提示上。先用 terminal_key 发送 q 或 ctrl_c 退出分页。".into())
        }
        _ => None,
    }
}

/// Types `input` and collects what comes back until the device is ready for
/// the next command. `echo` is the command line, removed from the output when
/// the device echoes it.
pub async fn drive<W, F>(
    write: W,
    rx: &mut mpsc::UnboundedReceiver<Vec<u8>>,
    input: &[u8],
    echo: Option<&str>,
    timeout: Duration,
    timing: Timing,
) -> Result<Outcome, String>
where
    W: Fn(Vec<u8>) -> F,
    F: Future<Output = Result<(), String>>,
{
    // Whatever arrived before this command is not its output.
    while rx.try_recv().is_ok() {}
    write(input.to_vec()).await?;

    let deadline = Instant::now() + timeout;
    let mut raw: Vec<u8> = Vec::new();
    let mut last_data = Instant::now();
    let mut pages = 0;
    let mut truncated = false;
    // Completed lines when the pager was last answered: the same `More` line
    // must not be answered twice.
    let mut answered_at: Option<usize> = None;

    let ended = loop {
        let now = Instant::now();
        if now >= deadline {
            break Ended::Timeout;
        }
        let wait = timing.quiet.min(deadline - now);
        match tokio::time::timeout(wait, rx.recv()).await {
            Ok(Some(chunk)) => {
                raw.extend_from_slice(&chunk);
                last_data = Instant::now();
                let lines = render(&raw);
                let done_lines = lines.len() - 1;
                let last = lines.last().map(String::as_str).unwrap_or("");
                if raw.len() > MAX_RAW && !truncated {
                    truncated = true;
                    write(b"\x03".to_vec()).await?;
                } else if !truncated
                    && classify(last) == LineKind::More
                    && answered_at != Some(done_lines)
                {
                    answered_at = Some(done_lines);
                    if pages < MAX_PAGES {
                        pages += 1;
                        write(b" ".to_vec()).await?;
                    } else {
                        truncated = true;
                        write(b"\x03".to_vec()).await?;
                    }
                }
            }
            Ok(None) => break Ended::Closed,
            Err(_) => {
                let lines = render(&raw);
                let last = lines.last().map(String::as_str).unwrap_or("").trim();
                match classify(last) {
                    LineKind::Prompt => break Ended::Prompt(last.to_string()),
                    LineKind::Password | LineKind::Login | LineKind::Confirm => {
                        break Ended::Input(last.to_string())
                    }
                    _ if last_data.elapsed() >= timing.idle => break Ended::Idle,
                    _ => {}
                }
            }
        }
    };

    let mut lines = render(&raw);
    // The echo of the command line itself.
    if let Some(cmd) = echo.map(str::trim).filter(|c| !c.is_empty()) {
        if let Some(i) = lines.iter().position(|l| !l.trim().is_empty()) {
            if lines[i].trim_end().ends_with(cmd) {
                lines.drain(..=i);
            }
        }
    }
    // The prompt is reported on its own, not as output.
    if matches!(ended, Ended::Prompt(_)) {
        lines.pop();
    }
    while lines.last().is_some_and(|l| l.trim().is_empty()) {
        lines.pop();
    }
    let mut output = lines.join("\n");
    if output.len() > MAX_OUTPUT {
        let mut cut = MAX_OUTPUT;
        while !output.is_char_boundary(cut) {
            cut -= 1;
        }
        output.truncate(cut);
        truncated = true;
    }
    Ok(Outcome {
        output,
        ended,
        pages,
        truncated,
    })
}

/// Lays out terminal output as the screen would show it, one string per line.
///
/// Just enough of a terminal for command output: carriage return, backspace,
/// cursor left/right and erase-line, which is how devices wipe their pager
/// prompt (`---- More ----` overwritten with spaces after `ESC[16D`). Colours
/// and other sequences are dropped. The last element is the line the cursor
/// is on, possibly empty.
pub fn render(raw: &[u8]) -> Vec<String> {
    let text = String::from_utf8_lossy(raw);
    let mut lines = Vec::new();
    let mut cur: Vec<char> = Vec::new();
    let mut col = 0usize;
    let mut it = text.chars().peekable();

    let put = |cur: &mut Vec<char>, col: &mut usize, c: char| {
        if *col < cur.len() {
            cur[*col] = c;
        } else {
            cur.resize(*col, ' ');
            cur.push(c);
        }
        *col += 1;
    };

    while let Some(c) = it.next() {
        match c {
            '\n' => {
                lines.push(finish(&cur));
                cur.clear();
                col = 0;
            }
            '\r' => col = 0,
            '\x08' => col = col.saturating_sub(1),
            '\t' => {
                put(&mut cur, &mut col, ' ');
                while col % 8 != 0 {
                    put(&mut cur, &mut col, ' ');
                }
            }
            '\x1b' => match it.next() {
                Some('[') => {
                    let mut params = String::new();
                    let mut fin = None;
                    for p in it.by_ref() {
                        if ('\x40'..='\x7e').contains(&p) {
                            fin = Some(p);
                            break;
                        }
                        params.push(p);
                    }
                    let n = params
                        .trim_start_matches('?')
                        .split(';')
                        .next()
                        .and_then(|v| v.parse::<usize>().ok());
                    match fin {
                        Some('D') => col = col.saturating_sub(n.unwrap_or(1).max(1)),
                        Some('C') => col += n.unwrap_or(1).max(1),
                        Some('G') => col = n.unwrap_or(1).max(1) - 1,
                        Some('K') => match n.unwrap_or(0) {
                            0 => cur.truncate(col),
                            1 => cur.iter_mut().take(col).for_each(|c| *c = ' '),
                            _ => cur.clear(),
                        },
                        _ => {}
                    }
                }
                // OSC (window title and the like) ends with BEL or ESC \.
                Some(']') => {
                    while let Some(p) = it.next() {
                        if p == '\x07' || (p == '\x1b' && it.next_if_eq(&'\\').is_some()) {
                            break;
                        }
                    }
                }
                // Charset selection carries one more byte.
                Some('(') | Some(')') => {
                    it.next();
                }
                _ => {}
            },
            c if c.is_control() => {}
            c => put(&mut cur, &mut col, c),
        }
    }
    lines.push(finish(&cur));
    lines
}

fn finish(cur: &[char]) -> String {
    cur.iter().collect::<String>().trim_end().to_string()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineKind {
    Prompt,
    More,
    Password,
    Login,
    Confirm,
    Other,
}

/// What the line the cursor sits on asks for.
pub fn classify(line: &str) -> LineKind {
    let t = line.trim();
    if t.is_empty() || t.chars().count() > 120 {
        return LineKind::Other;
    }
    let lower = t.to_lowercase();

    let core = lower.trim_matches(|c: char| c == ' ' || c == '-' || c == '<' || c == '>');
    if core.starts_with("more") && core.len() <= 24 {
        return LineKind::More;
    }
    let asks = t.ends_with(':') || t.ends_with('：');
    if asks && (lower.contains("password") || lower.contains("passwd") || t.contains("密码")) {
        return LineKind::Password;
    }
    if asks
        && (lower.ends_with("username:")
            || lower.ends_with("user name:")
            || lower.ends_with("login:")
            || t.contains("用户名"))
    {
        return LineKind::Login;
    }
    if lower.contains("y/n") || lower.contains("yes/no") {
        return LineKind::Confirm;
    }

    // `<sw01>` and `[sw01-GigabitEthernet1/0/1]` (H3C, Huawei; V8 adds `~`
    // or `*` inside the bracket).
    let bracketed = |open: char, close: char| {
        t.starts_with(open) && t.ends_with(close) && t.len() > 2 && !t.contains(' ')
    };
    if bracketed('<', '>') || bracketed('[', ']') {
        return LineKind::Prompt;
    }
    if let Some(last) = t.chars().last().filter(|c| "#>$%".contains(*c)) {
        let body = &t[..t.len() - last.len_utf8()];
        // `sw01#`, `sw01(config-if)#`, `user@host:~$`
        if !body.is_empty() && !body.contains(' ') {
            return LineKind::Prompt;
        }
        // `[root@host ~]#`, `(venv) user@host:~/src$`, macOS zsh's
        // `user@mac ~ %`
        if last != '>' && body.contains('@') {
            return LineKind::Prompt;
        }
        // PowerShell: `PS C:\Users\me>`
        if last == '>' && body.starts_with("PS ") && body.contains(':') {
            return LineKind::Prompt;
        }
    }
    LineKind::Other
}

/// Commands that must be confirmed even when the user let everything run:
/// they reboot, erase, or can cut the connection the user is on.
pub fn is_dangerous(command: &str) -> bool {
    let lower = command.to_lowercase();
    let words: Vec<&str> = lower.split_whitespace().collect();
    let first = match words.first() {
        Some(&"sudo") => words.get(1).copied().unwrap_or(""),
        Some(w) => w,
        None => return false,
    };
    const FIRST: &[&str] = &[
        "reboot", "reload", "poweroff", "halt", "shutdown", "format", "mkfs", "delete", "erase",
        "rmdir",
    ];
    const ANYWHERE: &[&str] = &[
        "reset saved-configuration",
        "write erase",
        "init 0",
        "init 6",
        "rm -rf",
        "rm -fr",
        "rm -r",
        "dd if=",
        "mkfs.",
        "factory",
        "> /dev/sd",
    ];
    let joined = words.join(" ");
    FIRST.contains(&first)
        || first.starts_with("mkfs")
        || ANYWHERE.iter().any(|p| joined.contains(p))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prompts_of_common_devices_are_recognised() {
        for p in [
            "<core-sw01>",
            "[core-sw01]",
            "[core-sw01-GigabitEthernet1/0/1]",
            "[~HUAWEI-10GE1/0/1]",
            "sw01>",
            "sw01#",
            "sw01(config-if)#",
            "user@host:~$ ",
            "root@web:/etc#",
            "[root@db ~]#",
            "(venv) dev@box:~/src$",
            "xuan@MacBook-Pro ~ %",
            r"PS C:\Users\xuan>",
            r"C:\Users\xuan>",
        ] {
            assert_eq!(classify(p), LineKind::Prompt, "{p:?}");
        }
        for o in [
            "Interface            PHY   Protocol  Description",
            "#",
            "GE1/0/1              UP    UP        uplink to core",
            " return",
            "",
            "Building configuration...",
            "CPU usage: 12%",
            "-> next hop 10.0.0.1 via eth0 >",
        ] {
            assert_ne!(classify(o), LineKind::Prompt, "{o:?}");
        }
    }

    #[test]
    fn questions_and_pagers_are_recognised() {
        assert_eq!(classify("  ---- More ----"), LineKind::More);
        assert_eq!(classify(" --More-- "), LineKind::More);
        assert_eq!(classify("<--- More --->"), LineKind::More);
        assert_eq!(classify("Password:"), LineKind::Password);
        assert_eq!(classify("请输入密码："), LineKind::Password);
        assert_eq!(classify("Username:"), LineKind::Login);
        assert_eq!(
            classify(
                "The current configuration will be written to the device. Are you sure? [Y/N]:"
            ),
            LineKind::Confirm
        );
        assert_eq!(classify("Continue? (yes/no)"), LineKind::Confirm);
    }

    #[test]
    fn rendering_wipes_the_pager_prompt_like_a_screen() {
        // H3C: prompt, then cursor back 16 and blanks over it.
        let raw = b"line one\r\n  ---- More ----\x1b[16D                \x1b[16Dline two\r\n<sw>";
        assert_eq!(render(raw), vec!["line one", "line two", "<sw>"]);
        // Cisco: backspaces over ` --More-- `.
        let raw = b"a\r\n --More-- \x08\x08\x08\x08\x08\x08\x08\x08\x08\x08          \x08\x08\x08\x08\x08\x08\x08\x08\x08\x08b\r\nsw#";
        assert_eq!(render(raw), vec!["a", "b", "sw#"]);
        // Colours vanish; erase-line works.
        assert_eq!(
            render(b"\x1b[1;32mok\x1b[0m\r\nxyz\rab\x1b[K"),
            vec!["ok", "ab"]
        );
    }

    #[test]
    fn dangerous_commands_are_flagged() {
        for c in [
            "reboot",
            "sudo reboot",
            "reset saved-configuration",
            "shutdown",
            "delete /unreserved flash:/startup.cfg",
            "rm -rf /var/log/nginx",
            "format flash:",
        ] {
            assert!(is_dangerous(c), "{c}");
        }
        for c in [
            "display interface brief",
            "ls -la",
            "show run",
            "undo shutdown",
            "save",
        ] {
            assert!(!is_dangerous(c), "{c}");
        }
    }

    /// A fake switch on a channel pair: answers each line typed with the
    /// scripted reply, pages with `---- More ----`.
    #[allow(clippy::type_complexity)]
    fn fake_device(
        script: Vec<(&'static str, Vec<&'static str>)>,
    ) -> (
        impl Fn(Vec<u8>) -> std::future::Ready<Result<(), String>>,
        mpsc::UnboundedReceiver<Vec<u8>>,
    ) {
        let (out_tx, out_rx) = mpsc::unbounded_channel::<Vec<u8>>();
        let (in_tx, mut in_rx) = mpsc::unbounded_channel::<Vec<u8>>();
        tokio::spawn(async move {
            let mut line = Vec::new();
            let mut pages: Vec<&str> = Vec::new();
            while let Some(bytes) = in_rx.recv().await {
                for b in bytes {
                    if !pages.is_empty() && b == b' ' {
                        let _ = out_tx.send(b"\x1b[16D                \x1b[16D".to_vec());
                        let next = pages.remove(0);
                        let more = if pages.is_empty() {
                            ""
                        } else {
                            "  ---- More ----"
                        };
                        let _ = out_tx.send(format!("{next}{more}").into_bytes());
                        continue;
                    }
                    if b == b'\r' {
                        let cmd = String::from_utf8_lossy(&line).to_string();
                        line.clear();
                        let _ = out_tx.send(format!("{cmd}\r\n").into_bytes());
                        if let Some((_, reply)) = script.iter().find(|(c, _)| *c == cmd) {
                            let mut reply = reply.clone();
                            let first = reply.remove(0);
                            pages = reply;
                            let more = if pages.is_empty() {
                                ""
                            } else {
                                "  ---- More ----"
                            };
                            let _ = out_tx.send(format!("{first}{more}").into_bytes());
                        }
                    } else {
                        line.push(b);
                    }
                }
            }
        });
        let write = move |data: Vec<u8>| {
            let _ = in_tx.send(data);
            std::future::ready(Ok(()))
        };
        (write, out_rx)
    }

    fn fast() -> Timing {
        Timing {
            quiet: Duration::from_millis(40),
            idle: Duration::from_millis(300),
        }
    }

    #[tokio::test]
    async fn pages_are_turned_and_the_prompt_ends_the_command() {
        let (write, mut rx) = fake_device(vec![(
            "display interface brief",
            vec!["GE1/0/1 UP\r\n", "GE1/0/2 DOWN\r\n", "GE1/0/3 UP\r\n<sw01>"],
        )]);
        let out = drive(
            write,
            &mut rx,
            b"display interface brief\r",
            Some("display interface brief"),
            Duration::from_secs(5),
            fast(),
        )
        .await
        .unwrap();
        assert_eq!(out.ended, Ended::Prompt("<sw01>".into()));
        assert_eq!(out.pages, 2);
        assert_eq!(out.output, "GE1/0/1 UP\nGE1/0/2 DOWN\nGE1/0/3 UP");
        assert!(!out.output.contains("More"));
    }

    #[tokio::test]
    async fn a_question_hands_control_back() {
        let (write, mut rx) = fake_device(vec![("save", vec!["Are you sure? [Y/N]:"])]);
        let out = drive(
            write,
            &mut rx,
            b"save\r",
            Some("save"),
            Duration::from_secs(5),
            fast(),
        )
        .await
        .unwrap();
        assert_eq!(out.ended, Ended::Input("Are you sure? [Y/N]:".into()));
        assert!(out.report().contains("等待输入"));
    }

    #[tokio::test]
    async fn silence_without_a_prompt_ends_as_idle_and_a_deadline_as_timeout() {
        let (write, mut rx) = fake_device(vec![("ping", vec!["Reply from 10.0.0.1"])]);
        let out = drive(
            write,
            &mut rx,
            b"ping\r",
            Some("ping"),
            Duration::from_secs(5),
            fast(),
        )
        .await
        .unwrap();
        assert_eq!(out.ended, Ended::Idle);
        assert_eq!(out.output, "Reply from 10.0.0.1");

        let (write, mut rx) = fake_device(vec![]);
        let slow = Timing {
            quiet: Duration::from_millis(40),
            idle: Duration::from_secs(60),
        };
        let out = drive(
            write,
            &mut rx,
            b"hang\r",
            None,
            Duration::from_millis(200),
            slow,
        )
        .await
        .unwrap();
        assert_eq!(out.ended, Ended::Timeout);
    }

    #[test]
    fn typing_is_refused_at_a_login_or_pager() {
        assert!(refuse_reason(b"\r\nPassword:").is_some());
        assert!(refuse_reason(b"line\r\n  ---- More ----").is_some());
        assert!(refuse_reason(b"\r\n<sw01>").is_none());
    }

    #[test]
    fn only_listed_keys_can_be_pressed() {
        assert_eq!(key_bytes("ctrl_c"), Some(&b"\x03"[..]));
        assert_eq!(key_bytes("y"), Some(&b"y"[..]));
        assert_eq!(key_bytes("hunter2"), None);
    }
}
