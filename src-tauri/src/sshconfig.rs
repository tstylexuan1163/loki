//! Reads `~/.ssh/config` so existing hosts can be imported in one step.
//!
//! Only the directives that map onto a loki host are understood: `HostName`,
//! `User`, `Port`, `IdentityFile` and `ProxyJump`. Wildcard patterns are not
//! hosts, but a `Host *` block supplies defaults the way OpenSSH applies them.

use serde::Serialize;

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SshConfigEntry {
    /// The `Host` alias, used as the loki host name.
    pub alias: String,
    pub hostname: String,
    pub user: String,
    pub port: u16,
    pub identity_file: Option<String>,
    /// First hop of `ProxyJump`, as written.
    pub proxy_jump: Option<String>,
}

#[derive(Default, Clone)]
struct Block {
    hostname: Option<String>,
    user: Option<String>,
    port: Option<u16>,
    identity_file: Option<String>,
    proxy_jump: Option<String>,
}

impl Block {
    /// OpenSSH uses the *first* value it sees for each directive.
    fn set(&mut self, key: &str, value: &str) {
        match key {
            "hostname" => {
                self.hostname.get_or_insert_with(|| value.to_string());
            }
            "user" => {
                self.user.get_or_insert_with(|| value.to_string());
            }
            "port" => {
                if self.port.is_none() {
                    self.port = value.parse().ok();
                }
            }
            "identityfile" => {
                self.identity_file.get_or_insert_with(|| value.to_string());
            }
            // Only the first hop of a chain is representable here.
            "proxyjump" if self.proxy_jump.is_none() && !value.eq_ignore_ascii_case("none") => {
                self.proxy_jump = value.split(',').next().map(|s| s.trim().to_string());
            }
            _ => {}
        }
    }
}

fn expand_home(path: &str, home: &str) -> String {
    if let Some(rest) = path.strip_prefix("~/") {
        format!("{home}/{rest}")
    } else if path == "~" {
        home.to_string()
    } else {
        path.replace("%d", home)
    }
}

/// Parses config text. `home` expands `~` in `IdentityFile`; `default_user`
/// fills `User` when the file does not set one.
pub fn parse(text: &str, home: &str, default_user: &str) -> Vec<SshConfigEntry> {
    let mut aliases: Vec<(Vec<String>, Block)> = Vec::new();
    let mut defaults = Block::default();
    let mut current: Option<usize> = None;
    let mut in_wildcard = false;

    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        // `Key value`, `Key=value` and `Key = value` are all accepted.
        let (key, value) = match line.split_once(|c: char| c.is_whitespace() || c == '=') {
            Some((k, v)) => (
                k.to_lowercase(),
                v.trim_start_matches(['=', ' ', '\t']).trim(),
            ),
            None => continue,
        };
        let value = value.trim_matches('"');

        match key.as_str() {
            "host" => {
                let patterns: Vec<String> = value.split_whitespace().map(String::from).collect();
                let concrete: Vec<String> = patterns
                    .iter()
                    .filter(|p| !p.contains(['*', '?', '!']))
                    .cloned()
                    .collect();
                in_wildcard = patterns.iter().any(|p| p == "*");
                if concrete.is_empty() {
                    current = None;
                } else {
                    aliases.push((concrete, Block::default()));
                    current = Some(aliases.len() - 1);
                }
            }
            // `Match` blocks are conditional; their settings cannot be
            // applied without evaluating them, so they are skipped.
            "match" => {
                current = None;
                in_wildcard = false;
            }
            _ => {
                if let Some(i) = current {
                    aliases[i].1.set(&key, value);
                } else if in_wildcard {
                    defaults.set(&key, value);
                }
            }
        }
    }

    let mut out = Vec::new();
    for (names, block) in aliases {
        for alias in names {
            let hostname = block
                .hostname
                .clone()
                .unwrap_or_else(|| alias.clone())
                .replace("%h", &alias);
            out.push(SshConfigEntry {
                alias: alias.clone(),
                hostname,
                user: block
                    .user
                    .clone()
                    .or_else(|| defaults.user.clone())
                    .unwrap_or_else(|| default_user.to_string()),
                port: block.port.or(defaults.port).unwrap_or(22),
                identity_file: block
                    .identity_file
                    .clone()
                    .or_else(|| defaults.identity_file.clone())
                    .map(|p| expand_home(&p, home)),
                proxy_jump: block
                    .proxy_jump
                    .clone()
                    .or_else(|| defaults.proxy_jump.clone()),
            });
        }
    }
    out
}

/// Reads and parses the current user's `~/.ssh/config`.
pub fn read_user_config() -> Result<Vec<SshConfigEntry>, String> {
    let home = crate::local::home_dir().ok_or_else(|| "无法确定主目录".to_string())?;
    let path = std::path::Path::new(&home).join(".ssh/config");
    let text =
        std::fs::read_to_string(&path).map_err(|e| format!("无法读取 {}：{e}", path.display()))?;
    // USERNAME is the Windows spelling.
    let user = std::env::var("USER")
        .or_else(|_| std::env::var("USERNAME"))
        .unwrap_or_default();
    Ok(parse(&text, &home, &user))
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"
# comment
Host *
  User fallback
  IdentityFile ~/.ssh/id_ed25519

Host prod web1
  HostName 10.0.0.5
  Port 2200
  ProxyJump bastion

Host bastion
  HostName=bastion.example.com
  User = ops

Host *.internal
  User ignored

Match host foo
  User nope
"#;

    #[test]
    fn parses_aliases_and_applies_wildcard_defaults() {
        let entries = parse(SAMPLE, "/home/me", "me");
        let names: Vec<_> = entries.iter().map(|e| e.alias.as_str()).collect();
        assert_eq!(names, ["prod", "web1", "bastion"]);

        let prod = &entries[0];
        assert_eq!(prod.hostname, "10.0.0.5");
        assert_eq!(prod.port, 2200);
        assert_eq!(prod.user, "fallback");
        assert_eq!(
            prod.identity_file.as_deref(),
            Some("/home/me/.ssh/id_ed25519")
        );
        assert_eq!(prod.proxy_jump.as_deref(), Some("bastion"));

        let bastion = &entries[2];
        assert_eq!(bastion.hostname, "bastion.example.com");
        assert_eq!(bastion.user, "ops");
        assert_eq!(bastion.port, 22);
    }

    #[test]
    fn hostname_defaults_to_the_alias() {
        let entries = parse("Host box\n  User u\n", "/h", "me");
        assert_eq!(entries[0].hostname, "box");
        assert_eq!(entries[0].identity_file, None);
    }
}
