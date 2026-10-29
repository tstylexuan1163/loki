//! A saved conversation must be unreadable on disk.
//!
//! Writes one through the real vault into a real SQLite file and greps both the
//! database and its WAL for the plaintext, which is what "打开 loki.db 搜不到"
//! means in practice.

use loki_lib::crypto::KdfParams;
use loki_lib::store::Store;
use loki_lib::vault::Vault;

fn contains(haystack: &[u8], needle: &str) -> bool {
    haystack
        .windows(needle.len())
        .any(|w| w == needle.as_bytes())
}

#[test]
fn conversation_plaintext_never_reaches_the_file() {
    let dir = std::env::temp_dir().join(format!(
        "loki-conv-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let db = dir.join("loki.db");

    let mut vault = Vault::new(Store::open(&db).unwrap()).unwrap();
    vault
        .create("conversation-test-pw", KdfParams::default())
        .unwrap();

    let secret = "TOP-SECRET-SERVER-OUTPUT-9f3a";
    let payload = serde_json::json!({
        "id": "c1",
        "title": "带秘密的对话",
        "hostId": "h1",
        "hostLabel": "prod",
        "turns": [{ "role": "user", "parts": [{ "kind": "text", "text": secret }] }],
    });

    let row = vault
        .encrypt_item("c1", "ai_conversation", &payload)
        .expect("encrypt");
    vault.store().put_item(&row).expect("store");

    // The round trip works while unlocked.
    let back = vault
        .decrypt_item(&vault.store().get_item("c1").unwrap().unwrap())
        .unwrap();
    assert_eq!(back.payload["turns"][0]["parts"][0]["text"], secret);

    // Drop the vault so nothing is held open, then look at the bytes.
    drop(vault);

    let mut checked = 0;
    for name in ["loki.db", "loki.db-wal"] {
        let path = dir.join(name);
        if let Ok(bytes) = std::fs::read(&path) {
            checked += 1;
            assert!(
                !contains(&bytes, secret),
                "{name} contains the plaintext conversation"
            );
            assert!(
                !contains(&bytes, "带秘密的对话"),
                "{name} contains the title"
            );
        }
    }
    assert!(checked > 0, "no database file was written");

    std::fs::remove_dir_all(&dir).ok();
}
