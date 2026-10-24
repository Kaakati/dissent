use std::collections::HashSet;

use dissent_identity::{CommunityKey, DeviceCert, DeviceId, DeviceKey, RootKey};
use dissent_mls::{MlsClient, MlsError, Processed, CIPHERSUITE};
use openmls::prelude::tls_codec::{Deserialize as _, Serialize as _};
use openmls::prelude::*;
use openmls_basic_credential::SignatureKeyPair;
use openmls_rust_crypto::OpenMlsRustCrypto;

const LABEL: &str = "dissent test exporter";

/// A device with its client and id, so tests can name who sent what.
struct Device {
    client: MlsClient,
    id: DeviceId,
}

fn new_device() -> Device {
    let root = RootKey::generate();
    let device = DeviceKey::generate();
    let cert = root.certify(device.id(), 1);
    let client = MlsClient::new(&device, &cert).unwrap();
    Device {
        client,
        id: device.id(),
    }
}

fn community() -> [u8; 32] {
    CommunityKey::generate().id().0
}

/// Alice creates a group, adds Bob, then adds Carol. All three are at epoch 2.
fn three_member_group(group: &[u8; 32]) -> (Device, Device, Device) {
    let mut alice = new_device();
    let mut bob = new_device();
    let mut carol = new_device();
    alice.client.create_group(*group).unwrap();

    let first = alice
        .client
        .add_members(group, &[bob.client.key_package().unwrap()])
        .unwrap();
    bob.client.join_from_welcome(&first.welcome).unwrap();

    let second = alice
        .client
        .add_members(group, &[carol.client.key_package().unwrap()])
        .unwrap();
    assert_eq!(
        bob.client.process(group, &second.commit).unwrap(),
        Processed::Commit
    );
    carol.client.join_from_welcome(&second.welcome).unwrap();

    (alice, bob, carol)
}

#[test]
fn three_members_exchange_messages_and_epoch_advances() {
    let group = community();
    let mut alice = new_device();
    let mut bob = new_device();
    let mut carol = new_device();

    alice.client.create_group(group).unwrap();
    assert_eq!(alice.client.epoch(&group).unwrap(), 0);

    let first = alice
        .client
        .add_members(&group, &[bob.client.key_package().unwrap()])
        .unwrap();
    assert_eq!(alice.client.epoch(&group).unwrap(), 1);
    assert_eq!(bob.client.join_from_welcome(&first.welcome).unwrap(), group);
    assert_eq!(bob.client.epoch(&group).unwrap(), 1);

    let second = alice
        .client
        .add_members(&group, &[carol.client.key_package().unwrap()])
        .unwrap();
    assert_eq!(alice.client.epoch(&group).unwrap(), 2);
    assert_eq!(
        bob.client.process(&group, &second.commit).unwrap(),
        Processed::Commit
    );
    assert_eq!(bob.client.epoch(&group).unwrap(), 2);
    carol.client.join_from_welcome(&second.welcome).unwrap();
    assert_eq!(carol.client.epoch(&group).unwrap(), 2);

    let roster: HashSet<DeviceId> = alice
        .client
        .members(&group)
        .unwrap()
        .iter()
        .map(|cert| cert.device)
        .collect();
    assert_eq!(roster, HashSet::from([alice.id, bob.id, carol.id]));

    let senders = [
        (alice.id, "from alice"),
        (bob.id, "from bob"),
        (carol.id, "from carol"),
    ];
    for (sender, text) in senders {
        let (sender_client, others) = match sender {
            id if id == alice.id => (&mut alice.client, [&mut bob.client, &mut carol.client]),
            id if id == bob.id => (&mut bob.client, [&mut alice.client, &mut carol.client]),
            _ => (&mut carol.client, [&mut alice.client, &mut bob.client]),
        };
        let wire = sender_client.encrypt(&group, text.as_bytes()).unwrap();
        for other in others {
            assert_eq!(
                other.process(&group, &wire).unwrap(),
                Processed::Application(text.as_bytes().to_vec(), sender)
            );
        }
    }
}

#[test]
fn removed_member_cannot_read_later_messages() {
    let group = community();
    let (mut alice, mut bob, mut carol) = three_member_group(&group);
    assert_eq!(alice.client.epoch(&group).unwrap(), 2);

    let removal = alice.client.remove_members(&group, &[carol.id]).unwrap();
    assert_eq!(alice.client.epoch(&group).unwrap(), 3);
    assert_eq!(
        bob.client.process(&group, &removal).unwrap(),
        Processed::Commit
    );
    assert_eq!(bob.client.epoch(&group).unwrap(), 3);
    // Carol processes the commit that removes her. She learns she was evicted.
    assert_eq!(
        carol.client.process(&group, &removal).unwrap(),
        Processed::Commit
    );

    let wire = alice.client.encrypt(&group, b"after the ban").unwrap();
    assert_eq!(
        bob.client.process(&group, &wire).unwrap(),
        Processed::Application(b"after the ban".to_vec(), alice.id)
    );
    assert!(carol.client.process(&group, &wire).is_err());
    assert!(carol.client.encrypt(&group, b"still here").is_err());

    let roster: Vec<DeviceId> = alice
        .client
        .members(&group)
        .unwrap()
        .iter()
        .map(|cert| cert.device)
        .collect();
    assert!(!roster.contains(&carol.id));
}

/// Builds a KeyPackage with OpenMLS directly, bypassing Dissent's checks.
fn rogue_key_package(signer_device: &DeviceKey, cert: &DeviceCert) -> Vec<u8> {
    let provider = OpenMlsRustCrypto::default();
    let signer = raw_signer(signer_device);
    signer.store(provider.storage()).unwrap();
    let bundle = KeyPackage::builder()
        .build(
            CIPHERSUITE,
            &provider,
            &signer,
            raw_credential(&signer, cert),
        )
        .unwrap();
    bundle.key_package().tls_serialize_detached().unwrap()
}

fn raw_signer(device: &DeviceKey) -> SignatureKeyPair {
    SignatureKeyPair::from_raw(
        SignatureScheme::ED25519,
        device.0.seed().to_vec(),
        device.id().0.to_vec(),
    )
}

fn raw_credential(signer: &SignatureKeyPair, cert: &DeviceCert) -> CredentialWithKey {
    CredentialWithKey {
        credential: BasicCredential::new(postcard::to_allocvec(cert).unwrap()).into(),
        signature_key: signer.to_public_vec().into(),
    }
}

/// A cert whose claimed root did not sign it: Mallory's root signs, but the cert names Alice's root.
fn forged_cert(mallory_device: &DeviceKey) -> DeviceCert {
    let mallory_root = RootKey::generate();
    let victim_root = RootKey::generate();
    let mut forged = mallory_root.certify(mallory_device.id(), 1);
    forged.root = victim_root.id();
    forged
}

#[test]
fn forged_credential_in_key_package_is_rejected() {
    let group = community();
    let mut alice = new_device();
    alice.client.create_group(group).unwrap();

    let mallory_device = DeviceKey::generate();
    let forged = forged_cert(&mallory_device);
    let key_package = rogue_key_package(&mallory_device, &forged);

    assert!(matches!(
        alice.client.add_members(&group, &[key_package]),
        Err(MlsError::Credential(_))
    ));
    assert_eq!(alice.client.epoch(&group).unwrap(), 0);
    assert!(MlsClient::new(&mallory_device, &forged).is_err());
}

#[test]
fn forged_credential_in_welcome_is_rejected_on_join() {
    let group = community();
    let mut bob = new_device();
    let bob_key_package = bob.client.key_package().unwrap();

    // Mallory's group carries the forged credential, then adds Bob with a welcome.
    let mallory_device = DeviceKey::generate();
    let forged = forged_cert(&mallory_device);
    let provider = OpenMlsRustCrypto::default();
    let signer = raw_signer(&mallory_device);
    signer.store(provider.storage()).unwrap();
    let config = MlsGroupCreateConfig::builder()
        .ciphersuite(CIPHERSUITE)
        .use_ratchet_tree_extension(true)
        .build();
    let mut rogue = MlsGroup::new_with_group_id(
        &provider,
        &signer,
        &config,
        GroupId::from_slice(&group),
        raw_credential(&signer, &forged),
    )
    .unwrap();
    let key_package = KeyPackageIn::tls_deserialize_exact(&bob_key_package)
        .unwrap()
        .validate(provider.crypto(), ProtocolVersion::Mls10)
        .unwrap();
    let (_commit, welcome, _group_info) = rogue
        .add_members(&provider, &signer, &[key_package])
        .unwrap();
    rogue.merge_pending_commit(&provider).unwrap();

    assert!(matches!(
        bob.client.join_from_welcome(&welcome.to_bytes().unwrap()),
        Err(MlsError::Credential(_))
    ));
    assert!(matches!(
        bob.client.epoch(&group),
        Err(MlsError::UnknownGroup)
    ));
}

#[test]
fn credential_must_match_signature_key() {
    let group = community();
    let mut alice = new_device();
    alice.client.create_group(group).unwrap();

    // A valid cert for one device, but the MLS signature key belongs to another device.
    let root = RootKey::generate();
    let certified_device = DeviceKey::generate();
    let cert = root.certify(certified_device.id(), 1);
    let other_device = DeviceKey::generate();
    let key_package = rogue_key_package(&other_device, &cert);

    assert!(matches!(
        alice.client.add_members(&group, &[key_package]),
        Err(MlsError::Credential(_))
    ));
}

#[test]
fn export_secret_agrees_within_an_epoch_and_changes_after_commit() {
    let group = community();
    let (mut alice, mut bob, mut carol) = three_member_group(&group);

    let before = alice.client.export_secret(&group, LABEL, 32).unwrap();
    assert_eq!(before.len(), 32);
    assert_eq!(bob.client.export_secret(&group, LABEL, 32).unwrap(), before);
    assert_eq!(
        carol.client.export_secret(&group, LABEL, 32).unwrap(),
        before
    );

    let mut dave = new_device();
    let commit = alice
        .client
        .add_members(&group, &[dave.client.key_package().unwrap()])
        .unwrap();
    bob.client.process(&group, &commit.commit).unwrap();
    carol.client.process(&group, &commit.commit).unwrap();
    dave.client.join_from_welcome(&commit.welcome).unwrap();

    let after = alice.client.export_secret(&group, LABEL, 32).unwrap();
    assert_ne!(after, before);
    assert_eq!(bob.client.export_secret(&group, LABEL, 32).unwrap(), after);
    assert_eq!(
        carol.client.export_secret(&group, LABEL, 32).unwrap(),
        after
    );
    assert_eq!(dave.client.export_secret(&group, LABEL, 32).unwrap(), after);
}

#[test]
fn client_is_send_and_sync() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<MlsClient>();
}
