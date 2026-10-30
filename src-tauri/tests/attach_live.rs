//! Live tests for attachments against the throwaway sshd from docs/STATUS.md.
//!
//! Ignored by default:
//! `LOKI_TEST_KEY=/tmp/loki-sshd/client_key LOKI_TEST_FINGERPRINT=SHA256:... \
//!  cargo test --test attach_live -- --ignored`

use loki_lib::ai::{attach, staging};
use loki_lib::ssh::{self, ConnectOutcome, Credential, Hop, KnownKey, TermEvent};
use std::sync::Arc;

fn key() -> String {
    let path =
        std::env::var("LOKI_TEST_KEY").unwrap_or_else(|_| "/tmp/loki-sshd/client_key".into());
    std::fs::read_to_string(path).expect("test key")
}

fn hop() -> Hop {
    Hop {
        address: "127.0.0.1".into(),
        port: 2222,
        username: std::env::var("USER").unwrap_or_else(|_| "xuan".into()),
        credentials: vec![Credential::Key {
            pem: key(),
            passphrase: None,
        }],
        known: vec![KnownKey {
            key_type: "ssh-ed25519".into(),
            fingerprint: std::env::var("LOKI_TEST_FINGERPRINT").expect("fingerprint"),
        }],
    }
}

async fn live() -> Arc<ssh::LiveSession> {
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel::<TermEvent>();
    match ssh::connect_chain(None, &hop(), "t", "attach-test", None, 80, 24, tx)
        .await
        .expect("connect")
        .outcome
    {
        ConnectOutcome::Live(s) => s,
        _ => panic!("expected a live session"),
    }
}

fn perms_cmd(path: &str) -> String {
    // macOS stat; the test host is macOS.
    format!("stat -f '%Lp' {}", attach::shell_escape(path))
}

#[tokio::test]
#[ignore]
async fn script_is_uploaded_verified_and_runs() {
    let session = live().await;
    let sftp = loki_lib::sftp::open_sftp(session.handle())
        .await
        .expect("sftp");
    let home = loki_lib::sftp::home_dir(&sftp).await.expect("home");
    let batch = attach::staging_name(chrono::Local::now().timestamp(), 0x1234);
    let dir = staging::prepare_batch(&sftp, &home, &batch)
        .await
        .expect("prepare");

    let script = b"#!/bin/sh\necho attach-ok-42\n";
    let (remote, verified) =
        staging::upload_file(&session, &sftp, &dir, "run.sh", script, |_, _| {})
            .await
            .expect("upload");
    assert!(verified, "the remote hash did not match");
    assert!(remote.starts_with(&dir), "{remote} not in {dir}");

    let mode = ssh::exec_with_timeout(
        &session,
        &perms_cmd(&remote),
        std::time::Duration::from_secs(10),
        1024,
    )
    .await
    .expect("stat");
    assert_eq!(mode.trim(), "600", "staged file is not private: {mode}");

    let ran = ssh::exec_with_timeout(
        &session,
        &format!("sh {}", attach::shell_escape(&remote)),
        std::time::Duration::from_secs(15),
        4096,
    )
    .await
    .expect("run");
    assert!(ran.contains("attach-ok-42"), "{ran}");

    // No `.part` left behind.
    let listing = ssh::exec_with_timeout(
        &session,
        &format!("ls -a {}", attach::shell_escape(&dir)),
        std::time::Duration::from_secs(10),
        4096,
    )
    .await
    .expect("ls");
    assert!(!listing.contains(".part"), "{listing}");

    let removed = staging::remove_tree(&sftp, &dir).await;
    assert!(removed >= 1, "cleanup removed nothing");
}

#[tokio::test]
#[ignore]
async fn a_catted_private_key_never_reaches_the_model() {
    let session = live().await;
    let sftp = loki_lib::sftp::open_sftp(session.handle())
        .await
        .expect("sftp");
    let home = loki_lib::sftp::home_dir(&sftp).await.expect("home");
    let batch = attach::staging_name(chrono::Local::now().timestamp(), 0xbeef);
    let dir = staging::prepare_batch(&sftp, &home, &batch)
        .await
        .expect("prepare");

    let body = "SUPERSECRETKEYBODY31337";
    let pem =
        format!("-----BEGIN OPENSSH PRIVATE KEY-----\n{body}\n-----END OPENSSH PRIVATE KEY-----\n");
    assert!(attach::is_secret("server.key", pem.as_bytes()));

    let (remote, verified) = staging::upload_file(
        &session,
        &sftp,
        &dir,
        "server.key",
        pem.as_bytes(),
        |_, _| {},
    )
    .await
    .expect("upload");
    assert!(verified);

    // The model does the worst thing it can: `cat` the key through a tool.
    let raw = ssh::exec_with_timeout(
        &session,
        &format!("cat {}", attach::shell_escape(&remote)),
        std::time::Duration::from_secs(10),
        8192,
    )
    .await
    .expect("cat");
    assert!(raw.contains(body), "the server really does hold the key");

    // What the engine passes on is the redacted form (this is the same call
    // `run_turn` makes on every tool result).
    let seen = attach::redact_private_keys(&raw);
    assert!(
        !seen.contains(body),
        "the key body reached the model: {seen}"
    );

    staging::remove_tree(&sftp, &dir).await;
}

#[tokio::test]
#[ignore]
async fn a_large_file_goes_up_in_chunks_with_progress() {
    let session = live().await;
    let sftp = loki_lib::sftp::open_sftp(session.handle())
        .await
        .expect("sftp");
    let home = loki_lib::sftp::home_dir(&sftp).await.expect("home");
    let batch = attach::staging_name(chrono::Local::now().timestamp(), 0x5678);
    let dir = staging::prepare_batch(&sftp, &home, &batch)
        .await
        .expect("prepare");

    // 3 MiB of non-repeating bytes: twelve chunks, and a hash that would catch
    // a chunk written twice or out of order.
    let data: Vec<u8> = (0u32..3 * 1024 * 1024)
        .map(|i| (i.wrapping_mul(2_654_435_761) >> 24) as u8)
        .collect();
    let mut reports: Vec<(u64, u64)> = Vec::new();
    let (_, verified) = staging::upload_file(&session, &sftp, &dir, "big.bin", &data, |s, t| {
        reports.push((s, t))
    })
    .await
    .expect("upload");

    assert!(verified, "the remote hash did not match");
    let total = data.len() as u64;
    assert_eq!(reports.last(), Some(&(total, total)), "{reports:?}");
    assert!(reports.windows(2).all(|w| w[0].0 <= w[1].0), "{reports:?}");

    staging::remove_tree(&sftp, &dir).await;
}
