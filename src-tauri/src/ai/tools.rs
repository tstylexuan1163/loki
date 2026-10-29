//! The tools the assistant may call, and the permission gate in front of them.
//!
//! Every tool runs in the Rust process and touches either the local machine, the
//! session's SSH connection, or the network. Nothing here can reach the WebView's
//! storage or the vault keys.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::sync::Arc;

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
pub fn may_run_unattended(mode: PermissionMode, tool: &str) -> bool {
    match mode {
        PermissionMode::Manual => false,
        PermissionMode::Auto => is_read_only(tool),
        PermissionMode::Bypass => true,
    }
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

/// Everything a tool call might need.
pub struct ToolContext {
    pub session: Option<Arc<LiveSession>>,
    /// Recent terminal output, for `read_terminal`.
    pub terminal_tail: String,
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
        ctx.session
            .clone()
            .ok_or_else(|| ToolOutcome::err("还没有打开的 SSH 会话"))
    };

    match name {
        "read_terminal" => {
            let lines = args
                .get("lines")
                .and_then(|v| v.as_u64())
                .unwrap_or(40)
                .min(200) as usize;
            let tail: Vec<&str> = ctx.terminal_tail.lines().rev().take(lines).collect();
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auto_mode_only_allows_reads() {
        assert!(may_run_unattended(PermissionMode::Auto, "read_file"));
        assert!(may_run_unattended(PermissionMode::Auto, "list_directory"));
        // Anything that can change the server must be confirmed.
        assert!(!may_run_unattended(PermissionMode::Auto, "run_command"));
        assert!(!may_run_unattended(PermissionMode::Auto, "write_file"));
    }

    #[test]
    fn manual_never_runs_unattended() {
        for tool in ["read_file", "run_command", "grep"] {
            assert!(!may_run_unattended(PermissionMode::Manual, tool), "{tool}");
        }
    }

    #[test]
    fn bypass_runs_everything() {
        assert!(may_run_unattended(PermissionMode::Bypass, "run_command"));
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
            terminal_tail: String::new(),
        };
        let out = run("does_not_exist", &json!({}), &ctx).await;
        assert!(!out.ok);
    }

    #[tokio::test]
    async fn read_terminal_returns_the_tail() {
        let ctx = ToolContext {
            session: None,
            terminal_tail: "one\ntwo\nthree".into(),
        };
        let out = run("read_terminal", &json!({"lines": 2}), &ctx).await;
        assert!(out.ok);
        assert_eq!(out.content, "two\nthree");
    }

    #[tokio::test]
    async fn tools_needing_a_session_say_so() {
        let ctx = ToolContext {
            session: None,
            terminal_tail: String::new(),
        };
        let out = run("read_file", &json!({"path": "/etc/hosts"}), &ctx).await;
        assert!(!out.ok);
        assert!(out.content.contains("会话"));
    }
}
