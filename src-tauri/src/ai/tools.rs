//! The tools the assistant may call, and the permission gate in front of them.
//!
//! Every tool runs in the Rust process and touches either the local machine, the
//! session's SSH connection, or the network. Nothing here can reach the WebView's
//! storage or the vault keys.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::sync::Arc;
use std::time::Duration;

use crate::ssh::LiveSession;

/// How much the assistant may do without asking.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PermissionMode {
    /// Every tool call waits for the user.
    Manual,
    /// Read-only tools run; anything that changes state waits.
    Auto,
    /// Everything runs. The user opted in explicitly in settings.
    Bypass,
}

/// Which tools are safe to run unattended in `Auto` mode.
pub fn is_read_only(tool: &str) -> bool {
    matches!(
        tool,
        "read_terminal" | "list_directory" | "read_file" | "grep" | "search_web" | "fetch_url"
    )
}

/// Decides whether a call may proceed.
///
/// A command that reboots, erases or can cut the connection is asked about
/// even when the user let everything run: one wrong guess there is not
/// undone by reading the transcript afterwards.
pub fn may_run_unattended(mode: PermissionMode, tool: &str, args: &Value) -> bool {
    match mode {
        PermissionMode::Manual => false,
        PermissionMode::Auto => is_read_only(tool),
        PermissionMode::Bypass => !is_dangerous_call(tool, args),
    }
}

/// Whether a call runs a command from the always-ask list.
pub fn is_dangerous_call(tool: &str, args: &Value) -> bool {
    matches!(tool, "run_command" | "type_in_terminal")
        && args
            .get("command")
            .and_then(|v| v.as_str())
            .is_some_and(super::interactive::is_dangerous)
}

/// The tool catalogue, described to the model in JSON Schema.
pub fn catalogue() -> Vec<super::provider::ToolSpec> {
    use super::provider::ToolSpec;
    let spec = |name: &str, description: &str, parameters: Value| ToolSpec {
        name: name.to_string(),
        description: description.to_string(),
        parameters,
    };

    vec![
        spec(
            "read_terminal",
            "读取当前终端最近若干行内容。用于了解屏幕上发生了什么。",
            json!({
                "type": "object",
                "properties": {
                    "lines": { "type": "integer", "description": "读取的最后行数，默认 40" }
                },
                "required": []
            }),
        ),
        spec(
            "run_command",
            "在这台 SSH 主机的终端里执行一条命令并返回输出。会改变服务器状态，请先确认。",
            json!({
                "type": "object",
                "properties": {
                    "command": { "type": "string", "description": "要执行的 shell 命令" }
                },
                "required": ["command"]
            }),
        ),
        spec(
            "type_in_terminal",
            "把一行命令输入到用户正在看的终端里（和用户自己打字一样），等设备回到提示符后返回输出。\
适合交换机、路由器等只有交互式命令行的设备，以及需要保持状态的操作（system-view、接口视图、su）。\
分页（---- More ----）会自动翻页，不需要先关闭分页。遇到密码或 [Y/N] 这类提问会停下，交给用户。\
会改变设备状态，请先确认。",
            json!({
                "type": "object",
                "properties": {
                    "command": { "type": "string", "description": "要输入的一行命令" },
                    "timeout_secs": { "type": "integer", "description": "最多等待的秒数，默认 30，最大 300" }
                },
                "required": ["command"]
            }),
        ),
        spec(
            "terminal_key",
            "在终端里按一个键：ctrl_c 中断命令，q 退出分页，y / n 回答确认提问，enter、space。只能按这几个键。",
            json!({
                "type": "object",
                "properties": {
                    "key": { "type": "string", "enum": ["ctrl_c", "enter", "space", "q", "y", "n"] }
                },
                "required": ["key"]
            }),
        ),
        spec(
            "list_directory",
            "列出远端目录内容。",
            json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "目录路径，留空为 home" }
                },
                "required": []
            }),
        ),
        spec(
            "read_file",
            "读取远端文本文件的内容（上限 4 MB）。",
            json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "文件路径" }
                },
                "required": ["path"]
            }),
        ),
        spec(
            "write_file",
            "写入或覆盖远端文件。会改变服务器状态，请先确认。",
            json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string" },
                    "content": { "type": "string" }
                },
                "required": ["path", "content"]
            }),
        ),
        spec(
            "grep",
            "在远端用 grep 搜索文件内容。",
            json!({
                "type": "object",
                "properties": {
                    "pattern": { "type": "string", "description": "正则或字面量" },
                    "path": { "type": "string", "description": "搜索起点，默认当前目录" }
                },
                "required": ["pattern"]
            }),
        ),
    ]
}

/// Tools that need an SSH connection (an exec channel or SFTP). A telnet,
/// serial or local session has neither.
const SSH_ONLY: &[&str] = &[
    "run_command",
    "list_directory",
    "read_file",
    "write_file",
    "grep",
];
/// Tools that type into the user's terminal.
const TYPING: &[&str] = &["type_in_terminal", "terminal_key"];

/// The catalogue, less what this session cannot do. Offering a tool that can
/// only fail invites the model to keep trying it.
pub fn offered(ssh: bool, typing: bool) -> Vec<super::provider::ToolSpec> {
    catalogue()
        .into_iter()
        .filter(|t| ssh || !SSH_ONLY.contains(&t.name.as_str()))
        .filter(|t| typing || !TYPING.contains(&t.name.as_str()))
        .collect()
}

/// Everything a tool call might need.
pub struct ToolContext {
    pub session: Option<Arc<LiveSession>>,
    /// The terminal the assistant may type into and read live.
    pub terminal: Option<super::interactive::Terminal>,
    /// Recent terminal output when the turn began; `read_terminal` falls back
    /// to it when there is no live terminal.
    pub terminal_tail: String,
    /// Remote paths of attached private keys: never read into the model.
    pub secret_paths: Vec<String>,
}

/// The outcome of running a tool, as the model sees it.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolOutcome {
    pub ok: bool,
    pub content: String,
}

impl ToolOutcome {
    pub fn ok(content: impl Into<String>) -> Self {
        Self {
            ok: true,
            content: content.into(),
        }
    }

    pub fn err(content: impl Into<String>) -> Self {
        Self {
            ok: false,
            content: content.into(),
        }
    }
}

/// Runs one tool call.
pub async fn run(name: &str, args: &Value, ctx: &ToolContext) -> ToolOutcome {
    let need_session = |ctx: &ToolContext| -> Result<Arc<LiveSession>, ToolOutcome> {
        ctx.session.clone().ok_or_else(|| {
            ToolOutcome::err(if ctx.terminal.is_some() {
                "这个工具只能在 SSH 会话里用，当前会话请改用 type_in_terminal"
            } else {
                "还没有打开的会话"
            })
        })
    };

    match name {
        "read_terminal" => {
            let lines = args
                .get("lines")
                .and_then(|v| v.as_u64())
                .unwrap_or(40)
                .min(200) as usize;
            // Read now, not when the turn began: after typing a command the
            // screen has moved on. Key bodies are stripped like any output.
            let screen = match &ctx.terminal {
                Some(t) => crate::ai::attach::redact_private_keys(
                    &super::interactive::render(&t.screen()).join("\n"),
                ),
                None => ctx.terminal_tail.clone(),
            };
            let tail: Vec<&str> = screen.lines().rev().take(lines).collect();
            let text = tail.into_iter().rev().collect::<Vec<_>>().join("\n");
            if text.is_empty() {
                ToolOutcome::ok("(终端还没有输出)")
            } else {
                ToolOutcome::ok(text)
            }
        }

        "run_command" => {
            let command = args.get("command").and_then(|v| v.as_str()).unwrap_or("");
            if command.trim().is_empty() {
                return ToolOutcome::err("缺少 command");
            }
            // Executed on a separate exec channel so it does not disturb the
            // user's interactive shell or its scrollback.
            let session = match need_session(ctx) {
                Ok(s) => s,
                Err(e) => return e,
            };
            match crate::ssh::exec_command(&session, command).await {
                Ok(out) => ToolOutcome::ok(out),
                Err(e) => ToolOutcome::err(e),
            }
        }

        "type_in_terminal" => {
            let command = args.get("command").and_then(|v| v.as_str()).unwrap_or("");
            if command.trim().is_empty() {
                return ToolOutcome::err("缺少 command");
            }
            let Some(term) = &ctx.terminal else {
                return ToolOutcome::err("当前没有可以输入的终端");
            };
            let secs = args
                .get("timeout_secs")
                .and_then(|v| v.as_u64())
                .unwrap_or(30)
                .clamp(1, 300);
            match super::interactive::type_line(term, command, Duration::from_secs(secs)).await {
                Ok(out) => ToolOutcome::ok(out.report()),
                Err(e) => ToolOutcome::err(e),
            }
        }

        "terminal_key" => {
            let key = args.get("key").and_then(|v| v.as_str()).unwrap_or("");
            let Some(term) = &ctx.terminal else {
                return ToolOutcome::err("当前没有可以输入的终端");
            };
            match super::interactive::press(term, key, Duration::from_secs(10)).await {
                Ok(out) => ToolOutcome::ok(out.report()),
                Err(e) => ToolOutcome::err(e),
            }
        }

        "list_directory" => {
            let path = args.get("path").and_then(|v| v.as_str()).unwrap_or("");
            let session = match need_session(ctx) {
                Ok(s) => s,
                Err(e) => return e,
            };
            match crate::sftp::list_for_session(&session, path).await {
                Ok(entries) => {
                    let mut out = String::new();
                    for e in entries.iter().take(200) {
                        out.push_str(&format!(
                            "{}\t{}\t{}\n",
                            if e.kind == "dir" { "dir " } else { "file" },
                            e.size,
                            e.name
                        ));
                    }
                    ToolOutcome::ok(if out.is_empty() {
                        "（空目录）".into()
                    } else {
                        out
                    })
                }
                Err(e) => ToolOutcome::err(e),
            }
        }

        "read_file" => {
            let path = args.get("path").and_then(|v| v.as_str()).unwrap_or("");
            // A private key the user attached stays opaque even by path: the
            // model may install it, not read it.
            if ctx.secret_paths.iter().any(|p| same_remote_file(p, path)) {
                return ToolOutcome::err("这是私钥文件，内容不提供。用路径操作即可。");
            }
            let session = match need_session(ctx) {
                Ok(s) => s,
                Err(e) => return e,
            };
            match crate::sftp::read_for_session(&session, path).await {
                Ok(text) => ToolOutcome::ok(text),
                Err(e) => ToolOutcome::err(e),
            }
        }

        "write_file" => {
            let path = args.get("path").and_then(|v| v.as_str()).unwrap_or("");
            let content = args.get("content").and_then(|v| v.as_str()).unwrap_or("");
            let session = match need_session(ctx) {
                Ok(s) => s,
                Err(e) => return e,
            };
            match crate::sftp::write_for_session(&session, path, content).await {
                Ok(()) => ToolOutcome::ok(format!("已写入 {path}")),
                Err(e) => ToolOutcome::err(e),
            }
        }

        "grep" => {
            let pattern = args.get("pattern").and_then(|v| v.as_str()).unwrap_or("");
            if pattern.is_empty() {
                return ToolOutcome::err("缺少 pattern");
            }
            let path = args.get("path").and_then(|v| v.as_str()).unwrap_or(".");
            // The pattern is passed through single quotes; an embedded quote is
            // escaped the POSIX way so it cannot break out into the shell.
            let safe = pattern.replace('\'', "'\\''");
            let safe_path = path.replace('\'', "'\\''");
            let cmd = format!("grep -rn --color=never -e '{safe}' '{safe_path}' 2>&1 | head -100");
            let session = match need_session(ctx) {
                Ok(s) => s,
                Err(e) => return e,
            };
            match crate::ssh::exec_command(&session, &cmd).await {
                Ok(out) => ToolOutcome::ok(out),
                Err(e) => ToolOutcome::err(e),
            }
        }

        other => ToolOutcome::err(format!("未知工具：{other}")),
    }
}

/// Whether `asked` names the staged file at `staged` (an absolute path).
///
/// The model is told the absolute path but may well write it as `~/…` or with
/// a stray `./` or doubled slash; all of those still mean the key.
fn same_remote_file(staged: &str, asked: &str) -> bool {
    fn tidy(p: &str) -> String {
        let mut out: Vec<&str> = Vec::new();
        for seg in p.split('/') {
            if !seg.is_empty() && seg != "." {
                out.push(seg);
            }
        }
        out.join("/")
    }
    let staged = tidy(staged);
    let asked = asked.trim();
    match asked.strip_prefix("~/") {
        // `~/.loki/…` matches whatever the home is: the staged path ends in it.
        // Anything else under `~` is a different file.
        Some(rest) => {
            let rest = tidy(rest);
            rest.starts_with(".loki/") && staged.ends_with(&format!("/{rest}"))
        }
        None => tidy(asked) == staged,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auto_mode_only_allows_reads() {
        assert!(may_run_unattended(
            PermissionMode::Auto,
            "read_file",
            &json!({})
        ));
        assert!(may_run_unattended(
            PermissionMode::Auto,
            "list_directory",
            &json!({})
        ));
        // Anything that can change the server must be confirmed.
        assert!(!may_run_unattended(
            PermissionMode::Auto,
            "run_command",
            &json!({})
        ));
        assert!(!may_run_unattended(
            PermissionMode::Auto,
            "write_file",
            &json!({})
        ));
    }

    #[test]
    fn manual_never_runs_unattended() {
        for tool in ["read_file", "run_command", "grep"] {
            assert!(
                !may_run_unattended(PermissionMode::Manual, tool, &json!({})),
                "{tool}"
            );
        }
    }

    #[test]
    fn bypass_runs_everything_but_the_always_ask_list() {
        let ok = json!({ "command": "display version" });
        assert!(may_run_unattended(
            PermissionMode::Bypass,
            "run_command",
            &ok
        ));
        assert!(may_run_unattended(
            PermissionMode::Bypass,
            "type_in_terminal",
            &ok
        ));
        let risky = json!({ "command": "reset saved-configuration" });
        assert!(!may_run_unattended(
            PermissionMode::Bypass,
            "type_in_terminal",
            &risky
        ));
        assert!(!may_run_unattended(
            PermissionMode::Bypass,
            "run_command",
            &json!({ "command": "sudo reboot" })
        ));
    }

    #[test]
    fn typing_into_the_terminal_is_never_a_read() {
        let args = json!({ "command": "display version" });
        assert!(!may_run_unattended(
            PermissionMode::Auto,
            "type_in_terminal",
            &args
        ));
        assert!(!may_run_unattended(
            PermissionMode::Auto,
            "terminal_key",
            &json!({ "key": "y" })
        ));
    }

    #[test]
    fn a_session_without_ssh_is_offered_only_the_terminal() {
        let names = |ssh, typing| -> Vec<String> {
            offered(ssh, typing).into_iter().map(|t| t.name).collect()
        };
        assert_eq!(names(true, true).len(), catalogue().len());
        assert_eq!(
            names(false, true),
            ["read_terminal", "type_in_terminal", "terminal_key"]
        );
        assert_eq!(names(false, false), ["read_terminal"]);
    }

    #[test]
    fn catalogue_has_valid_schemas() {
        for tool in catalogue() {
            assert!(
                tool.parameters.get("type").is_some(),
                "{} lacks a type",
                tool.name
            );
        }
    }

    #[tokio::test]
    async fn unknown_tool_is_an_error_not_a_panic() {
        let ctx = ToolContext {
            session: None,
            terminal: None,
            terminal_tail: String::new(),
            secret_paths: Vec::new(),
        };
        let out = run("does_not_exist", &json!({}), &ctx).await;
        assert!(!out.ok);
    }

    #[tokio::test]
    async fn read_terminal_returns_the_tail() {
        let ctx = ToolContext {
            session: None,
            terminal: None,
            terminal_tail: "one\ntwo\nthree".into(),
            secret_paths: Vec::new(),
        };
        let out = run("read_terminal", &json!({"lines": 2}), &ctx).await;
        assert!(out.ok);
        assert_eq!(out.content, "two\nthree");
    }

    #[tokio::test]
    async fn a_secret_attachment_is_refused_by_path() {
        let ctx = ToolContext {
            session: None,
            terminal: None,
            terminal_tail: String::new(),
            secret_paths: vec!["~/.loki/uploads/x/server.key".into()],
        };
        let out = run(
            "read_file",
            &json!({ "path": "~/.loki/uploads/x/server.key" }),
            &ctx,
        )
        .await;
        assert!(!out.ok);
        assert!(out.content.contains("私钥"), "{}", out.content);
    }

    #[test]
    fn a_staged_key_is_recognised_however_the_path_is_written() {
        let staged = "/home/deploy/.loki/uploads/20261030-090000-ab12/server.key";
        assert!(same_remote_file(staged, staged));
        assert!(same_remote_file(
            staged,
            "~/.loki/uploads/20261030-090000-ab12/server.key"
        ));
        assert!(same_remote_file(
            staged,
            "/home/deploy//.loki/./uploads/20261030-090000-ab12/server.key"
        ));
        assert!(!same_remote_file(staged, "/etc/ssl/server.key"));
        assert!(!same_remote_file(staged, "~/"));
        assert!(!same_remote_file(staged, "~/server.key"));
    }

    #[tokio::test]
    async fn tools_needing_a_session_say_so() {
        let ctx = ToolContext {
            session: None,
            terminal: None,
            terminal_tail: String::new(),
            secret_paths: Vec::new(),
        };
        let out = run("read_file", &json!({"path": "/etc/hosts"}), &ctx).await;
        assert!(!out.ok);
        assert!(out.content.contains("会话"));
    }
}
