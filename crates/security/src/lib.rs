//! Vault key lifecycle, OS keyring access, and authenticated record encryption.

use std::fmt;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use argon2::{Algorithm, Argon2, Params, Version};
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
const EXPORT_VERSION: u16 = 1;
const RECOVERY_SLOT_VERSION: u16 = 1;
const EXPORT_SALT_LEN: usize = 16;
const EXPORT_MEMORY_KIB: u32 = 64 * 1024;
const EXPORT_ITERATIONS: u32 = 3;
const EXPORT_PARALLELISM: u32 = 1;
const MAX_EXPORT_BYTES: usize = 512 * 1024 * 1024;

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
    /// The in-memory vault has not been unlocked or was automatically locked.
    #[error("vault is locked")]
    VaultLocked,
    /// The vault session lock was poisoned by a panicking thread.
    #[error("vault session is unavailable")]
    VaultSessionUnavailable,
    /// Export password or recorded KDF parameters are invalid.
    #[error("invalid export key parameters")]
    ExportKey,
    /// Export data exceeds the explicit container boundary.
    #[error("export payload exceeds {MAX_EXPORT_BYTES} bytes")]
    ExportTooLarge,
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

#[derive(Debug)]
struct VaultSessionState {
    key: Option<VaultKey>,
    last_activity: Instant,
}

/// In-memory vault key owner with monotonic idle-time locking.
#[derive(Debug)]
pub struct VaultSession {
    idle_timeout: Duration,
    state: Mutex<VaultSessionState>,
}

impl VaultSession {
    /// Creates a locked session with an explicit idle timeout.
    #[must_use]
    pub fn new(idle_timeout: Duration) -> Self {
        Self {
            idle_timeout,
            state: Mutex::new(VaultSessionState {
                key: None,
                last_activity: Instant::now(),
            }),
        }
    }

    /// Replaces any resident key and starts a fresh idle window.
    ///
    /// # Errors
    /// Returns [`SecurityError::VaultSessionUnavailable`] after lock poisoning.
    pub fn unlock(&self, key: VaultKey) -> Result<(), SecurityError> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| SecurityError::VaultSessionUnavailable)?;
        state.key = Some(key);
        state.last_activity = Instant::now();
        Ok(())
    }

    /// Removes the resident key immediately.
    ///
    /// # Errors
    /// Returns [`SecurityError::VaultSessionUnavailable`] after lock poisoning.
    pub fn lock(&self) -> Result<(), SecurityError> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| SecurityError::VaultSessionUnavailable)?;
        state.key = None;
        Ok(())
    }

    /// Returns whether a key is currently resident.
    ///
    /// # Errors
    /// Returns [`SecurityError::VaultSessionUnavailable`] after lock poisoning.
    pub fn is_unlocked(&self) -> Result<bool, SecurityError> {
        self.state
            .lock()
            .map(|state| state.key.is_some())
            .map_err(|_| SecurityError::VaultSessionUnavailable)
    }

    /// Executes one synchronous operation with the key and refreshes activity.
    ///
    /// # Errors
    /// Returns [`SecurityError::VaultLocked`] when no key is resident, plus any
    /// error returned by the operation.
    pub fn with_key<T>(
        &self,
        operation: impl FnOnce(&VaultKey) -> Result<T, SecurityError>,
    ) -> Result<T, SecurityError> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| SecurityError::VaultSessionUnavailable)?;
        let result = operation(state.key.as_ref().ok_or(SecurityError::VaultLocked)?)?;
        state.last_activity = Instant::now();
        Ok(result)
    }

    /// Locks the session when its monotonic idle deadline has elapsed.
    ///
    /// Returns true only when a resident key was removed.
    ///
    /// # Errors
    /// Returns [`SecurityError::VaultSessionUnavailable`] after lock poisoning.
    pub fn lock_if_idle(&self) -> Result<bool, SecurityError> {
        self.lock_if_idle_at(Instant::now())
    }

    fn lock_if_idle_at(&self, now: Instant) -> Result<bool, SecurityError> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| SecurityError::VaultSessionUnavailable)?;
        if state.key.is_some()
            && now.saturating_duration_since(state.last_activity) >= self.idle_timeout
        {
            state.key = None;
            Ok(true)
        } else {
            Ok(false)
        }
    }
}

/// Versioned Argon2id parameters stored with an encrypted export.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExportKdf {
    /// Memory cost in KiB.
    pub memory_kib: u32,
    /// Iteration count.
    pub iterations: u32,
    /// Lane count.
    pub parallelism: u32,
}

/// Password-encrypted, authenticated backup envelope.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExportEnvelope {
    /// Container version.
    pub version: u16,
    /// Versioned Argon2id parameters.
    pub kdf: ExportKdf,
    /// Random KDF salt.
    pub salt: [u8; EXPORT_SALT_LEN],
    /// Unique `XChaCha20` nonce.
    pub nonce: [u8; NONCE_LEN],
    /// Encrypted backup bytes followed by the authentication tag.
    pub ciphertext: Vec<u8>,
}

/// Password-wrapped vault key stored beside the encrypted database.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecoveryKeySlot {
    /// Slot encoding version.
    pub version: u16,
    /// Versioned Argon2id parameters.
    pub kdf: ExportKdf,
    /// Random KDF salt.
    pub salt: [u8; EXPORT_SALT_LEN],
    /// Unique `XChaCha20` nonce.
    pub nonce: [u8; NONCE_LEN],
    /// Wrapped 32-byte vault key followed by its authentication tag.
    pub wrapped_key: Vec<u8>,
}

/// Wraps a random vault key with a key derived from the mandatory recovery password.
///
/// # Errors
/// Returns an error for a password outside 12 to 1024 bytes, failed randomness,
/// KDF, or authenticated encryption.
pub fn wrap_vault_key(
    recovery_password: &[u8],
    vault_id: &str,
    vault_key: &VaultKey,
) -> Result<RecoveryKeySlot, SecurityError> {
    if !(12..=1024).contains(&recovery_password.len()) || vault_id.is_empty() {
        return Err(SecurityError::ExportKey);
    }
    let kdf = default_password_kdf();
    let mut salt = [0_u8; EXPORT_SALT_LEN];
    let mut nonce = [0_u8; NONCE_LEN];
    getrandom::fill(&mut salt).map_err(|error| SecurityError::Random(error.to_string()))?;
    getrandom::fill(&mut nonce).map_err(|error| SecurityError::Random(error.to_string()))?;
    let key = derive_export_key(recovery_password, &salt, kdf)?;
    let cipher = XChaCha20Poly1305::new_from_slice(key.as_ref())
        .map_err(|_| SecurityError::CipherInitialization)?;
    let wrapped_key = cipher
        .encrypt(
            XNonce::from_slice(&nonce),
            Payload {
                msg: vault_key.secret(),
                aad: &recovery_slot_aad(vault_id, kdf, &salt),
            },
        )
        .map_err(|_| SecurityError::Authentication)?;
    Ok(RecoveryKeySlot {
        version: RECOVERY_SLOT_VERSION,
        kdf,
        salt,
        nonce,
        wrapped_key,
    })
}

/// Recovers a vault key from a password slot without exposing partial key bytes.
///
/// # Errors
/// Returns an error for unsupported metadata, unsafe KDF parameters, a wrong
/// password, a different vault identifier, or modified wrapped key bytes.
pub fn unwrap_vault_key(
    recovery_password: &[u8],
    vault_id: &str,
    slot: &RecoveryKeySlot,
) -> Result<VaultKey, SecurityError> {
    if !(12..=1024).contains(&recovery_password.len())
        || vault_id.is_empty()
        || slot.version != RECOVERY_SLOT_VERSION
        || !valid_export_kdf(slot.kdf)
        || slot.wrapped_key.len() != VAULT_KEY_LEN + 16
    {
        return Err(SecurityError::ExportKey);
    }
    let key = derive_export_key(recovery_password, &slot.salt, slot.kdf)?;
    let cipher = XChaCha20Poly1305::new_from_slice(key.as_ref())
        .map_err(|_| SecurityError::CipherInitialization)?;
    let unwrapped = Zeroizing::new(
        cipher
            .decrypt(
                XNonce::from_slice(&slot.nonce),
                Payload {
                    msg: &slot.wrapped_key,
                    aad: &recovery_slot_aad(vault_id, slot.kdf, &slot.salt),
                },
            )
            .map_err(|_| SecurityError::Authentication)?,
    );
    VaultKey::from_secret(unwrapped.as_slice())
}

/// Encrypts a consistent database snapshot with an independent password key.
///
/// # Errors
/// Returns an error for an empty/oversized password, oversized input, failed
/// randomness, KDF, or authenticated encryption.
pub fn seal_export(password: &[u8], plaintext: &[u8]) -> Result<ExportEnvelope, SecurityError> {
    validate_export_input(password, plaintext.len())?;
    let kdf = default_password_kdf();
    let mut salt = [0_u8; EXPORT_SALT_LEN];
    let mut nonce = [0_u8; NONCE_LEN];
    getrandom::fill(&mut salt).map_err(|error| SecurityError::Random(error.to_string()))?;
    getrandom::fill(&mut nonce).map_err(|error| SecurityError::Random(error.to_string()))?;
    let key = derive_export_key(password, &salt, kdf)?;
    let cipher = XChaCha20Poly1305::new_from_slice(key.as_ref())
        .map_err(|_| SecurityError::CipherInitialization)?;
    let ciphertext = cipher
        .encrypt(
            XNonce::from_slice(&nonce),
            Payload {
                msg: plaintext,
                aad: &export_aad(kdf, &salt),
            },
        )
        .map_err(|_| SecurityError::Authentication)?;
    Ok(ExportEnvelope {
        version: EXPORT_VERSION,
        kdf,
        salt,
        nonce,
        ciphertext,
    })
}

/// Decrypts and authenticates a backup without exposing partial plaintext.
///
/// # Errors
/// Returns an error for unsupported metadata, unsafe KDF parameters, oversized
/// data, a wrong password, or modified ciphertext.
pub fn open_export(
    password: &[u8],
    envelope: &ExportEnvelope,
) -> Result<Zeroizing<Vec<u8>>, SecurityError> {
    validate_export_input(password, envelope.ciphertext.len())?;
    if envelope.version != EXPORT_VERSION || !valid_export_kdf(envelope.kdf) {
        return Err(SecurityError::ExportKey);
    }
    let key = derive_export_key(password, &envelope.salt, envelope.kdf)?;
    let cipher = XChaCha20Poly1305::new_from_slice(key.as_ref())
        .map_err(|_| SecurityError::CipherInitialization)?;
    let plaintext = cipher
        .decrypt(
            XNonce::from_slice(&envelope.nonce),
            Payload {
                msg: &envelope.ciphertext,
                aad: &export_aad(envelope.kdf, &envelope.salt),
            },
        )
        .map_err(|_| SecurityError::Authentication)?;
    Ok(Zeroizing::new(plaintext))
}

fn validate_export_input(password: &[u8], size: usize) -> Result<(), SecurityError> {
    if password.is_empty() || password.len() > 1024 {
        return Err(SecurityError::ExportKey);
    }
    if size > MAX_EXPORT_BYTES {
        return Err(SecurityError::ExportTooLarge);
    }
    Ok(())
}

fn valid_export_kdf(kdf: ExportKdf) -> bool {
    (16 * 1024..=256 * 1024).contains(&kdf.memory_kib)
        && (1..=10).contains(&kdf.iterations)
        && (1..=8).contains(&kdf.parallelism)
}

const fn default_password_kdf() -> ExportKdf {
    ExportKdf {
        memory_kib: EXPORT_MEMORY_KIB,
        iterations: EXPORT_ITERATIONS,
        parallelism: EXPORT_PARALLELISM,
    }
}

fn derive_export_key(
    password: &[u8],
    salt: &[u8; EXPORT_SALT_LEN],
    kdf: ExportKdf,
) -> Result<Zeroizing<[u8; VAULT_KEY_LEN]>, SecurityError> {
    if !valid_export_kdf(kdf) {
        return Err(SecurityError::ExportKey);
    }
    let params = Params::new(
        kdf.memory_kib,
        kdf.iterations,
        kdf.parallelism,
        Some(VAULT_KEY_LEN),
    )
    .map_err(|_| SecurityError::ExportKey)?;
    let mut key = Zeroizing::new([0_u8; VAULT_KEY_LEN]);
    Argon2::new(Algorithm::Argon2id, Version::V0x13, params)
        .hash_password_into(password, salt, key.as_mut())
        .map_err(|_| SecurityError::ExportKey)?;
    Ok(key)
}

fn export_aad(kdf: ExportKdf, salt: &[u8; EXPORT_SALT_LEN]) -> Vec<u8> {
    let mut aad = Vec::with_capacity(48);
    aad.extend_from_slice(b"firstshell.export.v1\0");
    aad.extend_from_slice(&kdf.memory_kib.to_be_bytes());
    aad.extend_from_slice(&kdf.iterations.to_be_bytes());
    aad.extend_from_slice(&kdf.parallelism.to_be_bytes());
    aad.extend_from_slice(salt);
    aad
}

fn recovery_slot_aad(vault_id: &str, kdf: ExportKdf, salt: &[u8; EXPORT_SALT_LEN]) -> Vec<u8> {
    let mut aad = Vec::with_capacity(64 + vault_id.len());
    aad.extend_from_slice(b"firstshell.vault-recovery.v1\0");
    append_field(&mut aad, vault_id.as_bytes());
    aad.extend_from_slice(&kdf.memory_kib.to_be_bytes());
    aad.extend_from_slice(&kdf.iterations.to_be_bytes());
    aad.extend_from_slice(&kdf.parallelism.to_be_bytes());
    aad.extend_from_slice(salt);
    aad
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

    #[test]
    fn vault_session_should_lock_after_monotonic_idle_timeout() {
        let session = VaultSession::new(Duration::from_secs(30));
        session
            .unlock(VaultKey::generate().expect("test key must generate"))
            .expect("session must unlock");
        let last_activity = session
            .state
            .lock()
            .expect("test mutex must remain available")
            .last_activity;
        assert!(
            session
                .lock_if_idle_at(last_activity + Duration::from_secs(31))
                .expect("idle lock must run")
        );
        assert!(!session.is_unlocked().expect("state must be readable"));
    }

    #[test]
    fn encrypted_export_should_reject_wrong_password_and_tampering() {
        let envelope = seal_export(b"correct horse battery staple", b"sqlite snapshot")
            .expect("test export must encrypt");
        assert_eq!(
            open_export(b"correct horse battery staple", &envelope)
                .expect("correct password must decrypt")
                .as_slice(),
            b"sqlite snapshot"
        );
        assert!(matches!(
            open_export(b"wrong password", &envelope),
            Err(SecurityError::Authentication)
        ));

        let mut modified = envelope;
        modified.ciphertext[0] ^= 0x40;
        assert!(matches!(
            open_export(b"correct horse battery staple", &modified),
            Err(SecurityError::Authentication)
        ));
    }

    #[test]
    fn recovery_slot_should_restore_the_same_vault_key_after_keyring_loss() {
        let original = VaultKey::generate().expect("test key must generate");
        let context = RecordContext {
            table: "folders",
            record_id: "folder-1",
            schema_version: 1,
            revision: 1,
            key_version: 1,
        };
        let sealed = seal_record(&original, context, b"recoverable").expect("record must encrypt");
        let slot = wrap_vault_key(b"mandatory recovery password", "vault-1", &original)
            .expect("key must wrap");
        drop(original);

        let recovered = unwrap_vault_key(b"mandatory recovery password", "vault-1", &slot)
            .expect("recovery password must unwrap key");
        assert_eq!(
            open_record(&recovered, context, &sealed)
                .expect("recovered key must decrypt prior record")
                .as_slice(),
            b"recoverable"
        );
        assert!(matches!(
            unwrap_vault_key(b"incorrect recovery password", "vault-1", &slot),
            Err(SecurityError::Authentication)
        ));
    }
}
