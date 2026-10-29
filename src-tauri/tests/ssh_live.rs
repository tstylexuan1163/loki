//! Live smoke test against a throwaway sshd on 127.0.0.1:2222.
//!
//! Ignored by default; it needs the local server described in docs/STATUS.md.
//! Run with: cargo test --test ssh_live -- --ignored --nocapture

use loki_lib::commands::HostView;
use loki_lib::ssh::{self, ConnectOutcome, Credential, TermEvent};

/// `LOKI_TEST_KEY` overrides where the client key for the throwaway sshd lives.
fn test_key_path() -> String {
    std::env::var("LOKI_TEST_KEY").unwrap_or_else(|_| "/tmp/loki-sshd/client_key".into())
}

fn test_host() -> HostView {
    let mut host = HostView::ssh(
        "t",
        "local-test",
        "127.0.0.1",
        2222,
        &std::env::var("USER").unwrap_or_else(|_| "xuan".into()),
    );
    host.has_key = true;
    host
}

#[tokio::test]
#[ignore]
async fn first_contact_reports_host_key() {
    let key = std::fs::read_to_string(test_key_path()).expect("test key");
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel::<TermEvent>();

    let outcome = ssh::connect(
        &test_host(),
        &Credential::Key {
            pem: key,
            passphrase: None,
        },
        None,
        120,
        32,
        tx,
    )
    .await
    .expect("connect call should not error on an unknown host");

    match outcome {
        ConnectOutcome::UnknownHostKey {
            fingerprint,
            previous,
            ..
        } => {
            assert!(fingerprint.starts_with("SHA256:"), "got {fingerprint}");
            assert!(previous.is_none(), "a first contact has no previous key");
            println!("unknown host key, as expected: {fingerprint}");
        }
        ConnectOutcome::Live(_) => panic!("must not trust an unknown host key"),
        ConnectOutcome::NeedPassword { .. } => panic!("host key check comes before auth"),
    }
}

#[tokio::test]
#[ignore]
async fn connects_and_runs_a_shell_with_a_pinned_key() {
    let key = std::fs::read_to_string(test_key_path()).expect("test key");
    let pin = std::env::var("LOKI_TEST_FINGERPRINT").expect("set LOKI_TEST_FINGERPRINT");

    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<TermEvent>();
    let outcome = ssh::connect(
        &test_host(),
        &Credential::Key {
            pem: key,
            passphrase: None,
        },
        Some(pin),
        120,
        32,
        tx,
    )
    .await
    .expect("connect");

    let session = match outcome {
        ConnectOutcome::Live(s) => s,
        ConnectOutcome::UnknownHostKey { fingerprint, .. } => {
            panic!("pin was rejected; server presented {fingerprint}")
        }
        ConnectOutcome::NeedPassword { .. } => panic!("the key should have been accepted"),
    };
    assert_eq!(session.info.status, "connected");

    // Type a command and wait for its output to come back through the pump.
    session
        .write(b"echo LOKI_MARK_$((6*7))\n")
        .await
        .expect("write");

    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(10);
    let mut seen = String::new();
    while tokio::time::Instant::now() < deadline {
        match tokio::time::timeout(std::time::Duration::from_secs(2), rx.recv()).await {
            Ok(Some(TermEvent::Data { data, .. })) => {
                use base64::Engine;
                let raw = base64::engine::general_purpose::STANDARD
                    .decode(&data)
                    .unwrap();
                seen.push_str(&String::from_utf8_lossy(&raw));
                if seen.contains("LOKI_MARK_42") {
                    break;
                }
            }
            Ok(None) | Err(_) => break,
            _ => {}
        }
    }
    println!("terminal saw:\n{seen}");
    // The marker is only present if the shell actually evaluated it; a bare
    // echo-back of the typed line would read `LOKI_MARK_$((6*7))`.
    assert!(
        seen.contains("LOKI_MARK_42"),
        "shell output never arrived; saw: {seen:?}"
    );
}

/// SFTP against the same throwaway server: list, read, write, rename, delete.
#[tokio::test]
#[ignore]
async fn sftp_lists_and_round_trips_a_file() {
    let key = std::fs::read_to_string(test_key_path()).expect("test key");
    let pin = std::env::var("LOKI_TEST_FINGERPRINT").expect("set LOKI_TEST_FINGERPRINT");
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel::<TermEvent>();

    let outcome = ssh::connect(
        &test_host(),
        &Credential::Key {
            pem: key,
            passphrase: None,
        },
        Some(pin),
        120,
        32,
        tx,
    )
    .await
    .expect("connect");
    let session = match outcome {
        ConnectOutcome::Live(s) => s,
        _ => panic!("expected a live session"),
    };

    let sftp = loki_lib::sftp::open_sftp(session.handle())
        .await
        .expect("sftp");
    let home = loki_lib::sftp::home_dir(&sftp).await.expect("home");
    assert!(home.starts_with('/'), "home should be absolute, got {home}");

    // A freshly created file must appear in the listing with its size.
    let path = loki_lib::sftp::join_path(&home, "loki-sftp-test.txt");
    loki_lib::sftp::write_file(&sftp, &path, b"hello sftp")
        .await
        .expect("write");

    let entries = loki_lib::sftp::list_dir(&sftp, &home).await.expect("list");
    let found = entries
        .iter()
        .find(|e| e.name == "loki-sftp-test.txt")
        .expect("the file we just wrote should be listed");
    assert_eq!(found.kind, "file");
    assert_eq!(found.size, 10);

    assert_eq!(
        loki_lib::sftp::read_file(&sftp, &path).await.expect("read"),
        b"hello sftp"
    );

    // Rename, then delete, and confirm it is gone.
    let moved = loki_lib::sftp::join_path(&home, "loki-sftp-test-renamed.txt");
    loki_lib::sftp::rename(&sftp, &path, &moved)
        .await
        .expect("rename");
    assert!(!loki_lib::sftp::list_dir(&sftp, &home)
        .await
        .expect("list")
        .iter()
        .any(|e| e.name == "loki-sftp-test.txt"));

    loki_lib::sftp::remove(&sftp, &moved, false)
        .await
        .expect("remove");
    assert!(!loki_lib::sftp::list_dir(&sftp, &home)
        .await
        .expect("list")
        .iter()
        .any(|e| e.name == "loki-sftp-test-renamed.txt"));
}

/// Starts a real HTTP server on the remote side of the tunnel and fetches it
/// through the local forward. This is the property that matters: bytes arriving
/// at the remote port, over SSH, in both directions.
#[tokio::test]
#[ignore]
async fn local_forward_carries_traffic() {
    use loki_lib::forward::{self, ForwardSpec};

    // A tiny origin server that answers with a fixed body.
    let origin = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .unwrap();
    let origin_port = origin.local_addr().unwrap().port();
    tokio::spawn(async move {
        loop {
            let Ok((mut socket, _)) = origin.accept().await else {
                break;
            };
            tokio::spawn(async move {
                use tokio::io::{AsyncReadExt, AsyncWriteExt};
                let mut buf = [0u8; 1024];
                let _ = socket.read(&mut buf).await;
                let _ = socket
                    .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 11\r\n\r\nhello-tunnel")
                    .await;
                let _ = socket.shutdown().await;
            });
        }
    });

    let key = std::fs::read_to_string(test_key_path()).expect("test key");
    let pin = std::env::var("LOKI_TEST_FINGERPRINT").expect("set LOKI_TEST_FINGERPRINT");
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel::<TermEvent>();
    let outcome = ssh::connect(
        &test_host(),
        &Credential::Key {
            pem: key,
            passphrase: None,
        },
        Some(pin),
        120,
        32,
        tx,
    )
    .await
    .expect("connect");
    let session = match outcome {
        ConnectOutcome::Live(s) => s,
        _ => panic!("expected a live session"),
    };

    // Ask the OS for a free local port, then forward it to the origin.
    let (view, _stop) = forward::start(
        session,
        ForwardSpec {
            kind: "local".into(),
            local_port: 0,
            remote_host: Some("127.0.0.1".into()),
            remote_port: Some(origin_port),
        },
    )
    .await
    .expect("forward should start");
    assert!(view.local_port > 0);

    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let mut client = tokio::net::TcpStream::connect(("127.0.0.1", view.local_port))
        .await
        .expect("connect to the forwarded port");
    client.write_all(b"GET / HTTP/1.0\r\n\r\n").await.unwrap();

    let mut response = Vec::new();
    let mut buf = [0u8; 512];
    loop {
        match tokio::time::timeout(std::time::Duration::from_secs(5), client.read(&mut buf)).await {
            Ok(Ok(0)) | Err(_) => break,
            Ok(Ok(n)) => response.extend_from_slice(&buf[..n]),
            Ok(Err(_)) => break,
        }
    }
    let text = String::from_utf8_lossy(&response);
    assert!(
        text.contains("hello-tunnel"),
        "traffic did not cross the tunnel; got: {text:?}"
    );
}
