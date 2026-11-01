//! Machine-bound encryption for the locally stored credentials.
//!
//! The token file must survive bulk disk leakage — backups, disk images,
//! sync clients — without asking the user for anything beyond the login
//! they already typed. A random key guards the file; where that key lives
//! is decided by [`crate::credential`]: the OS keyring when one works, a
//! `0600` `keyfile` next to the data otherwise.
//!
//! This is honest about what it is: an attacker who can run code as the
//! user does not care about either file — they would just run `gita` and
//! use the token directly. The encryption is for the offline copy of the
//! disk, and it costs the user nothing.
//!
//! Blob layout: `magic(8) | key_source(1) | nonce(24) | AEAD ciphertext`.
//! The magic doubles as the associated data, so a truncated or corrupted
//! blob fails authentication instead of decrypting to garbage. The key
//! source byte is informational — it records which store holds the key
//! that encrypted this blob — but the bytes are only ever decrypted with
//! the key the caller supplies.

use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use rand::RngCore;

/// `GITAENC` + format version. Changing the format bumps the last byte.
const MAGIC: &[u8; 8] = b"GITAENC\x01";

/// The nonce half of XChaCha20-Poly1305. Random per write: 24 bytes is
/// large enough that generation collisions are not a concern.
const NONCE_LEN: usize = 24;

/// Length of the encryption key.
pub const KEY_LEN: usize = 32;

/// Where the encryption key lives.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeySource {
    /// The OS keyring (service `gita`, account `credentials-key`).
    Keyring,
    /// The local `keyfile` next to the credential store.
    KeyFile,
}

impl KeySource {
    fn to_byte(self) -> u8 {
        match self {
            KeySource::Keyring => 0,
            KeySource::KeyFile => 1,
        }
    }

    /// The key source recorded in a blob's header.
    pub fn from_blob(blob: &[u8]) -> Option<Self> {
        let byte = *blob.get(MAGIC.len())?;
        match byte {
            0 => Some(Self::Keyring),
            1 => Some(Self::KeyFile),
            _ => None,
        }
    }
}

/// A fresh random key.
pub fn generate_key() -> [u8; KEY_LEN] {
    let mut key = [0u8; KEY_LEN];
    rand::rngs::OsRng.fill_bytes(&mut key);
    key
}

/// Encrypt `plaintext` under `key`, prefixed with the format header.
pub fn encrypt(key: &[u8; KEY_LEN], source: KeySource, plaintext: &[u8]) -> anyhow::Result<Vec<u8>> {
    let cipher = XChaCha20Poly1305::new(key.into());
    let mut nonce = [0u8; NONCE_LEN];
    rand::rngs::OsRng.fill_bytes(&mut nonce);
    let ciphertext = cipher
        .encrypt(
            XNonce::from_slice(&nonce),
            Payload {
                msg: plaintext,
                aad: MAGIC,
            },
        )
        .map_err(|_| anyhow::anyhow!("credential encryption failed"))?;

    let mut blob = Vec::with_capacity(MAGIC.len() + 1 + NONCE_LEN + ciphertext.len());
    blob.extend_from_slice(MAGIC);
    blob.push(source.to_byte());
    blob.extend_from_slice(&nonce);
    blob.extend_from_slice(&ciphertext);
    Ok(blob)
}

/// Decrypt a blob written by [`encrypt`].
///
/// A wrong key, a corrupted file and a truncated file all end here — the
/// caller cannot (and should not need to) tell them apart: the answer is
/// "the credentials are gone, log in again".
pub fn decrypt(key: &[u8; KEY_LEN], blob: &[u8]) -> anyhow::Result<Vec<u8>> {
    let header = MAGIC.len() + 1 + NONCE_LEN;
    if blob.len() < header + 16 {
        anyhow::bail!("credential file is truncated or not a gita credential file");
    }
    if &blob[..MAGIC.len()] != MAGIC {
        anyhow::bail!("credential file is not a gita credential file");
    }
    let cipher = XChaCha20Poly1305::new(key.into());
    cipher
        .decrypt(
            XNonce::from_slice(&blob[MAGIC.len() + 1..header]),
            Payload {
                msg: &blob[header..],
                aad: MAGIC,
            },
        )
        .map_err(|_| anyhow::anyhow!("credential file cannot be decrypted with this key"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The round trip returns exactly what went in — the credential store
    /// reads back tokens byte for byte.
    #[test]
    fn encrypt_then_decrypt_round_trips() {
        let key = generate_key();
        let plaintext = b"token-json-payload";
        let blob = encrypt(&key, KeySource::Keyring, plaintext).unwrap();
        assert_eq!(decrypt(&key, &blob).unwrap(), plaintext);
    }

    /// A different key must fail authentication — not decrypt to garbage.
    /// This is how "the key is gone" is detected.
    #[test]
    fn a_wrong_key_fails_authentication() {
        let key = generate_key();
        let other = generate_key();
        let blob = encrypt(&key, KeySource::Keyring, b"secret").unwrap();
        let err = decrypt(&other, &blob).unwrap_err();
        assert!(err.to_string().contains("cannot be decrypted"), "{err}");
    }

    /// Truncated and foreign files must be rejected, not decrypted.
    #[test]
    fn damaged_blobs_are_rejected() {
        let key = generate_key();
        let blob = encrypt(&key, KeySource::KeyFile, b"secret").unwrap();

        let truncated = &blob[..blob.len() - 4];
        assert!(decrypt(&key, truncated).is_err());

        let mut corrupted = blob.clone();
        let last = corrupted.len() - 1;
        corrupted[last] ^= 0xff;
        assert!(decrypt(&key, &corrupted).is_err());

        let foreign = b"not a gita file at all, just text";
        assert!(decrypt(&key, foreign).is_err());
    }

    /// The recorded key source survives the round trip, so a reader can
    /// tell where to look for the key before trying to decrypt.
    #[test]
    fn the_key_source_is_recorded_in_the_header() {
        let key = generate_key();
        let blob = encrypt(&key, KeySource::KeyFile, b"secret").unwrap();
        assert_eq!(KeySource::from_blob(&blob), Some(KeySource::KeyFile));
        let blob = encrypt(&key, KeySource::Keyring, b"secret").unwrap();
        assert_eq!(KeySource::from_blob(&blob), Some(KeySource::Keyring));
    }
}
