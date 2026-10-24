use std::collections::HashSet;

use dissent_identity::{CommunityKey, DeviceCert, DeviceId, DeviceKey, RootId, RootKey};
use dissent_mls::{Commit, CommitInfo, MlsClient, MlsError, Processed, WelcomeInfo, CIPHERSUITE};
use openmls::prelude::tls_codec::{Deserialize as _, Serialize as _};
use openmls::prelude::*;
use openmls::schedule::{ExternalPsk, PreSharedKeyId, Psk};
use openmls_basic_credential::SignatureKeyPair;
use openmls_rust_crypto::OpenMlsRustCrypto;

const LABEL: &str = "dissent test exporter";

/// A device with its client, id and root, so tests can name who sent what.
struct Device {
    client: MlsClient,
    id: DeviceId,
    root: RootId,
}

fn new_device() -> Device {
    let root = RootKey::generate();
    let device = DeviceKey::generate();
    let cert = root.certify(device.id(), 1);
    let client = MlsClient::new(&device, &cert).unwrap();
    Device {
        client,
        id: device.id(),
        root: root.id(),
    }
}

fn community() -> [u8; 32] {
    CommunityKey::generate().id().0
}

fn allow_commit(_: &CommitInfo) -> bool {
    true
}

fn allow_welcome(_: &WelcomeInfo) -> bool {
    true
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
    bob.client
        .join_from_welcome_with_policy(&first.welcome, allow_welcome)
        .unwrap();

    let second = alice
        .client
        .add_members(group, &[carol.client.key_package().unwrap()])
        .unwrap();
    assert_eq!(
        bob.client
            .process_with_policy(group, &second.commit, allow_commit)
            .unwrap(),
        Processed::Commit
    );
    carol
        .client
        .join_from_welcome_with_policy(&second.welcome, allow_welcome)
        .unwrap();

    (alice, bob, carol)
}

/// Like [`three_member_group`], but also returns the welcome that added Carol, not yet joined.
fn group_with_pending_welcome(group: &[u8; 32]) -> (Device, Device, Device, Commit) {
    let mut alice = new_device();
    let mut bob = new_device();
    let carol = new_device();
    alice.client.create_group(*group).unwrap();
    let first = alice
        .client
        .add_members(group, &[bob.client.key_package().unwrap()])
        .unwrap();
    bob.client
        .join_from_welcome_with_policy(&first.welcome, allow_welcome)
        .unwrap();
    let carol_key_package = carol.client.key_package().unwrap();
    let second = alice
        .client
        .add_members(group, &[carol_key_package])
        .unwrap();
    bob.client
        .process_with_policy(group, &second.commit, allow_commit)
        .unwrap();
    (alice, bob, carol, second)
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
    assert_eq!(
        bob.client
            .join_from_welcome_with_policy(&first.welcome, allow_welcome)
            .unwrap(),
        group
    );
    assert_eq!(bob.client.epoch(&group).unwrap(), 1);

    let second = alice
        .client
        .add_members(&group, &[carol.client.key_package().unwrap()])
        .unwrap();
    assert_eq!(alice.client.epoch(&group).unwrap(), 2);
    assert_eq!(
        bob.client
            .process_with_policy(&group, &second.commit, allow_commit)
            .unwrap(),
        Processed::Commit
    );
    assert_eq!(bob.client.epoch(&group).unwrap(), 2);
    carol
        .client
        .join_from_welcome_with_policy(&second.welcome, allow_welcome)
        .unwrap();
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
                other
                    .process_with_policy(&group, &wire, allow_commit)
                    .unwrap(),
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
        bob.client
            .process_with_policy(&group, &removal, allow_commit)
            .unwrap(),
        Processed::Commit
    );
    assert_eq!(bob.client.epoch(&group).unwrap(), 3);
    // Carol processes the commit that removes her. She learns she was evicted.
    assert_eq!(
        carol
            .client
            .process_with_policy(&group, &removal, allow_commit)
            .unwrap(),
        Processed::Commit
    );

    let wire = alice.client.encrypt(&group, b"after the ban").unwrap();
    assert_eq!(
        bob.client
            .process_with_policy(&group, &wire, allow_commit)
            .unwrap(),
        Processed::Application(b"after the ban".to_vec(), alice.id)
    );
    assert!(carol
        .client
        .process_with_policy(&group, &wire, allow_commit)
        .is_err());
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

#[test]
fn policy_refusing_commit_that_adds_outsider_leaves_state_unchanged() {
    let group = community();
    let mut alice = new_device();
    let mut bob = new_device();
    let mut eve = new_device(); // An outsider, whose root is not in the community.
    alice.client.create_group(group).unwrap();
    let first = alice
        .client
        .add_members(&group, &[bob.client.key_package().unwrap()])
        .unwrap();
    bob.client
        .join_from_welcome_with_policy(&first.welcome, allow_welcome)
        .unwrap();

    // Alice, a malicious member, adds Eve. Bob's policy only accepts the community's known roots.
    let known = [alice.root, bob.root];
    let malicious = alice
        .client
        .add_members(&group, &[eve.client.key_package().unwrap()])
        .unwrap();
    let refused = bob
        .client
        .process_with_policy(&group, &malicious.commit, |info| {
            info.adds.iter().all(|cert| known.contains(&cert.root))
        });
    assert!(matches!(refused, Err(MlsError::Rejected(_))));
    assert_eq!(bob.client.epoch(&group).unwrap(), 1);

    // Eve holds the welcome and sits in epoch 2 with Alice. Bob's traffic stays unreadable to her.
    eve.client
        .join_from_welcome_with_policy(&malicious.welcome, allow_welcome)
        .unwrap();
    let from_bob = bob.client.encrypt(&group, b"members only").unwrap();
    assert!(eve
        .client
        .process_with_policy(&group, &from_bob, allow_commit)
        .is_err());
}

#[test]
fn policy_accepting_commit_merges_it() {
    let group = community();
    let mut alice = new_device();
    let mut bob = new_device();
    let mut carol = new_device();
    alice.client.create_group(group).unwrap();
    let first = alice
        .client
        .add_members(&group, &[bob.client.key_package().unwrap()])
        .unwrap();
    bob.client
        .join_from_welcome_with_policy(&first.welcome, allow_welcome)
        .unwrap();

    let known = [alice.root, bob.root, carol.root];
    let second = alice
        .client
        .add_members(&group, &[carol.client.key_package().unwrap()])
        .unwrap();
    let merged = bob
        .client
        .process_with_policy(&group, &second.commit, |info| {
            info.adds.iter().all(|cert| known.contains(&cert.root))
        });
    assert_eq!(merged.unwrap(), Processed::Commit);
    assert_eq!(bob.client.epoch(&group).unwrap(), 2);
    carol
        .client
        .join_from_welcome_with_policy(&second.welcome, allow_welcome)
        .unwrap();
}

#[test]
fn commit_info_reports_committer_adds_and_removes() {
    let group = community();
    let (mut alice, mut bob, carol) = three_member_group(&group);
    let dave = new_device();

    let added = alice
        .client
        .add_members(&group, &[dave.client.key_package().unwrap()])
        .unwrap();
    let mut seen = None;
    bob.client
        .process_with_policy(&group, &added.commit, |info| {
            seen = Some(info.clone());
            true
        })
        .unwrap();
    let info = seen.expect("policy is called for a commit");
    assert_eq!(info.committer.device, alice.id);
    assert_eq!(
        info.adds.iter().map(|c| c.device).collect::<Vec<_>>(),
        vec![dave.id]
    );
    assert!(info.removes.is_empty());
    assert_eq!(info.epoch_after, 3);

    let removal = alice.client.remove_members(&group, &[carol.id]).unwrap();
    let mut seen = None;
    bob.client
        .process_with_policy(&group, &removal, |info| {
            seen = Some(info.clone());
            true
        })
        .unwrap();
    let info = seen.expect("policy is called for a commit");
    assert_eq!(info.committer.device, alice.id);
    assert!(info.adds.is_empty());
    assert_eq!(
        info.removes.iter().map(|c| c.device).collect::<Vec<_>>(),
        vec![carol.id]
    );
    assert_eq!(info.epoch_after, 4);
}

#[test]
fn inspect_welcome_lists_all_members_and_keeps_key_package() {
    let group = community();
    let (alice, bob, mut carol, second) = group_with_pending_welcome(&group);

    let info = carol.client.inspect_welcome(&second.welcome).unwrap();
    assert_eq!(info.group, group);
    assert_eq!(info.epoch, 2);
    let listed: HashSet<DeviceId> = info.members.iter().map(|c| c.device).collect();
    assert_eq!(listed, HashSet::from([alice.id, bob.id, carol.id]));

    // Inspecting restored the key package, so the same welcome still joins.
    assert_eq!(
        carol
            .client
            .join_from_welcome_with_policy(&second.welcome, allow_welcome)
            .unwrap(),
        group
    );
}

#[test]
fn refused_welcome_leaves_no_group_and_keeps_key_package() {
    let group = community();
    let (_alice, _bob, mut carol, second) = group_with_pending_welcome(&group);

    let refused = carol
        .client
        .join_from_welcome_with_policy(&second.welcome, |_| false);
    assert!(matches!(refused, Err(MlsError::Rejected(_))));
    assert!(!carol.client.has_group(&group));
    assert!(matches!(
        carol.client.epoch(&group),
        Err(MlsError::UnknownGroup)
    ));

    assert_eq!(
        carol
            .client
            .join_from_welcome_with_policy(&second.welcome, allow_welcome)
            .unwrap(),
        group
    );
}

#[test]
fn replayed_application_message_is_rejected() {
    let group = community();
    let (mut alice, mut bob, _carol) = three_member_group(&group);
    let wire = alice.client.encrypt(&group, b"once").unwrap();

    bob.client
        .process_with_policy(&group, &wire, allow_commit)
        .unwrap();
    assert!(bob
        .client
        .process_with_policy(&group, &wire, allow_commit)
        .is_err());
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
        bob.client
            .join_from_welcome_with_policy(&welcome.to_bytes().unwrap(), allow_welcome),
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
#[allow(deprecated)]
fn deprecated_entry_points_still_join_and_process() {
    let group = community();
    let mut alice = new_device();
    let mut bob = new_device();
    alice.client.create_group(group).unwrap();
    let first = alice
        .client
        .add_members(&group, &[bob.client.key_package().unwrap()])
        .unwrap();
    assert_eq!(bob.client.join_from_welcome(&first.welcome).unwrap(), group);
    let wire = alice.client.encrypt(&group, b"legacy").unwrap();
    assert_eq!(
        bob.client.process(&group, &wire).unwrap(),
        Processed::Application(b"legacy".to_vec(), alice.id)
    );
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
    bob.client
        .process_with_policy(&group, &commit.commit, allow_commit)
        .unwrap();
    carol
        .client
        .process_with_policy(&group, &commit.commit, allow_commit)
        .unwrap();
    dave.client
        .join_from_welcome_with_policy(&commit.welcome, allow_welcome)
        .unwrap();

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

/// The ciphersuite before the switch to the hybrid suite: X25519, AES-128-GCM and SHA-256.
const CLASSICAL_SUITE: Ciphersuite = Ciphersuite::MLS_128_DHKEMX25519_AES128GCM_SHA256_Ed25519;
/// X-Wing (ML-KEM-768 with X25519) public key length: 1184 + 32 bytes.
const XWING_PUBLIC_KEY_LEN: usize = 1216;

/// A KeyPackage for `suite`, built directly with OpenMLS.
fn key_package_for_suite(suite: Ciphersuite, device: &DeviceKey, cert: &DeviceCert) -> Vec<u8> {
    let provider = OpenMlsRustCrypto::default();
    let signer = raw_signer(device);
    signer.store(provider.storage()).unwrap();
    let bundle = KeyPackage::builder()
        .build(suite, &provider, &signer, raw_credential(&signer, cert))
        .unwrap();
    bundle.key_package().tls_serialize_detached().unwrap()
}

fn validated(bytes: &[u8]) -> KeyPackage {
    let provider = OpenMlsRustCrypto::default();
    KeyPackageIn::tls_deserialize_exact(bytes)
        .unwrap()
        .validate(provider.crypto(), ProtocolVersion::Mls10)
        .unwrap()
}

#[test]
fn key_packages_use_hybrid_kem_and_aes256_gcm() {
    let device = new_device();
    let bytes = device.client.key_package().unwrap();

    // KeyPackage starts with the protocol version (2 bytes), then the ciphersuite id (2 bytes).
    assert_eq!(&bytes[2..4], &[0x00, 0x4e]);

    let kp = validated(&bytes);
    assert_eq!(kp.ciphersuite(), CIPHERSUITE);
    assert_eq!(
        kp.ciphersuite().hpke_kem_algorithm(),
        HpkeKemType::XWingKemDraft6
    );
    assert_eq!(HpkeKemType::XWingKemDraft6 as u16, 0x004d);
    assert_eq!(
        kp.ciphersuite().hpke_aead_algorithm(),
        HpkeAeadType::AesGcm256
    );
    // The init key is an X-Wing public key, so the KEM really is hybrid.
    assert_eq!(kp.hpke_init_key().as_slice().len(), XWING_PUBLIC_KEY_LEN);
}

#[test]
fn key_package_with_other_ciphersuite_is_rejected() {
    let group = community();
    let mut alice = new_device();
    alice.client.create_group(group).unwrap();
    let root = RootKey::generate();
    let device = DeviceKey::generate();
    let cert = root.certify(device.id(), 1);
    let classical = key_package_for_suite(CLASSICAL_SUITE, &device, &cert);

    assert!(matches!(
        alice.client.add_members(&group, &[classical]),
        Err(MlsError::WrongCiphersuite {
            expected: 0x004e,
            found: 0x0001
        })
    ));
    assert_eq!(alice.client.epoch(&group).unwrap(), 0);
}

#[test]
fn welcome_with_other_ciphersuite_is_rejected_before_any_key_is_used() {
    let group = community();
    let mut bob = new_device();
    let root = RootKey::generate();

    // A group on the classical suite, with a classical member added. Bob's client refuses it.
    let creator = DeviceKey::generate();
    let creator_cert = root.certify(creator.id(), 1);
    let provider = OpenMlsRustCrypto::default();
    let signer = raw_signer(&creator);
    signer.store(provider.storage()).unwrap();
    let config = MlsGroupCreateConfig::builder()
        .ciphersuite(CLASSICAL_SUITE)
        .use_ratchet_tree_extension(true)
        .build();
    let mut rogue = MlsGroup::new_with_group_id(
        &provider,
        &signer,
        &config,
        GroupId::from_slice(&group),
        raw_credential(&signer, &creator_cert),
    )
    .unwrap();
    let member = DeviceKey::generate();
    let member_cert = root.certify(member.id(), 1);
    let member_kp = validated(&key_package_for_suite(
        CLASSICAL_SUITE,
        &member,
        &member_cert,
    ));
    let (_commit, welcome, _info) = rogue.add_members(&provider, &signer, &[member_kp]).unwrap();
    rogue.merge_pending_commit(&provider).unwrap();

    assert!(matches!(
        bob.client
            .join_from_welcome_with_policy(&welcome.to_bytes().unwrap(), allow_welcome),
        Err(MlsError::WrongCiphersuite {
            expected: 0x004e,
            found: 0x0001
        })
    ));
    assert!(!bob.client.has_group(&group));
}

#[test]
fn snapshot_restores_under_current_suite_and_refuses_other_formats() {
    let group = community();
    let device = DeviceKey::generate();
    let root = RootKey::generate();
    let cert = root.certify(device.id(), 1);
    let mut alice = MlsClient::new(&device, &cert).unwrap();
    alice.create_group(group).unwrap();

    let snapshot = alice.snapshot().unwrap();
    let restored = MlsClient::restore(&device, &cert, &snapshot, &[group]).unwrap();
    assert_eq!(restored.epoch(&group).unwrap(), 0);

    // A snapshot from before the header existed, under the classical suite, has no tag.
    let untagged = &snapshot[6..];
    assert!(matches!(
        MlsClient::restore(&device, &cert, untagged, &[group]),
        Err(MlsError::IncompatibleSnapshot)
    ));
    // A snapshot tagged with another ciphersuite is refused the same way.
    let mut other_suite = snapshot.clone();
    other_suite[4..6].copy_from_slice(&[0x00, 0x01]);
    assert!(matches!(
        MlsClient::restore(&device, &cert, &other_suite, &[group]),
        Err(MlsError::IncompatibleSnapshot)
    ));
}

#[test]
fn key_package_and_welcome_sizes_are_reported_and_bounded() {
    let group = community();
    let mut alice = new_device();
    let bob = new_device();
    alice.client.create_group(group).unwrap();

    let kp = bob.client.key_package().unwrap();
    let commit = alice
        .client
        .add_members(&group, std::slice::from_ref(&kp))
        .unwrap();

    let root = RootKey::generate();
    let device = DeviceKey::generate();
    let cert = root.certify(device.id(), 1);
    let classical_kp = key_package_for_suite(CLASSICAL_SUITE, &device, &cert);

    eprintln!(
        "hybrid key package {} bytes (classical {} bytes), hybrid welcome {} bytes, commit {} bytes",
        kp.len(),
        classical_kp.len(),
        commit.welcome.len(),
        commit.commit.len()
    );
    // Well inside the 4 MiB page and 16 MiB frame caps used by core's RPC.
    assert!(kp.len() < 16 * 1024);
    assert!(commit.welcome.len() < 64 * 1024);
}

/// A group that Alice builds with OpenMLS directly, so she can commit anything. Bob is a normal
/// client, joined at epoch 1.
struct RawGroup {
    root: RootKey,
    provider: OpenMlsRustCrypto,
    signer: SignatureKeyPair,
    group: MlsGroup,
    bob: Device,
    id: [u8; 32],
}

fn raw_group_with_bob() -> RawGroup {
    let id = community();
    let root = RootKey::generate();
    let device = DeviceKey::generate();
    let cert = root.certify(device.id(), 1);
    let provider = OpenMlsRustCrypto::default();
    let signer = raw_signer(&device);
    signer.store(provider.storage()).unwrap();
    let config = MlsGroupCreateConfig::builder()
        .ciphersuite(CIPHERSUITE)
        .use_ratchet_tree_extension(true)
        .build();
    let mut group = MlsGroup::new_with_group_id(
        &provider,
        &signer,
        &config,
        GroupId::from_slice(&id),
        raw_credential(&signer, &cert),
    )
    .unwrap();
    let mut bob = new_device();
    let (_commit, welcome, _info) = group
        .add_members(
            &provider,
            &signer,
            &[validated(&bob.client.key_package().unwrap())],
        )
        .unwrap();
    group.merge_pending_commit(&provider).unwrap();
    bob.client
        .join_from_welcome_with_policy(&welcome.to_bytes().unwrap(), allow_welcome)
        .unwrap();
    assert_eq!(bob.client.epoch(&id).unwrap(), 1);
    RawGroup {
        root,
        provider,
        signer,
        group,
        bob,
        id,
    }
}

/// Alice commits `proposals` by value. She does not merge the commit, so her group stays at epoch 1.
fn raw_commit(raw: &mut RawGroup, proposals: Vec<Proposal>) -> Vec<u8> {
    let bundle = raw
        .group
        .commit_builder()
        .add_proposals(proposals)
        .load_psks(raw.provider.storage())
        .unwrap()
        .build(
            raw.provider.rand(),
            raw.provider.crypto(),
            &raw.signer,
            |_| true,
        )
        .unwrap()
        .stage_commit(&raw.provider)
        .unwrap();
    bundle.commit().to_bytes().unwrap()
}

#[test]
fn commit_with_psk_proposal_is_refused_and_state_is_unchanged() {
    let mut raw = raw_group_with_bob();
    let psk_id = PreSharedKeyId::new(
        CIPHERSUITE,
        raw.provider.rand(),
        Psk::External(ExternalPsk::new(b"outside".to_vec())),
    )
    .unwrap();
    psk_id.store(&raw.provider, &[7u8; 32]).unwrap();
    let commit = raw_commit(
        &mut raw,
        vec![Proposal::PreSharedKey(Box::new(PreSharedKeyProposal::new(
            psk_id,
        )))],
    );

    let refused = raw
        .bob
        .client
        .process_with_policy(&raw.id, &commit, allow_commit);
    assert!(matches!(
        refused,
        Err(MlsError::OpenMls(ref msg)) if msg.contains("PSK could not be found")
    ));
    assert_eq!(raw.bob.client.epoch(&raw.id).unwrap(), 1);
}

#[test]
fn commit_with_group_context_extension_is_refused_and_state_is_unchanged() {
    let mut raw = raw_group_with_bob();
    let extensions = Extensions::single(Extension::ExternalSenders(Vec::new())).unwrap();
    let bundle = raw
        .group
        .commit_builder()
        .propose_group_context_extensions(extensions)
        .unwrap()
        .load_psks(raw.provider.storage())
        .unwrap()
        .build(
            raw.provider.rand(),
            raw.provider.crypto(),
            &raw.signer,
            |_| true,
        )
        .unwrap()
        .stage_commit(&raw.provider)
        .unwrap();
    let commit = bundle.commit().to_bytes().unwrap();

    let refused = raw
        .bob
        .client
        .process_with_policy(&raw.id, &commit, allow_commit);
    assert!(matches!(
        refused,
        Err(MlsError::Rejected(
            "commit carries a proposal type Dissent does not send"
        ))
    ));
    assert_eq!(raw.bob.client.epoch(&raw.id).unwrap(), 1);
}

#[test]
fn external_commit_with_external_init_is_refused_and_state_is_unchanged() {
    let mut raw = raw_group_with_bob();
    // Eve is outside the group. She joins with an external commit, which carries ExternalInit.
    let eve_root = RootKey::generate();
    let eve_device = DeviceKey::generate();
    let eve_cert = eve_root.certify(eve_device.id(), 1);
    let eve_provider = OpenMlsRustCrypto::default();
    let eve_signer = raw_signer(&eve_device);
    eve_signer.store(eve_provider.storage()).unwrap();
    let group_info_bytes = raw
        .group
        .export_group_info(raw.provider.crypto(), &raw.signer, true)
        .unwrap()
        .to_bytes()
        .unwrap();
    let MlsMessageBodyIn::GroupInfo(info) = MlsMessageIn::tls_deserialize_exact(&group_info_bytes)
        .unwrap()
        .extract()
    else {
        panic!("expected a group info message");
    };
    let join_config = MlsGroupJoinConfig::builder()
        .use_ratchet_tree_extension(true)
        .build();
    let (_eve_group, bundle) = MlsGroup::external_commit_builder()
        .with_config(join_config)
        .build_group(&eve_provider, info, raw_credential(&eve_signer, &eve_cert))
        .unwrap()
        .load_psks(eve_provider.storage())
        .unwrap()
        .build(
            eve_provider.rand(),
            eve_provider.crypto(),
            &eve_signer,
            |_| true,
        )
        .unwrap()
        .finalize(&eve_provider)
        .unwrap();
    let commit = bundle.commit().to_bytes().unwrap();

    let refused = raw
        .bob
        .client
        .process_with_policy(&raw.id, &commit, allow_commit);
    assert!(matches!(
        refused,
        Err(MlsError::Rejected("commit from non-member"))
    ));
    assert_eq!(raw.bob.client.epoch(&raw.id).unwrap(), 1);
}

#[test]
fn update_path_that_changes_device_is_refused_and_state_is_unchanged() {
    let mut raw = raw_group_with_bob();
    // Alice replaces her own leaf with a new device, certified by the same root.
    let new_device = DeviceKey::generate();
    let new_cert = raw.root.certify(new_device.id(), 2);
    let new_signer = raw_signer(&new_device);
    new_signer.store(raw.provider.storage()).unwrap();
    let bundle = raw
        .group
        .self_update_with_new_signer(
            &raw.provider,
            &raw.signer,
            NewSignerBundle {
                signer: &new_signer,
                credential_with_key: raw_credential(&new_signer, &new_cert),
            },
            LeafNodeParameters::default(),
        )
        .unwrap();
    let commit = bundle.commit().to_bytes().unwrap();

    let refused = raw
        .bob
        .client
        .process_with_policy(&raw.id, &commit, allow_commit);
    assert!(matches!(
        refused,
        Err(MlsError::Rejected("update path changes device"))
    ));
    assert_eq!(raw.bob.client.epoch(&raw.id).unwrap(), 1);
}
