//! The vault: owns the derived keys and the unlocked `vault_key`, and turns
//! plaintext items into encrypted rows. Nothing here ever hands a key to the
//! caller — the only way out is [`Vault::encrypt_item`] / [`Vault::decrypt_item`].

use crate::crypto::{self, Envelope, KdfParams, RecoveryCode};
use crate::store::{ItemRow, Store, StoreError};
use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use zeroize::Zeroize;

const META_SALT: &str = "kdf_salt";
const META_PARAMS: &str = "kdf_params";
const META_PROTECTED_VAULT_KEY: &str = "protected_vault_key";
const META_RECOVERY_VAULT_KEY: &str = "recovery_vault_key";

#[derive(Debug, Error)]
pub enum VaultError {
    #[error("保险库已存在")]
    AlreadyExists,
    #[error("尚未创建保险库")]
    NotInitialized,
    #[error("保险库已锁定")]
    Locked,
    #[error("主密码错误")]
    WrongPassword,
    #[error("恢复码错误")]
    WrongRecoveryCode,
    #[error(transparent)]
    Crypto(#[from] crypto::CryptoError),
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error("保险库元数据损坏：{0}")]
    Corrupt(String),
}

/// What the WebView is allowed to know about the vault.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VaultStatus {
    pub exists: bool,
    pub unlocked: bool,
    pub kdf: Option<KdfParams>,
}

/// A decrypted item, as plain data handed back to the caller.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlainItem {
    pub id: String,
    pub kind: String,
    pub payload: serde_json::Value,
}

pub struct Vault {
    store: Store,
    vault_key: Option<[u8; crypto::KEY_LEN]>,
    params: Option<KdfParams>,
    salt: Option<Vec<u8>>,
}

impl Vault {
    pub fn new(store: Store) -> Result<Self, VaultError> {
        let salt = store
            .get_meta(META_SALT)?
            .map(|s| B64.decode(s).unwrap_or_default());
        let params = match store.get_meta(META_PARAMS)? {
            Some(json) => {
                Some(serde_json::from_str(&json).map_err(|e| VaultError::Corrupt(e.to_string()))?)
            }
            None => None,
        };
        Ok(Self {
            store,
            vault_key: None,
            params,
            salt,
        })
    }

    pub fn status(&self) -> VaultStatus {
        VaultStatus {
            exists: self.salt.is_some(),
            unlocked: self.vault_key.is_some(),
            kdf: self.params,
        }
    }

    /// Creates a fresh vault. Refuses when one already exists.
    /// Creates a vault and returns the recovery code.
    ///
    /// The code is the only way back in if the password is forgotten, and it is
    /// shown exactly once: only the wrapped key it protects is stored.
    pub fn create(
        &mut self,
        password: &str,
        params: KdfParams,
    ) -> Result<RecoveryCode, VaultError> {
        if self.status().exists {
            return Err(VaultError::AlreadyExists);
        }
        params.validate()?;

        let salt = crypto::random_bytes::<{ crypto::SALT_LEN }>()?;
        let keys = crypto::derive_master_keys(password, &salt, params)?;

        let vault_key = crypto::random_bytes::<{ crypto::KEY_LEN }>()?;
        let protected = crypto::wrap_vault_key(&keys.enc_key, &vault_key)?;

        // A second copy of the vault key, wrapped by a key derived from the
        // recovery code, so the password is not a single point of failure.
        let code = RecoveryCode::generate()?;
        let recovery_key = crypto::derive_recovery_key(&code);
        let recovery_wrapped = crypto::wrap_vault_key_recovery(&recovery_key, &vault_key)?;

        self.store.set_meta(META_SALT, &B64.encode(salt))?;
        self.store.set_meta(
            META_PARAMS,
            &serde_json::to_string(&params).map_err(|e| VaultError::Corrupt(e.to_string()))?,
        )?;
        self.store.set_meta(
            META_PROTECTED_VAULT_KEY,
            &serde_json::to_string(&protected).map_err(|e| VaultError::Corrupt(e.to_string()))?,
        )?;
        self.store.set_meta(
            META_RECOVERY_VAULT_KEY,
            &serde_json::to_string(&recovery_wrapped)
                .map_err(|e| VaultError::Corrupt(e.to_string()))?,
        )?;

        self.salt = Some(salt.to_vec());
        self.params = Some(params);
        self.vault_key = Some(vault_key);
        Ok(code)
    }

    /// Unlocks with a recovery code instead of the master password.
    ///
    /// This does not change the password; it just gets the vault key back so the
    /// UI can let the user set a new one.
    pub fn unlock_with_recovery(&mut self, code: &RecoveryCode) -> Result<(), VaultError> {
        if self.salt.is_none() {
            return Err(VaultError::NotInitialized);
        }
        let raw = self
            .store
            .get_meta(META_RECOVERY_VAULT_KEY)?
            .ok_or_else(|| VaultError::Corrupt("missing recovery_vault_key".into()))?;
        let env: Envelope =
            serde_json::from_str(&raw).map_err(|e| VaultError::Corrupt(e.to_string()))?;

        let recovery_key = crypto::derive_recovery_key(code);
        let vault_key = crypto::unwrap_vault_key_recovery(&recovery_key, &env)
            .map_err(|_| VaultError::WrongRecoveryCode)?;

        self.vault_key = Some(vault_key);
        Ok(())
    }

    /// Replaces the master password, keeping every item readable.
    ///
    /// Only the wrapped vault key is re-encrypted; item ciphertexts are bound to
    /// their own AAD and stay untouched.
    pub fn change_password(
        &mut self,
        new_password: &str,
        params: KdfParams,
    ) -> Result<(), VaultError> {
        let vault_key = *self.key()?;
        params.validate()?;

        let salt = crypto::random_bytes::<{ crypto::SALT_LEN }>()?;
        let keys = crypto::derive_master_keys(new_password, &salt, params)?;
        let protected = crypto::wrap_vault_key(&keys.enc_key, &vault_key)?;

        self.store.set_meta(META_SALT, &B64.encode(salt))?;
        self.store.set_meta(
            META_PARAMS,
            &serde_json::to_string(&params).map_err(|e| VaultError::Corrupt(e.to_string()))?,
        )?;
        self.store.set_meta(
            META_PROTECTED_VAULT_KEY,
            &serde_json::to_string(&protected).map_err(|e| VaultError::Corrupt(e.to_string()))?,
        )?;

        self.salt = Some(salt.to_vec());
        self.params = Some(params);
        Ok(())
    }

    /// Unlocks with the master password.
    pub fn unlock(&mut self, password: &str) -> Result<(), VaultError> {
        let salt = self.salt.as_ref().ok_or(VaultError::NotInitialized)?;
        let params = self.params.ok_or(VaultError::NotInitialized)?;

        let protected_raw = self
            .store
            .get_meta(META_PROTECTED_VAULT_KEY)?
            .ok_or_else(|| VaultError::Corrupt("missing protected_vault_key".into()))?;
        let protected: Envelope =
            serde_json::from_str(&protected_raw).map_err(|e| VaultError::Corrupt(e.to_string()))?;

        let keys = crypto::derive_master_keys(password, salt, params)?;
        // A wrong password fails the GCM tag check; there is no way to tell it
        // apart from a corrupted blob, which is intended.
        let vault_key = crypto::unwrap_vault_key(&keys.enc_key, &protected)
            .map_err(|_| VaultError::WrongPassword)?;

        self.vault_key = Some(vault_key);
        Ok(())
    }

    /// Drops the keys from memory. The caller must not hold copies elsewhere.
    pub fn lock(&mut self) {
        if let Some(mut k) = self.vault_key.take() {
            k.zeroize();
        }
    }

    fn key(&self) -> Result<&[u8; crypto::KEY_LEN], VaultError> {
        self.vault_key.as_ref().ok_or(VaultError::Locked)
    }

    /// Encrypts one item under the unlocked vault key.
    pub fn encrypt_item(
        &self,
        id: &str,
        kind: &str,
        payload: &serde_json::Value,
    ) -> Result<ItemRow, VaultError> {
        let plaintext =
            serde_json::to_vec(payload).map_err(|e| VaultError::Corrupt(e.to_string()))?;
        let envelope = crypto::seal(self.key()?, &crypto::item_aad(id), &plaintext)?;
        Ok(ItemRow {
            id: id.to_string(),
            kind: kind.to_string(),
            envelope,
            updated_at: now_secs(),
        })
    }

    /// Decrypts one stored row back to plain data.
    pub fn decrypt_item(&self, row: &ItemRow) -> Result<PlainItem, VaultError> {
        let bytes = crypto::open(self.key()?, &crypto::item_aad(&row.id), &row.envelope)?;
        let payload =
            serde_json::from_slice(&bytes).map_err(|e| VaultError::Corrupt(e.to_string()))?;
        Ok(PlainItem {
            id: row.id.clone(),
            kind: row.kind.clone(),
            payload,
        })
    }

    pub fn store(&self) -> &Store {
        &self.store
    }
}

fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fast() -> KdfParams {
        KdfParams {
            m_kib: 8,
            t: 1,
            p: 1,
        }
    }

    #[test]
    fn create_then_status_exists() {
        let store = Store::open_in_memory().unwrap();
        let mut vault = Vault::new(store).unwrap();
        assert!(!vault.status().exists);
        // Production floor params are accepted.
        vault
            .create("correct horse battery staple", KdfParams::default())
            .unwrap();
        let st = vault.status();
        assert!(st.exists);
        assert!(st.unlocked);
        // Creating twice is refused rather than silently overwriting keys.
        assert!(vault.create("other", KdfParams::default()).is_err());
    }

    #[test]
    fn unlock_rejects_wrong_password() {
        let store = Store::open_in_memory().unwrap();
        let mut vault = Vault::new(store).unwrap();
        vault
            .create("right-password", KdfParams::default())
            .unwrap();
        vault.lock();
        assert!(vault.status().exists && !vault.status().unlocked);
        assert!(matches!(
            vault.unlock("wrong-password"),
            Err(VaultError::WrongPassword)
        ));
        vault.unlock("right-password").unwrap();
        assert!(vault.status().unlocked);
    }

    #[test]
    fn items_are_encrypted_at_rest() {
        let store = Store::open_in_memory().unwrap();
        let mut vault = Vault::new(store).unwrap();
        vault.create("pw", KdfParams::default()).unwrap();

        let payload = serde_json::json!({ "name": "prod-api-tokyo", "address": "10.0.12.31" });
        let row = vault.encrypt_item("item-1", "host", &payload).unwrap();
        vault.store().put_item(&row).unwrap();

        // The stored bytes must not contain the plaintext anywhere.
        let raw = serde_json::to_string(&row.envelope).unwrap();
        assert!(!raw.contains("prod-api-tokyo"));
        assert!(!raw.contains("10.0.12.31"));

        let back = vault.decrypt_item(&row).unwrap();
        assert_eq!(back.payload, payload);
    }

    #[test]
    fn locked_vault_refuses_operations() {
        let store = Store::open_in_memory().unwrap();
        let mut vault = Vault::new(store).unwrap();
        vault.create("pw", KdfParams::default()).unwrap();
        vault.lock();
        assert!(matches!(
            vault.encrypt_item("x", "host", &serde_json::json!({})),
            Err(VaultError::Locked)
        ));
        assert!(matches!(
            vault.decrypt_item(&locked_row()),
            Err(VaultError::Locked)
        ));
    }

    fn locked_row() -> ItemRow {
        ItemRow {
            id: "x".into(),
            kind: "host".into(),
            envelope: Envelope {
                v: 1,
                n: "AA".into(),
                c: "AA".into(),
            },
            updated_at: 0,
        }
    }

    #[test]
    fn item_ids_are_bound_by_aad() {
        let store = Store::open_in_memory().unwrap();
        let mut vault = Vault::new(store).unwrap();
        vault.create("pw", KdfParams::default()).unwrap();

        let row = vault
            .encrypt_item("original", "host", &serde_json::json!({"a": 1}))
            .unwrap();
        // Move the same ciphertext under a different id: AAD breaks.
        let moved = ItemRow {
            id: "stolen".into(),
            ..row
        };
        assert!(vault.decrypt_item(&moved).is_err());
    }

    #[test]
    fn recovery_code_unlocks_without_the_password() {
        let store = Store::open_in_memory().unwrap();
        let mut vault = Vault::new(store).unwrap();
        let code = vault.create("the-password", KdfParams::default()).unwrap();

        // Something is stored, then the vault is locked and the password is
        // "forgotten".
        let payload = serde_json::json!({ "name": "prod-api-tokyo" });
        let row = vault.encrypt_item("h1", "host", &payload).unwrap();
        vault.store().put_item(&row).unwrap();
        vault.lock();

        // The recovery code gets the vault key back.
        vault.unlock_with_recovery(&code).unwrap();
        let back = vault.decrypt_item(&row).unwrap();
        assert_eq!(back.payload, payload);

        // And it sets a new password, after which the old one stops working.
        vault
            .change_password("a-new-password", KdfParams::default())
            .unwrap();
        vault.lock();
        assert!(matches!(
            vault.unlock("the-password"),
            Err(VaultError::WrongPassword)
        ));
        vault.unlock("a-new-password").unwrap();
        assert_eq!(vault.decrypt_item(&row).unwrap().payload, payload);
    }

    #[test]
    fn a_wrong_recovery_code_is_rejected() {
        let store = Store::open_in_memory().unwrap();
        let mut vault = Vault::new(store).unwrap();
        vault.create("pw", KdfParams::default()).unwrap();
        vault.lock();

        // A well-formed but different code must not unwrap the vault key.
        let other = RecoveryCode::generate().unwrap();
        assert!(matches!(
            vault.unlock_with_recovery(&other),
            Err(VaultError::WrongRecoveryCode)
        ));
    }

    #[test]
    fn changing_the_password_does_not_rewrite_items() {
        let store = Store::open_in_memory().unwrap();
        let mut vault = Vault::new(store).unwrap();
        vault.create("pw", KdfParams::default()).unwrap();

        let row = vault
            .encrypt_item("h1", "host", &serde_json::json!({ "name": "x" }))
            .unwrap();
        vault.store().put_item(&row).unwrap();
        let before = serde_json::to_string(&row.envelope).unwrap();

        vault.change_password("pw2", KdfParams::default()).unwrap();

        // Item ciphertext is bound to its own AAD, so it is untouched: changing
        // the password must be O(1), not a re-encryption of the whole vault.
        let stored = vault.store().get_item("h1").unwrap().unwrap();
        assert_eq!(serde_json::to_string(&stored.envelope).unwrap(), before);
        assert_eq!(vault.decrypt_item(&stored).unwrap().payload["name"], "x");
    }

    #[test]
    fn floor_blocks_weak_params_on_create() {
        let store = Store::open_in_memory().unwrap();
        let mut vault = Vault::new(store).unwrap();
        // A malformed/hostile source cannot talk the app into a cheap KDF.
        assert!(matches!(
            vault.create("pw", fast()),
            Err(VaultError::Crypto(_))
        ));
        assert!(!vault.status().exists);
    }
}
