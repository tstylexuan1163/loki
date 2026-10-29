//! Recording through a real local shell.
//!
//! Starts a local PTY, attaches a recorder the same way `session_record_start`
//! does, runs `seq 1 50000`, and checks the file. This is the acceptance case for
//! long output that overflows the scrollback.

use loki_lib::direct::DirectManager;
use loki_lib::recorder::Recorder;
use loki_lib::ssh::TermEvent;
use std::time::{Duration, Instant};

#[tokio::test]
async fn local_session_records_fifty_thousand_lines() {
    let dir = std::env::temp_dir().join(format!(
        "loki-rec-live-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("session.txt");

    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<TermEvent>();
    // Drain events continuously: reading one per poll would throttle the PTY and
    // keep the tail from ever arriving.
    tokio::spawn(async move { while rx.recv().await.is_some() {} });
    let mut mgr = DirectManager::new();
    mgr.open_local("s1".into(), 120, 32, tx)
        .expect("open local shell");

    // Attach the recorder exactly where the command handler puts it.
    let slot = mgr.recorder_slot("s1").expect("recorder slot");
    *slot.lock().unwrap() =
        Some(Recorder::start(&path, "# loki 测试录制\n\n", true).expect("start"));

    let session = mgr.get("s1").expect("session");
    session
        .write(b"seq 1 50000\n")
        .await
        .expect("write command");

    // Drain events so the PTY never blocks on a full channel, and stop once the
    // file ends with our marker line.
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut done = false;
    while Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(200)).await;
        if let Ok(text) = std::fs::read_to_string(&path) {
            if text.lines().any(|l| l.trim() == "50000") {
                done = true;
                break;
            }
        }
    }
    assert!(done, "recorder never saw the end of `seq 1 50000`");

    // Stop the way the UI does, then check the file.
    let mut rec = slot.lock().unwrap().take().expect("still recording");
    let bytes = rec.finish("# 结束\n");
    let text = std::fs::read_to_string(&path).expect("read recording");

    assert!(bytes > 0);
    assert!(
        !text.as_bytes().contains(&0x1b),
        "escape bytes leaked into the file"
    );
    assert!(
        text.ends_with("# 结束\n"),
        "{:?}",
        &text[text.len().saturating_sub(40)..]
    );

    // Every number from 1 to 50000 must be present as its own line; prompts and
    // the echoed command may add extra lines, which is fine.
    let nums: std::collections::HashSet<u64> = text
        .lines()
        .filter_map(|l| l.trim().parse::<u64>().ok())
        .collect();
    for n in [1u64, 2, 25_000, 49_999, 50_000] {
        assert!(nums.contains(&n), "missing line {n} in the recording");
    }
    assert!(
        nums.len() >= 50_000,
        "only {} numeric lines recorded",
        nums.len()
    );

    std::fs::remove_dir_all(&dir).ok();
}
