//! Cryptography: master-key derivation, vault-key wrapping, and the item envelope.
//!
//! Key hierarchy (mirrors the blueprint's design):
//!
//! ```text
//! master password
//!  └─ Argon2id(salt, m=64MiB, t=3, p=4) → master_key(32B)
//!      ├─ HKDF-SHA256("loki/enc/v1")  → enc_key   (client only, never leaves this process)
//!      └─ HKDF-SHA256("loki/auth/v1") → auth_key  (what a sync backend would see)
//!
//! vault_key(32B random)
//!      └─ AES-256-GCM(enc_key, AAD="loki/vault-key/v1") → protected_vault_key
//!
//! every item → AES-256-GCM(vault_key, AAD="loki/item/v1/{item_id}")
//! ```
//!
//! The WebView never receives `enc_key`, `vault_key`, or any plaintext secret.

use aes_gcm::aead::{Aead, KeyInit, Payload};
use aes_gcm::{Aes256Gcm, Key, Nonce};
use argon2::{Algorithm, Argon2, Params, Version};
use base64::{engine::general_purpose::STANDARD as B64, Engine};
use hkdf::Hkdf;
use rand::RngCore;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;
use zeroize::{Zeroize, ZeroizeOnDrop};

pub const KEY_LEN: usize = 32;
pub const NONCE_LEN: usize = 12;
pub const SALT_LEN: usize = 16;

/// AAD domain separators. Binding each ciphertext to its purpose and id means a
/// ciphertext moved to another row or item id will not decrypt.
const AAD_VAULT_KEY: &[u8] = b"loki/vault-key/v1";
const AAD_RECOVERY_VAULT_KEY: &[u8] = b"loki/recovery-vault-key/v1";
const AAD_ITEM_PREFIX: &str = "loki/item/v1/";
/// Used by the sync milestone for the encrypted device name.
#[allow(dead_code)]
const AAD_DEVICE_PREFIX: &str = "loki/device/v1/";

/// Argon2 upper bounds: a hostile backend cannot make a device allocate gigabytes.
const MAX_M_KIB: u32 = 512 * 1024;
const MAX_T: u32 = 32;
const MAX_P: u32 = 16;
#[allow(dead_code)]
const ARGON2_VERSION: u32 = 19;

#[derive(Debug, Error)]
pub enum CryptoError {
    #[error("密钥派生参数低于安全下限")]
    WeakKdfParams,
    #[error("不支持的密钥派生算法")]
    UnsupportedKdf,
    #[error("不支持的 Argon2 版本")]
    UnsupportedVersion,
    #[error("密码派生失败")]
    Hash,
    #[error("解密失败")]
    Decrypt,
    #[error("密文格式无效")]
    Malformed,
    #[error("随机数生成失败")]
    Random,
}

/// Argon2id parameters, validated on construction and on deserialization.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct KdfParams {
    pub m_kib: u32,
    pub t: u32,
    pub p: u32,
}

impl Default for KdfParams {
    /// m = 64 MiB, t = 3, p = 4.
    fn default() -> Self {
        Self {
            m_kib: 64 * 1024,
            t: 3,
            p: 4,
        }
    }
}

impl KdfParams {
    /// The weakest parameters this build will accept. A hostile backend or a
    /// tampered local database could otherwise downgrade the KDF so a derived
    /// key becomes cheap to brute-force.
    pub const FLOOR: Self = Self {
        m_kib: 64 * 1024,
        t: 3,
        p: 1,
    };

    pub fn validate(&self) -> Result<(), CryptoError> {
        if self.p > MAX_P || self.t > MAX_T || self.m_kib > MAX_M_KIB {
            return Err(CryptoError::WeakKdfParams);
        }
        if self.t < Self::FLOOR.t || self.p < Self::FLOOR.p || self.m_kib < Self::FLOOR.m_kib {
            return Err(CryptoError::WeakKdfParams);
        }
        // Argon2 requires at least 8 KiB per lane.
        if self.m_kib < 8 * self.p {
            return Err(CryptoError::WeakKdfParams);
        }
        Ok(())
    }
}

/// The two subkeys a master password produces.
#[derive(Zeroize, ZeroizeOnDrop)]
pub struct MasterKeys {
    pub enc_key: [u8; KEY_LEN],
    pub auth_key: [u8; KEY_LEN],
}

/// Derives 32 bytes from the password with Argon2id, then splits it by HKDF.
pub fn derive_master_keys(
    password: &str,
    salt: &[u8],
    params: KdfParams,
) -> Result<MasterKeys, CryptoError> {
    params.validate()?;

    let argon_params = Params::new(params.m_kib, params.t, params.p, Some(KEY_LEN))
        .map_err(|_| CryptoError::WeakKdfParams)?;
    let argon = Argon2::new(Algorithm::Argon2id, Version::V0x13, argon_params);

    let mut master_key = [0u8; KEY_LEN];
    argon
        .hash_password_into(password.as_bytes(), salt, &mut master_key)
        .map_err(|_| CryptoError::Hash)?;

    let hk = Hkdf::<Sha256>::new(None, &master_key);
    let mut enc_key = [0u8; KEY_LEN];
    let mut auth_key = [0u8; KEY_LEN];
    hk.expand(b"loki/enc/v1", &mut enc_key)
        .map_err(|_| CryptoError::Hash)?;
    hk.expand(b"loki/auth/v1", &mut auth_key)
        .map_err(|_| CryptoError::Hash)?;

    master_key.zeroize();
    Ok(MasterKeys { enc_key, auth_key })
}

/// 32 random bytes, used for the vault key and salts.
pub fn random_bytes<const N: usize>() -> Result<[u8; N], CryptoError> {
    let mut out = [0u8; N];
    rand::thread_rng()
        .try_fill_bytes(&mut out)
        .map_err(|_| CryptoError::Random)?;
    Ok(out)
}

/// A sealed blob in the on-disk / on-wire format.
///
/// JSON: `{"v":1,"n":"<b64 12-byte nonce>","c":"<b64 ciphertext||16-byte GCM tag>"}`
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Envelope {
    pub v: u8,
    pub n: String,
    pub c: String,
}

/// Encrypts `plaintext` under `key`, authenticating `aad`.
pub fn seal(key: &[u8; KEY_LEN], aad: &[u8], plaintext: &[u8]) -> Result<Envelope, CryptoError> {
    let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(key));
    let nonce_bytes = random_bytes::<NONCE_LEN>()?;
    let nonce = Nonce::from_slice(&nonce_bytes);

    let ct = cipher
        .encrypt(
            nonce,
            Payload {
                msg: plaintext,
                aad,
            },
        )
        .map_err(|_| CryptoError::Decrypt)?;

    Ok(Envelope {
        v: 1,
        n: B64.encode(nonce_bytes),
        c: B64.encode(ct),
    })
}

/// Decrypts an envelope. Version, nonce length and tag are all checked.
pub fn open(key: &[u8; KEY_LEN], aad: &[u8], env: &Envelope) -> Result<Vec<u8>, CryptoError> {
    if env.v != 1 {
        return Err(CryptoError::Malformed);
    }
    let nonce_bytes = B64.decode(&env.n).map_err(|_| CryptoError::Malformed)?;
    if nonce_bytes.len() != NONCE_LEN {
        return Err(CryptoError::Malformed);
    }
    let ct = B64.decode(&env.c).map_err(|_| CryptoError::Malformed)?;

    let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(key));
    cipher
        .decrypt(Nonce::from_slice(&nonce_bytes), Payload { msg: &ct, aad })
        .map_err(|_| CryptoError::Decrypt)
}

/// Wraps a random `vault_key` under `enc_key`.
pub fn wrap_vault_key(
    enc_key: &[u8; KEY_LEN],
    vault_key: &[u8; KEY_LEN],
) -> Result<Envelope, CryptoError> {
    seal(enc_key, AAD_VAULT_KEY, vault_key)
}

/// Unwraps `vault_key`. Fails with `Decrypt` for a wrong password.
pub fn unwrap_vault_key(
    enc_key: &[u8; KEY_LEN],
    env: &Envelope,
) -> Result<[u8; KEY_LEN], CryptoError> {
    let bytes = open(enc_key, AAD_VAULT_KEY, env)?;
    if bytes.len() != KEY_LEN {
        return Err(CryptoError::Malformed);
    }
    let mut out = [0u8; KEY_LEN];
    out.copy_from_slice(&bytes);
    Ok(out)
}

/// Derives the 32-byte key that protects the recovery-wrapped vault key.
///
/// The recovery secret is already 128 bits of CSPRNG output, so a plain HKDF
/// with a domain-separating info string is enough; no password stretching.
pub fn derive_recovery_key(code: &RecoveryCode) -> [u8; KEY_LEN] {
    let hk = Hkdf::<Sha256>::new(None, code.secret());
    let mut out = [0u8; KEY_LEN];
    // 32 bytes into a 32-byte buffer cannot fail.
    let _ = hk.expand(b"loki/recovery/v1", &mut out);
    out
}

/// Wraps `vault_key` under the recovery key, under its own AAD.
pub fn wrap_vault_key_recovery(
    recovery_key: &[u8; KEY_LEN],
    vault_key: &[u8; KEY_LEN],
) -> Result<Envelope, CryptoError> {
    seal(recovery_key, AAD_RECOVERY_VAULT_KEY, vault_key)
}

/// Unwraps the recovery copy of `vault_key`.
pub fn unwrap_vault_key_recovery(
    recovery_key: &[u8; KEY_LEN],
    env: &Envelope,
) -> Result<[u8; KEY_LEN], CryptoError> {
    let bytes = open(recovery_key, AAD_RECOVERY_VAULT_KEY, env)?;
    if bytes.len() != KEY_LEN {
        return Err(CryptoError::Malformed);
    }
    let mut out = [0u8; KEY_LEN];
    out.copy_from_slice(&bytes);
    Ok(out)
}

// ---- Recovery code -------------------------------------------------------

/// 128-bit secret plus a 4-byte checksum, rendered as 32 Crockford Base32
/// characters in eight groups of four.
///
/// The checksum lets a typo be rejected before any expensive work, and the
/// alphabet drops I, L, O and U so hand-copying cannot confuse them.
pub const RECOVERY_SECRET_LEN: usize = 16;
const RECOVERY_CHECK_LEN: usize = 4;
const CROCKFORD: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";

/// A recovery code: the text the user writes down, and the bytes it stands for.
#[derive(Clone, PartialEq, Eq)]
pub struct RecoveryCode {
    pub display: String,
    secret: [u8; RECOVERY_SECRET_LEN],
}

impl std::fmt::Debug for RecoveryCode {
    /// Never print the secret by accident; logs go to disk.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RecoveryCode")
            .field("display", &"<redacted>")
            .finish()
    }
}

impl RecoveryCode {
    /// The raw secret, for deriving the recovery key.
    pub fn secret(&self) -> &[u8; RECOVERY_SECRET_LEN] {
        &self.secret
    }

    /// Generates a fresh code.
    pub fn generate() -> Result<Self, CryptoError> {
        let secret = random_bytes::<{ RECOVERY_SECRET_LEN }>()?;
        Ok(Self {
            display: format_code(&secret),
            secret,
        })
    }

    /// Parses what the user typed. Accepts spaces, dashes and lower case, and
    /// maps the letters Crockford says are interchangeable.
    pub fn parse(input: &str) -> Result<Self, CryptoError> {
        let mut bits: Vec<u8> = Vec::with_capacity(160);
        for ch in input.chars() {
            let c = ch.to_ascii_uppercase();
            if c == ' ' || c == '-' || c == '\t' {
                continue;
            }
            // Crockford aliases: I and L read as 1, O reads as 0. U is rejected
            // so a mistyped letter cannot silently become a digit.
            let c = match c {
                'I' | 'L' => '1',
                'O' => '0',
                'U' => return Err(CryptoError::Malformed),
                other => other,
            };
            let idx = CROCKFORD
                .iter()
                .position(|&b| b as char == c)
                .ok_or(CryptoError::Malformed)?;
            for shift in (0..5).rev() {
                bits.push(((idx >> shift) & 1) as u8);
            }
        }

        let expected = (RECOVERY_SECRET_LEN + RECOVERY_CHECK_LEN) * 8;
        if bits.len() != expected {
            return Err(CryptoError::Malformed);
        }

        let mut bytes = vec![0u8; expected / 8];
        for (i, bit) in bits.iter().enumerate() {
            if *bit == 1 {
                bytes[i / 8] |= 1 << (7 - (i % 8));
            }
        }

        let (secret_part, check_part) = bytes.split_at(RECOVERY_SECRET_LEN);
        let mut secret = [0u8; RECOVERY_SECRET_LEN];
        secret.copy_from_slice(secret_part);

        // Constant-time compare: a mismatched checksum must not reveal how many
        // digits were right.
        let want = checksum(&secret);
        let mut diff = 0u8;
        for (a, b) in want.iter().zip(check_part.iter()) {
            diff |= a ^ b;
        }
        if diff != 0 {
            return Err(CryptoError::Malformed);
        }

        Ok(Self {
            display: format_code(&secret),
            secret,
        })
    }
}

/// First four bytes of SHA-256(secret), which is the trailing check group.
fn checksum(secret: &[u8; RECOVERY_SECRET_LEN]) -> [u8; RECOVERY_CHECK_LEN] {
    let mut h = Sha256::new();
    h.update(secret);
    let digest = h.finalize();
    let mut out = [0u8; RECOVERY_CHECK_LEN];
    out.copy_from_slice(&digest[..RECOVERY_CHECK_LEN]);
    out
}

/// Renders 20 bytes as 32 base32 characters, grouped 4 by 4.
fn format_code(secret: &[u8; RECOVERY_SECRET_LEN]) -> String {
    let mut bytes = Vec::with_capacity(RECOVERY_SECRET_LEN + RECOVERY_CHECK_LEN);
    bytes.extend_from_slice(secret);
    bytes.extend_from_slice(&checksum(secret));

    // 160 bits -> exactly 32 five-bit symbols.
    let mut symbols = String::with_capacity(32);
    for chunk in bytes.chunks(5) {
        for i in 0..8 {
            let bit = i * 5;
            let mut value = 0u8;
            for j in 0..5 {
                let pos = bit + j;
                let byte = chunk[pos / 8];
                let bit_in_byte = 7 - (pos % 8);
                value = (value << 1) | ((byte >> bit_in_byte) & 1);
            }
            symbols.push(CROCKFORD[value as usize] as char);
        }
    }

    symbols
        .as_bytes()
        .chunks(4)
        .map(|c| std::str::from_utf8(c).unwrap_or(""))
        .collect::<Vec<_>>()
        .join(" ")
}

/// AAD for one item: the ciphertext is bound to its id, so a server cannot move
/// a blob from one item to another.
pub fn item_aad(item_id: &str) -> Vec<u8> {
    format!("{AAD_ITEM_PREFIX}{item_id}").into_bytes()
}

/// AAD for a device name, which syncs encrypted but is displayed.
/// Used by the sync milestone for the encrypted device name.
#[allow(dead_code)]
pub fn device_aad(device_id: &str) -> Vec<u8> {
    format!("{AAD_DEVICE_PREFIX}{device_id}").into_bytes()
}

/// SHA-256 for what a server is allowed to store (never the key itself).
/// What a sync backend is allowed to store; used from the sync milestone.
#[allow(dead_code)]
pub fn sha256_hex(data: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(data);
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn floor_rejects_weak_parameters() {
        assert!(KdfParams {
            m_kib: 4,
            t: 1,
            p: 1
        }
        .validate()
        .is_err());
        assert!(KdfParams {
            m_kib: 64 * 1024,
            t: 1,
            p: 1
        }
        .validate()
        .is_err());
        assert!(KdfParams {
            m_kib: 64 * 1024,
            t: 3,
            p: 1
        }
        .validate()
        .is_ok());
        assert!(KdfParams {
            m_kib: 1024 * 1024,
            t: 3,
            p: 1
        }
        .validate()
        .is_err());
    }

    #[test]
    fn seal_and_open_roundtrip() {
        let key = [7u8; KEY_LEN];
        let env = seal(&key, b"aad", b"hello loki").unwrap();
        assert_eq!(open(&key, b"aad", &env).unwrap(), b"hello loki");
    }

    #[test]
    fn aad_mismatch_is_rejected() {
        let key = [7u8; KEY_LEN];
        let env = seal(&key, b"item-a", b"secret").unwrap();
        assert!(open(&key, b"item-b", &env).is_err());
    }

    #[test]
    fn vault_key_wrap_roundtrip() {
        let enc = [1u8; KEY_LEN];
        let vk = [9u8; KEY_LEN];
        let env = wrap_vault_key(&enc, &vk).unwrap();
        assert_eq!(unwrap_vault_key(&enc, &env).unwrap(), vk);
        // A different password must fail rather than return garbage.
        assert!(unwrap_vault_key(&[2u8; KEY_LEN], &env).is_err());
    }

    #[test]
    fn derivation_refuses_relaxed_params() {
        // Even relaxed test params must not sneak past the production floor.
        assert!(matches!(
            derive_master_keys(
                "pw",
                &[0u8; SALT_LEN],
                KdfParams {
                    m_kib: 8,
                    t: 1,
                    p: 1
                }
            ),
            Err(CryptoError::WeakKdfParams)
        ));
    }

    #[test]
    fn recovery_code_roundtrips() {
        let code = RecoveryCode::generate().unwrap();
        // Eight groups of four characters.
        let groups: Vec<&str> = code.display.split(' ').collect();
        assert_eq!(groups.len(), 8);
        assert!(groups.iter().all(|g| g.len() == 4), "{:?}", code.display);

        let parsed = RecoveryCode::parse(&code.display).unwrap();
        assert_eq!(parsed.secret(), code.secret());
    }

    #[test]
    fn recovery_parse_is_forgiving_about_typing() {
        let code = RecoveryCode::generate().unwrap();
        let messy = code.display.to_lowercase().replace(' ', "-");
        assert!(RecoveryCode::parse(&messy).is_ok());
        // Crockford aliases: O -> 0, I/L -> 1.
        let with_zero = code.display.replace('0', "O");
        assert!(RecoveryCode::parse(&with_zero).is_ok());
    }

    #[test]
    fn recovery_rejects_a_typo() {
        let code = RecoveryCode::generate().unwrap();
        let mut chars: Vec<char> = code.display.chars().collect();
        // Flip one character to something else in the alphabet.
        let idx = chars.iter().position(|c| *c != ' ').unwrap();
        chars[idx] = if chars[idx] == '0' { '1' } else { '0' };
        let broken: String = chars.into_iter().collect();

        assert!(
            RecoveryCode::parse(&broken).is_err(),
            "a corrupted code must not parse; the checksum exists for this"
        );
    }

    #[test]
    fn recovery_rejects_wrong_length_and_bad_letters() {
        assert!(RecoveryCode::parse("").is_err());
        assert!(RecoveryCode::parse("ZZZZ ZZZZ ZZZZ ZZZZ").is_err());
        let code = RecoveryCode::generate().unwrap();
        // U is excluded on purpose so it cannot be confused with V.
        let with_u = code
            .display
            .replacen(|c: char| c.is_ascii_alphanumeric(), "U", 1);
        assert!(RecoveryCode::parse(&with_u).is_err());
        // A truncated code is not silently padded.
        assert!(RecoveryCode::parse(&code.display[..10]).is_err());
    }

    #[test]
    fn master_keys_differ_by_domain() {
        // Uses the production floor so the derivation actually runs.
        let salt = [3u8; SALT_LEN];
        let keys = derive_master_keys("correct horse", &salt, KdfParams::FLOOR).unwrap();
        assert_ne!(keys.enc_key, keys.auth_key);

        // Deterministic for the same password and salt...
        let again = derive_master_keys("correct horse", &salt, KdfParams::FLOOR).unwrap();
        assert_eq!(keys.enc_key, again.enc_key);

        // ...and different for a different salt.
        let other =
            derive_master_keys("correct horse", &[4u8; SALT_LEN], KdfParams::FLOOR).unwrap();
        assert_ne!(keys.enc_key, other.enc_key);
    }
}
