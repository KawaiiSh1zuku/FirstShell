//! M0 platform keyring write/read/delete spike.

#![cfg(target_os = "windows")]

use firstshell_security::{OsKeyring, RecordContext, VaultKey, open_record, seal_record};
use uuid::Uuid;

struct Cleanup(OsKeyring);

impl Drop for Cleanup {
    fn drop(&mut self) {
        let _ = self.0.delete();
    }
}

#[test]
fn windows_keyring_should_round_trip_a_vault_key_without_persistence_leak() {
    let keyring = OsKeyring::new("dev.firstshell.m0-spike", Uuid::new_v4().to_string());
    let cleanup = Cleanup(keyring);
    let expected = VaultKey::generate().expect("Windows CSPRNG must be available");

    cleanup
        .0
        .store(&expected)
        .expect("Credential Manager write must succeed");
    let loaded = cleanup
        .0
        .load()
        .expect("Credential Manager read must succeed");
    let context = RecordContext {
        table: "vault_meta",
        record_id: "keyring-spike",
        schema_version: 1,
        revision: 1,
        key_version: 1,
    };
    let sealed =
        seal_record(&expected, context, b"keyring-proof").expect("test encryption must succeed");
    let opened = open_record(&loaded, context, &sealed).expect("loaded key must authenticate");

    assert_eq!(opened.as_slice(), b"keyring-proof");
}
