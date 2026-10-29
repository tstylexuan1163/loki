//! Telnet against a real socket.
//!
//! Start the stand-in server with `python3 /tmp/telnet-test.py`, then:
//!   cargo test --test telnet_live -- --ignored --nocapture

use loki_lib::direct::DirectManager;
use loki_lib::ssh::TermEvent;

#[tokio::test]
#[ignore]
async fn telnet_negotiates_and_echoes() {
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<TermEvent>();

    let conn = DirectManager::connect_telnet("127.0.0.1", 2323, 120, 32)
        .await
        .expect("connect");

    let mut mgr = DirectManager::new();
    mgr.register_telnet("t1".into(), "127.0.0.1".into(), 2323, conn, tx);

    // The server greets with a banner; telnet's WILL ECHO must not appear in it.
    let mut seen = String::new();
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(3);
    while tokio::time::Instant::now() < deadline {
        match tokio::time::timeout(std::time::Duration::from_millis(300), rx.recv()).await {
            Ok(Some(TermEvent::Data { data, .. })) => {
                use base64::Engine;
                let bytes = base64::engine::general_purpose::STANDARD
                    .decode(&data)
                    .unwrap_or_default();
                seen.push_str(&String::from_utf8_lossy(&bytes));
                if seen.contains("ready") {
                    break;
                }
            }
            Ok(Some(_)) | Ok(None) => break,
            Err(_) => continue,
        }
    }

    println!("banner: {seen:?}");
    assert!(
        seen.contains("loki-telnet-test ready"),
        "server banner should arrive: {seen:?}"
    );
    assert!(
        !seen.contains('\u{fffd}') && !seen.contains('\u{fb}'),
        "telnet negotiation bytes must be stripped: {seen:?}"
    );

    // Writing reaches the server, which echoes it back.
    let session = mgr.get("t1").expect("session");
    session.write(b"hello\r\n").await.expect("write");

    let echo_deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(3);
    let mut echoed = String::new();
    while tokio::time::Instant::now() < echo_deadline {
        match tokio::time::timeout(std::time::Duration::from_millis(300), rx.recv()).await {
            Ok(Some(TermEvent::Data { data, .. })) => {
                use base64::Engine;
                let bytes = base64::engine::general_purpose::STANDARD
                    .decode(&data)
                    .unwrap_or_default();
                echoed.push_str(&String::from_utf8_lossy(&bytes));
                if echoed.contains("you said: hello") {
                    break;
                }
            }
            Ok(Some(_)) | Ok(None) => break,
            Err(_) => continue,
        }
    }

    println!("echo: {echoed:?}");
    assert!(
        echoed.contains("you said: hello"),
        "the server should echo what we sent: {echoed:?}"
    );
}
