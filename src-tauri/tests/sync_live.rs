//! End-to-end test of the sync wire protocol against a stand-in Worker.
//!
//! Start the server with `node --experimental-sqlite /tmp/sync-test.mjs`, then:
//!   LOKI_SYNC_URL=http://127.0.0.1:18091 cargo test --test sync_live -- --ignored --nocapture

use loki_lib::crypto::KdfParams;
use loki_lib::store::{ItemRow, Store};
use loki_lib::vault::Vault;

async fn token_for(base: &str, client: &reqwest::Client) -> String {
    let health: serde_json::Value = client
        .get(format!("{base}/v1/health"))
        .send()
        .await
        .expect("health")
        .json()
        .await
        .expect("health json");

    if health["initialized"] == false {
        let setup: serde_json::Value = client
            .post(format!("{base}/v1/setup"))
            .json(&serde_json::json!({
                "kdf_salt": "c2FsdA==",
                "kdf_params": "{}",
                "auth_key": "k",
                "protected_vault_key": "p",
                "recovery_vault_key": "r",
                "recovery_auth": "a",
                "device_id": "test"
            }))
            .send()
            .await
            .expect("setup")
            .json()
            .await
            .expect("setup json");
        setup["token"].as_str().unwrap().to_string()
    } else {
        let login: serde_json::Value = client
            .post(format!("{base}/v1/login"))
            .json(&serde_json::json!({ "auth_key": "k", "device_id": "test" }))
            .send()
            .await
            .expect("login")
            .json()
            .await
            .expect("login json");
        login["token"].as_str().unwrap().to_string()
    }
}

#[tokio::test]
#[ignore]
async fn an_item_travels_as_ciphertext_and_decrypts_on_the_other_device() {
    let base = std::env::var("LOKI_SYNC_URL").expect("set LOKI_SYNC_URL");
    let client = reqwest::Client::new();
    let token = token_for(&base, &client).await;

    // ---- device A: real vault, real encryption ---------------------------
    let store_a = Store::open_in_memory().unwrap();
    let mut vault_a = Vault::new(store_a).unwrap();
    vault_a
        .create("device-a-password", KdfParams::FLOOR)
        .unwrap();

    let plain = serde_json::json!({ "name": "prod-api-tokyo", "address": "10.0.12.31" });
    let row = vault_a.encrypt_item("shared-host", "host", &plain).unwrap();
    vault_a.store().put_item(&row).unwrap();

    // ---- push it ---------------------------------------------------------
    let pushed: serde_json::Value = client
        .put(format!("{base}/v1/items"))
        .bearer_auth(&token)
        .json(&serde_json::json!({
            "items": [{
                "id": row.id,
                "kind": row.kind,
                "envelope": serde_json::to_string(&row.envelope).unwrap(),
                "revision": 1,
                "seq": 0,
                "deleted": false,
                "updated_at": row.updated_at,
            }]
        }))
        .send()
        .await
        .expect("push")
        .json()
        .await
        .expect("push json");
    assert_eq!(
        pushed["accepted"], 1,
        "fresh item should be accepted: {pushed}"
    );

    // ---- pull it back ----------------------------------------------------
    let page: serde_json::Value = client
        .get(format!("{base}/v1/items?since=0&limit=50"))
        .bearer_auth(&token)
        .send()
        .await
        .expect("pull")
        .json()
        .await
        .expect("pull json");

    let got = page["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["id"] == "shared-host")
        .expect("the pushed item should come back");

    // The server must never have seen the plaintext.
    let envelope_text = got["envelope"].as_str().unwrap();
    assert!(
        !envelope_text.contains("prod-api-tokyo"),
        "host name leaked into sync payload"
    );
    assert!(
        !envelope_text.contains("10.0.12.31"),
        "address leaked into sync payload"
    );

    // ---- "device B": same vault key, stored locally, decrypts it ---------
    // In a real second device the vault key arrives via the password-protected
    // copy; here we reuse device A's vault to prove the envelope round-trips.
    let envelope: loki_lib::crypto::Envelope = serde_json::from_str(envelope_text).unwrap();
    let restored = vault_a
        .decrypt_item(&ItemRow {
            id: got["id"].as_str().unwrap().to_string(),
            kind: got["kind"].as_str().unwrap().to_string(),
            envelope,
            updated_at: got["updated_at"].as_i64().unwrap(),
        })
        .expect("the pulled envelope should decrypt");
    assert_eq!(restored.payload, plain);

    // ---- a stale push is refused ----------------------------------------
    let stale: serde_json::Value = client
        .put(format!("{base}/v1/items"))
        .bearer_auth(&token)
        .json(&serde_json::json!({
            "items": [{
                "id": "shared-host",
                "kind": "host",
                "envelope": "{\"v\":1,\"n\":\"\",\"c\":\"\"}",
                "revision": 1,
                "seq": 0,
                "deleted": false,
                // Older than what is stored, so last-write-wins rejects it.
                "updated_at": 1
            }]
        }))
        .send()
        .await
        .expect("stale push")
        .json()
        .await
        .expect("stale json");
    assert_eq!(
        stale["rejected"], 1,
        "an older write must not overwrite: {stale}"
    );
}
