//! Attachments: putting a local file on the server, and keeping private keys
//! away from the model.
//!
//! Everything here is pure: sizes, sniffing, the staging path name, shell
//! quoting, the text the model is given, and the redaction applied to tool
//! output. The IO lives in `commands.rs`, which is where it can be tested
//! against a real server.

use std::path::Path;

/// Largest single file accepted.
pub const MAX_FILE: usize = 20 * 1024 * 1024;
/// Most files in one attach action.
pub const MAX_BATCH: usize = 10;
/// A text file up to this size may be shown to the model verbatim.
pub const SHARE_LIMIT: usize = 64 * 1024;

/// Folders the staging area lives in, relative to the account's home.
pub const STAGING_ROOT: &str = ".loki";
pub const STAGING_SUBDIR: &str = "uploads";

const REDACTED: &str = "[私钥内容已隐藏]";

/// `text` when the bytes are valid UTF-8 without NUL, otherwise `binary`.
pub fn sniff_kind(bytes: &[u8]) -> &'static str {
    if bytes.contains(&0) {
        return "binary";
    }
    match std::str::from_utf8(bytes) {
        Ok(_) => "text",
        Err(_) => "binary",
    }
}

/// Hex SHA-256, the form `sha256sum` prints.
pub fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Whether the file must never be handed to the model.
///
/// Content wins over the name: a private key saved as `notes.txt` is still a
/// private key. The container formats carry no readable marker, so their
/// extension is the only signal available.
pub fn is_secret(name: &str, bytes: &[u8]) -> bool {
    let lower = name.to_ascii_lowercase();
    let ext = Path::new(&lower)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("");
    if matches!(ext, "pfx" | "p12" | "jks") {
        return true;
    }
    // Lossy is fine: the marker is ASCII and survives replacement characters.
    String::from_utf8_lossy(bytes).contains("PRIVATE KEY-----")
}

/// Replaces every PEM private-key body with a placeholder.
///
/// Covers RSA, EC, OPENSSH, ENCRYPTED and untyped blocks, several per text, and
/// a block whose END line never arrived (the rest of the text is hidden). A
/// `-----BEGIN` that is not followed by `PRIVATE KEY-----` on the same line —
/// a certificate, a public key — is left alone.
pub fn redact_private_keys(text: &str) -> String {
    const BEGIN: &str = "-----BEGIN";
    const END: &str = "-----END";

    let mut out = String::with_capacity(text.len());
    let mut rest = text;

    loop {
        let Some(start) = rest.find(BEGIN) else {
            out.push_str(rest);
            break;
        };
        let line_end = rest[start..]
            .find('\n')
            .map(|i| start + i)
            .unwrap_or(rest.len());
        let header = &rest[start..line_end];

        if !header.contains("PRIVATE KEY-----") {
            // A certificate or public key: keep the line as it is.
            out.push_str(&rest[..line_end]);
            rest = &rest[line_end..];
            continue;
        }

        out.push_str(&rest[..start]);
        out.push_str(REDACTED);

        let after = &rest[line_end..];
        match after.find(END) {
            Some(offset) => {
                let end_at = line_end + offset;
                // Swallow the END line's newline so the placeholder does not
                // leave a blank line behind.
                let next = rest[end_at..]
                    .find('\n')
                    .map(|i| end_at + i + 1)
                    .unwrap_or(rest.len());
                rest = &rest[next..];
            }
            None => {
                // Unterminated block: nothing after it can be trusted.
                rest = "";
            }
        }
    }

    out
}

/// Staging directory name: `<YYYYMMDD-HHMMSS>-<4 hex>`.
pub fn staging_name(secs: i64, rand: u16) -> String {
    let stamp = chrono::DateTime::from_timestamp(secs, 0)
        .map(|dt| {
            use chrono::TimeZone;
            chrono::Local
                .from_utc_datetime(&dt.naive_utc())
                .format("%Y%m%d-%H%M%S")
                .to_string()
        })
        .unwrap_or_else(|| chrono::Local::now().format("%Y%m%d-%H%M%S").to_string());
    format!("{stamp}-{rand:04x}")
}

/// POSIX single-quoted string, safe to drop into a shell command.
pub fn shell_escape(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// `2.1 KB` — sizes as a person reads them.
pub fn human_size(bytes: u64) -> String {
    const KB: f64 = 1024.0;
    let b = bytes as f64;
    if b < KB {
        format!("{bytes} B")
    } else if b < KB * KB {
        format!("{:.1} KB", b / KB)
    } else {
        format!("{:.1} MB", b / (KB * KB))
    }
}

/// One attachment as the model is told about it.
pub struct AttachDesc<'a> {
    pub name: &'a str,
    pub size: u64,
    pub sha256: &'a str,
    pub secret: bool,
    pub remote_path: &'a str,
    /// Present only for shareable files the user let the model read.
    pub content: Option<&'a str>,
}

/// The block appended to the user's message.
///
/// A secret file contributes its path and nothing else — even if `content` was
/// somehow set, it is ignored here. That is the last line of defence behind
/// `redact_private_keys`.
pub fn describe(descs: &[AttachDesc]) -> String {
    if descs.is_empty() {
        return String::new();
    }
    let mut out = String::from("附件（已上传到服务器）：\n");
    for (i, d) in descs.iter().enumerate() {
        let short: String = d.sha256.chars().take(12).collect();
        if d.secret {
            out.push_str(&format!(
                "#{} {}，{}，含私钥：内容不提供，只能用路径操作（{}）\n",
                i + 1,
                d.name,
                human_size(d.size),
                d.remote_path
            ));
        } else {
            out.push_str(&format!(
                "#{} {}，{}，sha256 {}…，路径 {}\n",
                i + 1,
                d.name,
                human_size(d.size),
                short,
                d.remote_path
            ));
            if let Some(content) = d.content {
                out.push_str("内容：\n```\n");
                out.push_str(content);
                if !content.ends_with('\n') {
                    out.push('\n');
                }
                out.push_str("```\n");
            }
        }
    }
    out.push_str("用这些路径操作。修改服务器上的配置前先备份，改动后先校验再重载。\n");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const RSA_KEY: &str = "-----BEGIN RSA PRIVATE KEY-----\nMIIEowIBAAKCAQEA\nabc123\n-----END RSA PRIVATE KEY-----\n";
    const OPENSSH_KEY: &str =
        "-----BEGIN OPENSSH PRIVATE KEY-----\nb3BlbnNzaC1rZXk\n-----END OPENSSH PRIVATE KEY-----\n";

    #[test]
    fn kind_is_binary_on_nul_or_invalid_utf8() {
        assert_eq!(sniff_kind(b"hello\n"), "text");
        assert_eq!(sniff_kind(b"he\0llo"), "binary");
        assert_eq!(sniff_kind(&[0xff, 0xfe, 0x00]), "binary");
    }

    #[test]
    fn secret_is_detected_from_content_or_container_extension() {
        assert!(is_secret("server.pem", RSA_KEY.as_bytes()));
        // The name says nothing, the content does.
        assert!(is_secret("notes.txt", OPENSSH_KEY.as_bytes()));
        assert!(is_secret("bundle.p12", &[0x30, 0x82, 0x00]));
        assert!(is_secret("store.jks", &[0xfe, 0xed]));
        assert!(!is_secret("script.sh", b"#!/bin/sh\necho hi\n"));
        assert!(!is_secret(
            "site.crt",
            b"-----BEGIN CERTIFICATE-----\nMIIB\n-----END CERTIFICATE-----\n"
        ));
    }

    #[test]
    fn body_of_a_private_key_is_hidden() {
        let out = redact_private_keys(RSA_KEY);
        assert!(!out.contains("MIIEowIBAAKCAQEA"), "{out}");
        assert!(out.contains(REDACTED));
    }

    #[test]
    fn every_key_type_is_covered() {
        for header in [
            "RSA PRIVATE KEY",
            "EC PRIVATE KEY",
            "OPENSSH PRIVATE KEY",
            "ENCRYPTED PRIVATE KEY",
            "PRIVATE KEY",
            "DSA PRIVATE KEY",
        ] {
            let text = format!("-----BEGIN {header}-----\nSECRETBODY\n-----END {header}-----\n");
            let out = redact_private_keys(&text);
            assert!(!out.contains("SECRETBODY"), "{header} leaked: {out}");
        }
    }

    #[test]
    fn several_blocks_and_crlf_are_handled() {
        let text = format!("a\r\n{RSA_KEY}b\r\n{OPENSSH_KEY}c");
        let out = redact_private_keys(&text);
        assert!(!out.contains("MIIEowIBAAKCAQEA"));
        assert!(!out.contains("b3BlbnNzaC1rZXk"));
        assert!(out.contains('a') && out.contains('b') && out.contains('c'));
    }

    #[test]
    fn an_unterminated_block_hides_everything_after_it() {
        let text = "before\n-----BEGIN RSA PRIVATE KEY-----\nLEAKEDBODY\nand more";
        let out = redact_private_keys(text);
        assert!(!out.contains("LEAKEDBODY"));
        assert!(!out.contains("and more"));
        assert!(out.starts_with("before"));
    }

    #[test]
    fn certificates_and_public_keys_are_not_touched() {
        let text = "-----BEGIN CERTIFICATE-----\nMIIB\n-----END CERTIFICATE-----\n-----BEGIN PUBLIC KEY-----\nAAA\n-----END PUBLIC KEY-----\n";
        assert_eq!(redact_private_keys(text), text);
    }

    #[test]
    fn staging_name_has_the_expected_shape() {
        let name = staging_name(1_782_000_000, 0x00ab);
        // 1782000000 is 2026-06-20 in UTC; local shifting may move the day but
        // never the shape.
        assert_eq!(name.len(), 15 + 5, "{name}");
        assert_eq!(name.rsplit('-').next().unwrap(), "00ab");
        assert!(name.starts_with("20"));
    }

    #[test]
    fn shell_escape_quotes_embedded_quotes() {
        assert_eq!(shell_escape("/tmp/a b"), "'/tmp/a b'");
        assert_eq!(shell_escape("it's"), "'it'\\''s'");
    }

    #[test]
    fn description_never_carries_a_secret_body() {
        let desc = AttachDesc {
            name: "server.key",
            size: 1700,
            sha256: "3fa9c1e2b7d0aa",
            secret: true,
            remote_path: "~/.loki/uploads/20260101-000000-00ab/server.key",
            // Even if a caller sets this by mistake, describe must ignore it.
            content: Some(RSA_KEY),
        };
        let text = describe(&[desc]);
        assert!(!text.contains("MIIEowIBAAKCAQEA"), "{text}");
        assert!(text.contains("含私钥"));
        assert!(text.contains("内容不提供"));
    }

    #[test]
    fn description_includes_shareable_content_and_size() {
        let desc = AttachDesc {
            name: "deploy.sh",
            size: 2048,
            sha256: "abcdef1234567890",
            secret: false,
            remote_path: "~/.loki/uploads/x/deploy.sh",
            content: Some("#!/bin/sh\necho ok\n"),
        };
        let text = describe(&[desc]);
        assert!(text.contains("2.0 KB"), "{text}");
        assert!(text.contains("abcdef123456"), "{text}");
        assert!(text.contains("echo ok"), "{text}");
    }

    #[test]
    fn description_of_nothing_is_empty() {
        assert!(describe(&[]).is_empty());
    }
}
