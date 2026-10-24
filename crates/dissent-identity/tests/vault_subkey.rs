use dissent_identity::Vault;

const AAD: &[u8] = b"dissent/test.bin";

#[test]
fn subkeys_for_different_purposes_do_not_open_each_others_blobs() {
    let vault = Vault::ephemeral_for_tests();
    let mls = vault.subkey(b"dissent/mls-state/v1");
    let log = vault.subkey(b"dissent/content-log/v1");

    let sealed = mls.seal(AAD, b"secret");
    assert_eq!(mls.open_sealed(AAD, &sealed).unwrap(), b"secret");
    assert!(log.open_sealed(AAD, &sealed).is_err());
    assert!(vault.open_sealed(AAD, &sealed).is_err());
}

#[test]
fn subkey_is_stable_for_the_same_vault_and_purpose() {
    let passphrase_salt = [5u8; 16];
    let a = Vault::from_passphrase("pw", &passphrase_salt).unwrap();
    let b = Vault::from_passphrase("pw", &passphrase_salt).unwrap();
    let sealed = a.subkey(b"p").seal(AAD, b"x");
    assert_eq!(b.subkey(b"p").open_sealed(AAD, &sealed).unwrap(), b"x");
}
