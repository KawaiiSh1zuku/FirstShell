//! Vault key lifecycle, OS keyring access, and authenticated record encryption.

use std::fmt;

use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use hkdf::Hkdf;
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use thiserror::Error;
use zeroize::{Zeroize, ZeroizeOnDrop, Zeroizing};

/// Size of a `FirstShell` vault key in bytes.
pub const VAULT_KEY_LEN: usize = 32;
/// Maximum plaintext or ciphertext accepted by the record encryption boundary.
pub const MAX_SEALED_BYTES: usize = 16 * 1024 * 1024;
const NONCE_LEN: usize = 24;
const PAYLOAD_VERSION: u16 = 1;

/// Root key for local record encryption.
#[derive(Zeroize, ZeroizeOnDrop)]
pub struct VaultKey([u8; VAULT_KEY_LEN]);

impl fmt::Debug for VaultKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("VaultKey([REDACTED])")
    }
}

impl VaultKey {
    /// Generates a new key with the operating system CSPRNG.
    ///
    /// # Errors
    /// Returns [`SecurityError::Random`] if secure randomness is unavailable.
    pub fn generate() -> Result<Self, SecurityError> {
        let mut bytes = [0_u8; VAULT_KEY_LEN];
        getrandom::fill(&mut bytes).map_err(|error| SecurityError::Random(error.to_string()))?;
        Ok(Self(bytes))
    }

    /// Creates a key from an exact 32-byte secret returned by a trusted key store.
    ///
    /// # Errors
    /// Returns [`SecurityError::InvalidKeyLength`] for any other length.
    pub fn from_secret(secret: &[u8]) -> Result<Self, SecurityError> {
        let bytes: [u8; VAULT_KEY_LEN] = secret
            .try_into()
            .map_err(|_| SecurityError::InvalidKeyLength(secret.len()))?;
        Ok(Self(bytes))
    }

    fn secret(&self) -> &[u8; VAULT_KEY_LEN] {
        &self.0
    }
}

/// Stable context bound into key derivation and authenticated data.
#[derive(Debug, Clone, Copy)]
pub struct RecordContext<'a> {
    /// Concrete table that owns the encrypted record.
    pub table: &'a str,
    /// Stable record identifier.
    pub record_id: &'a str,
    /// Schema version used to parse the plaintext.
    pub schema_version: u32,
    /// Optimistic-concurrency revision.
    pub revision: u64,
    /// Vault key generation used for this record.
    pub key_version: u32,
}

impl RecordContext<'_> {
    fn derivation_info(&self) -> Vec<u8> {
        let mut info = Vec::with_capacity(64 + self.table.len() + self.record_id.len());
        info.extend_from_slice(b"firstshell.record-key.v1\0");
        append_field(&mut info, self.table.as_bytes());
        append_field(&mut info, self.record_id.as_bytes());
        info.extend_from_slice(&self.key_version.to_be_bytes());
        info
    }

    fn aad(&self) -> Vec<u8> {
        let mut aad = Vec::with_capacity(64 + self.table.len() + self.record_id.len());
        aad.extend_from_slice(b"firstshell.record-aad.v1\0");
        append_field(&mut aad, self.table.as_bytes());
        append_field(&mut aad, self.record_id.as_bytes());
        aad.extend_from_slice(&self.schema_version.to_be_bytes());
        aad.extend_from_slice(&self.revision.to_be_bytes());
        aad.extend_from_slice(&self.key_version.to_be_bytes());
        aad
    }
}

fn append_field(target: &mut Vec<u8>, value: &[u8]) {
    target.extend_from_slice(&(value.len() as u64).to_be_bytes());
    target.extend_from_slice(value);
}

/// Versioned authenticated-encryption envelope stored in `SQLite`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SealedPayload {
    /// Payload encoding version.
    pub version: u16,
    /// Vault key generation used to derive the record key.
    pub key_version: u32,
    /// Unique `XChaCha20` nonce.
    pub nonce: [u8; NONCE_LEN],
    /// Ciphertext followed by its Poly1305 authentication tag.
    pub ciphertext: Vec<u8>,
}

/// Security boundary failures that are safe for infrastructure callers.
#[derive(Debug, Error)]
pub enum SecurityError {
    /// The OS CSPRNG failed.
    #[error("secure random generation failed: {0}")]
    Random(String),
    /// A trusted store returned an unexpected key size.
    #[error("vault key must be {VAULT_KEY_LEN} bytes, got {0}")]
    InvalidKeyLength(usize),
    /// Input exceeded the explicit record boundary.
    #[error("record payload exceeds {MAX_SEALED_BYTES} bytes")]
    PayloadTooLarge,
    /// HKDF could not produce the required record key.
    #[error("record key derivation failed")]
    KeyDerivation,
    /// Cipher initialization failed.
    #[error("record cipher initialization failed")]
    CipherInitialization,
    /// Authentication failed; no partial plaintext is returned.
    #[error("record authentication failed")]
    Authentication,
    /// Payload metadata does not match a supported format or context.
    #[error("unsupported sealed payload metadata")]
    UnsupportedPayload,
    /// The platform credential store rejected an operation.
    #[error("OS keyring operation failed: {0}")]
    Keyring(#[from] keyring::Error),
}

/// Encrypts one sensitive record with a fresh nonce and context-bound subkey.
///
/// # Errors
/// Returns an error when randomness or key derivation fails, the input is too
/// large, or the cipher cannot produce an authenticated envelope.
pub fn seal_record(
    vault_key: &VaultKey,
    context: RecordContext<'_>,
    plaintext: &[u8],
) -> Result<SealedPayload, SecurityError> {
    if plaintext.len() > MAX_SEALED_BYTES {
        return Err(SecurityError::PayloadTooLarge);
    }

    let record_key = derive_record_key(vault_key, context)?;
    let cipher = XChaCha20Poly1305::new_from_slice(record_key.as_ref())
        .map_err(|_| SecurityError::CipherInitialization)?;
    let mut nonce = [0_u8; NONCE_LEN];
    getrandom::fill(&mut nonce).map_err(|error| SecurityError::Random(error.to_string()))?;
    let ciphertext = cipher
        .encrypt(
            XNonce::from_slice(&nonce),
            Payload {
                msg: plaintext,
                aad: &context.aad(),
            },
        )
        .map_err(|_| SecurityError::Authentication)?;

    Ok(SealedPayload {
        version: PAYLOAD_VERSION,
        key_version: context.key_version,
        nonce,
        ciphertext,
    })
}

/// Authenticates and decrypts one record without exposing partial plaintext.
///
/// # Errors
/// Returns an error for unsupported metadata, oversized input, a wrong key or
/// context, or modified ciphertext.
pub fn open_record(
    vault_key: &VaultKey,
    context: RecordContext<'_>,
    sealed: &SealedPayload,
) -> Result<Zeroizing<Vec<u8>>, SecurityError> {
    if sealed.version != PAYLOAD_VERSION || sealed.key_version != context.key_version {
        return Err(SecurityError::UnsupportedPayload);
    }
    if sealed.ciphertext.len() > MAX_SEALED_BYTES + 16 {
        return Err(SecurityError::PayloadTooLarge);
    }

    let record_key = derive_record_key(vault_key, context)?;
    let cipher = XChaCha20Poly1305::new_from_slice(record_key.as_ref())
        .map_err(|_| SecurityError::CipherInitialization)?;
    let plaintext = cipher
        .decrypt(
            XNonce::from_slice(&sealed.nonce),
            Payload {
                msg: &sealed.ciphertext,
                aad: &context.aad(),
            },
        )
        .map_err(|_| SecurityError::Authentication)?;
    Ok(Zeroizing::new(plaintext))
}

fn derive_record_key(
    vault_key: &VaultKey,
    context: RecordContext<'_>,
) -> Result<Zeroizing<[u8; VAULT_KEY_LEN]>, SecurityError> {
    let mut key = Zeroizing::new([0_u8; VAULT_KEY_LEN]);
    Hkdf::<Sha256>::new(None, vault_key.secret())
        .expand(&context.derivation_info(), key.as_mut())
        .map_err(|_| SecurityError::KeyDerivation)?;
    Ok(key)
}

/// Minimal platform keyring adapter for one vault key.
#[derive(Debug)]
pub struct OsKeyring {
    service: String,
    account: String,
}

impl OsKeyring {
    /// Creates a namespaced keyring location.
    #[must_use]
    pub fn new(service: impl Into<String>, account: impl Into<String>) -> Self {
        Self {
            service: service.into(),
            account: account.into(),
        }
    }

    /// Persists the vault key in the platform credential store.
    ///
    /// # Errors
    /// Returns [`SecurityError::Keyring`] if the platform rejects the write.
    pub fn store(&self, key: &VaultKey) -> Result<(), SecurityError> {
        keyring::Entry::new(&self.service, &self.account)?.set_secret(key.secret())?;
        Ok(())
    }

    /// Loads and validates a vault key from the platform credential store.
    ///
    /// # Errors
    /// Returns a keyring error or an invalid-length error without producing a key.
    pub fn load(&self) -> Result<VaultKey, SecurityError> {
        let secret =
            Zeroizing::new(keyring::Entry::new(&self.service, &self.account)?.get_secret()?);
        VaultKey::from_secret(secret.as_ref())
    }

    /// Deletes the credential for this vault.
    ///
    /// # Errors
    /// Returns [`SecurityError::Keyring`] when the credential cannot be deleted.
    pub fn delete(&self) -> Result<(), SecurityError> {
        keyring::Entry::new(&self.service, &self.account)?.delete_credential()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn context(revision: u64) -> RecordContext<'static> {
        RecordContext {
            table: "connection_profiles",
            record_id: "07e60c68-2a80-44ac-bc44-b93ff2d27db1",
            schema_version: 1,
            revision,
            key_version: 1,
        }
    }

    #[test]
    fn seal_record_should_round_trip_authenticated_plaintext() {
        let key = VaultKey::generate().expect("test CSPRNG must be available");
        let sealed = seal_record(&key, context(1), b"host=example.test")
            .expect("test encryption must succeed");
        let opened = open_record(&key, context(1), &sealed).expect("record must authenticate");
        assert_eq!(opened.as_slice(), b"host=example.test");
    }

    #[test]
    fn open_record_should_fail_when_revision_aad_changes() {
        let key = VaultKey::generate().expect("test CSPRNG must be available");
        let sealed =
            seal_record(&key, context(1), b"secret").expect("test encryption must succeed");
        assert!(matches!(
            open_record(&key, context(2), &sealed),
            Err(SecurityError::Authentication)
        ));
    }

    #[test]
    fn open_record_should_fail_when_ciphertext_is_modified() {
        let key = VaultKey::generate().expect("test CSPRNG must be available");
        let mut sealed =
            seal_record(&key, context(1), b"secret").expect("test encryption must succeed");
        sealed.ciphertext[0] ^= 0x80;
        assert!(matches!(
            open_record(&key, context(1), &sealed),
            Err(SecurityError::Authentication)
        ));
    }

    #[test]
    fn seal_record_should_generate_unique_nonces() {
        let key = VaultKey::generate().expect("test CSPRNG must be available");
        let first = seal_record(&key, context(1), b"same").expect("test encryption must succeed");
        let second = seal_record(&key, context(1), b"same").expect("test encryption must succeed");
        assert_ne!(first.nonce, second.nonce);
    }
}
