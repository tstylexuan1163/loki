//! Live tests for the connection chain: known hosts, a changed key, the
//! password prompt, a jump host, resource stats and SFTP transfers.
//!
//! Ignored by default. They need the throwaway sshd described in docs/STATUS.md:
//! `LOKI_TEST_KEY=<client key> LOKI_TEST_FINGERPRINT=SHA256:... \
//!  cargo test --test chain_live -- --ignored`

use loki_lib::ssh::{self, ConnectOutcome, Credential, Hop, KnownKey, TermEvent};
use std::sync::atomic::AtomicBool;

fn key() -> String {
    let path = std::env::var("LOKI_TEST_KEY").unwrap_or_else(|_| "/tmp/loki-sshd/client_key".into());
    std::fs::read_to_string(path).expect("test key")
}

fn fingerprint() -> String {
    std::env::var("LOKI_TEST_FINGERPRINT").expect("set LOKI_TEST_FINGERPRINT")
}

fn hop(credentials: Vec<Credential>, known: Vec<KnownKey>) -> Hop {
    Hop {
        address: "127.0.0.1".into(),
        port: 2222,
        username: std::env::var("USER").unwrap_or_else(|_| "xuan".into()),
        credentials,
        known,
    }
}

fn key_cred() -> Credential {
    Credential::Key {
        pem: key(),
        passphrase: None,
    }
}

fn trusted() -> Vec<KnownKey> {
    vec![KnownKey {
        key_type: "ssh-ed25519".into(),
        fingerprint: fingerprint(),
    }]
}

async fn live(jump: Option<&Hop>, target: &Hop) -> std::sync::Arc<ssh::LiveSession> {
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel::<TermEvent>();
    match ssh::connect_chain(jump, target, "t", "test", None, 80, 24, tx)
        .await
        .expect("connect")
        .outcome
    {
        ConnectOutcome::Live(s) => s,
        _ => panic!("expected a live session"),
    }
}

#[tokio::test]
#[ignore]
async fn first_contact_asks_then_accepting_records_the_key() {
    let target = hop(vec![key_cred()], vec![]);
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel::<TermEvent>();
    let first = ssh::connect_chain(None, &target, "t", "test", None, 80, 24, tx.clone())
        .await
        .unwrap();
    let fp = match first.outcome {
        ConnectOutcome::UnknownHostKey {
            fingerprint,
            previous,
            key_type,
            ..
        } => {
            assert!(previous.is_none());
            assert_eq!(key_type, "ssh-ed25519");
            fingerprint
        }
        _ => panic!("an unknown host must be asked about"),
    };
    assert_eq!(fp, fingerprint());

    let second = ssh::connect_chain(None, &target, "t", "test", Some(&fp), 80, 24, tx)
        .await
        .unwrap();
    assert!(matches!(second.outcome, ConnectOutcome::Live(_)));
    // The accepted key comes back so the caller can persist it.
    assert_eq!(second.accepted.len(), 1);
    assert_eq!(second.accepted[0].fingerprint, fp);
}

#[tokio::test]
#[ignore]
async fn a_known_key_connects_without_asking() {
    let session = live(None, &hop(vec![key_cred()], trusted())).await;
    assert_eq!(session.info.status, "connected");
}

#[tokio::test]
#[ignore]
async fn a_changed_key_reports_the_previous_fingerprint() {
    let stale = vec![KnownKey {
        key_type: "ssh-ed25519".into(),
        fingerprint: "SHA256:AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA".into(),
    }];
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel::<TermEvent>();
    let res = ssh::connect_chain(None, &hop(vec![key_cred()], stale), "t", "test", None, 80, 24, tx)
        .await
        .unwrap();
    match res.outcome {
        ConnectOutcome::UnknownHostKey { previous, .. } => {
            assert_eq!(
                previous.as_deref(),
                Some("SHA256:AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA")
            );
        }
        _ => panic!("a changed key must not connect"),
    }
}

#[tokio::test]
#[ignore]
async fn no_working_credential_and_no_password_asks_for_one() {
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel::<TermEvent>();
    let res = ssh::connect_chain(None, &hop(vec![], trusted()), "t", "test", None, 80, 24, tx)
        .await
        .unwrap();
    assert!(matches!(res.outcome, ConnectOutcome::NeedPassword { .. }));
}

#[tokio::test]
#[ignore]
async fn tunnels_through_a_jump_host() {
    // The same server acts as its own bastion: the second handshake runs over
    // a direct-tcpip channel of the first.
    let jump = hop(vec![key_cred()], trusted());
    let target = hop(vec![key_cred()], trusted());
    let session = live(Some(&jump), &target).await;
    let out = ssh::exec_command(&session, "echo via-jump-$((20+22))").await.unwrap();
    assert!(out.contains("via-jump-42"), "{out}");
}

#[tokio::test]
#[ignore]
async fn stats_come_back_populated() {
    let session = live(None, &hop(vec![key_cred()], trusted())).await;
    let s = loki_lib::stats::sample(&session).await.unwrap();
    assert!(s.cpus.unwrap_or(0) > 0, "{s:?}");
    assert!(s.mem_total.unwrap_or(0) > 0, "{s:?}");
    assert!(s.disk_total.unwrap_or(0) > 0, "{s:?}");
    println!("{s:?}");
}

#[tokio::test]
#[ignore]
async fn uploads_and_downloads_a_directory() {
    let session = live(None, &hop(vec![key_cred()], trusted())).await;
    let sftp = loki_lib::sftp::open_sftp(session.handle()).await.unwrap();

    let base = std::env::temp_dir().join(format!("loki-xfer-{}", uuid::Uuid::new_v4()));
    let src = base.join("src");
    std::fs::create_dir_all(src.join("nested")).unwrap();
    std::fs::write(src.join("a.txt"), b"hello").unwrap();
    std::fs::write(src.join("nested/b.bin"), vec![7u8; 600_000]).unwrap();

    let remote_dir = format!("{}/remote", base.display());
    std::fs::create_dir_all(&remote_dir).unwrap();
    let cancel = AtomicBool::new(false);
    let mut last = (0, 0);
    loki_lib::transfer::upload(&sftp, &src, &remote_dir, &cancel, |b, t| last = (b, t))
        .await
        .unwrap();
    assert_eq!(last, (600_005, 600_005));
    assert_eq!(std::fs::read(format!("{remote_dir}/src/a.txt")).unwrap(), b"hello");

    let back = base.join("back");
    loki_lib::transfer::download(&sftp, &format!("{remote_dir}/src"), &back, &cancel, |_, _| {})
        .await
        .unwrap();
    assert_eq!(std::fs::read(back.join("nested/b.bin")).unwrap().len(), 600_000);
    assert!(!back.join("a.txt.part").exists(), "no .part left behind");

    loki_lib::transfer::remove_recursive(&sftp, &format!("{remote_dir}/src"))
        .await
        .unwrap();
    assert!(!std::path::Path::new(&format!("{remote_dir}/src")).exists());
    std::fs::remove_dir_all(&base).unwrap();
}

#[tokio::test]
#[ignore]
async fn authenticates_through_the_agent() {
    // Opt-in: needs SSH_AUTH_SOCK pointing at an agent that holds the test
    // key, which the everyday agent should not.
    if std::env::var("LOKI_TEST_AGENT").is_err() {
        eprintln!("LOKI_TEST_AGENT not set; skipping");
        return;
    }
    let session = live(None, &hop(vec![Credential::Agent], trusted())).await;
    let out = ssh::exec_command(&session, "echo agent-ok").await.unwrap();
    assert!(out.contains("agent-ok"), "{out}");
}
