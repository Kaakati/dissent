use dissent_identity::{
    GuardianApproval, GuardianSet, IdentityError, KeyRotation, RootKey, RotationRequest,
};

struct Setup {
    owner: RootKey,
    guardians: Vec<RootKey>,
    set: GuardianSet,
    new_root: RootKey,
    request: RotationRequest,
}

fn setup(n: usize, threshold: u8) -> Setup {
    let owner = RootKey::generate();
    let guardians: Vec<RootKey> = (0..n).map(|_| RootKey::generate()).collect();
    let ids = guardians.iter().map(|g| g.id()).collect();
    let set = GuardianSet::create(&owner, ids, threshold, 1).unwrap();
    let new_root = RootKey::generate();
    let request = RotationRequest::new_for(&new_root, owner.id(), 100);
    Setup { owner, guardians, set, new_root, request }
}

fn approve(s: &Setup, idx: usize) -> GuardianApproval {
    GuardianApproval::sign(&s.guardians[idx], s.request)
}

fn guardian_rotation(s: &Setup, approvals: Vec<GuardianApproval>) -> KeyRotation {
    KeyRotation::by_guardians(&s.new_root, s.set.clone(), s.request, approvals).unwrap()
}

#[test]
fn two_of_three_succeeds() {
    let s = setup(3, 2);
    let rot = guardian_rotation(&s, vec![approve(&s, 0), approve(&s, 2)]);
    rot.verify().unwrap();
}

#[test]
fn one_of_three_fails() {
    let s = setup(3, 2);
    let rot = guardian_rotation(&s, vec![approve(&s, 1)]);
    assert!(matches!(rot.verify(), Err(IdentityError::Recovery(_))));
}

#[test]
fn duplicated_approval_counts_once() {
    let s = setup(3, 2);
    let rot = guardian_rotation(&s, vec![approve(&s, 0), approve(&s, 0)]);
    assert!(rot.verify().is_err());
}

#[test]
fn approval_from_non_guardian_is_ignored() {
    let s = setup(3, 2);
    let stranger = RootKey::generate();
    let rot = guardian_rotation(
        &s,
        vec![approve(&s, 0), GuardianApproval::sign(&stranger, s.request)],
    );
    assert!(rot.verify().is_err());
}

#[test]
fn approval_for_different_request_fails() {
    let s = setup(3, 2);
    let other = RotationRequest { requested_ms: 999, ..s.request };
    let bad = GuardianApproval::sign(&s.guardians[1], other);
    assert!(bad.verify_for(&s.request).is_err());
    let rot = guardian_rotation(&s, vec![approve(&s, 0), bad]);
    assert!(rot.verify().is_err());
}

#[test]
fn tampered_set_fails() {
    let s = setup(3, 2);
    let mut set = s.set.clone();
    set.threshold = 1;
    let rot = KeyRotation::by_guardians(&s.new_root, set, s.request, vec![approve(&s, 0)]).unwrap();
    assert!(matches!(rot.verify(), Err(IdentityError::BadSignature)));
}

#[test]
fn new_root_must_prove_possession() {
    let s = setup(3, 2);
    let mut rot = guardian_rotation(&s, vec![approve(&s, 0), approve(&s, 1)]);
    rot.new_root_sig = s.owner.0.sign(b"not the new root");
    assert!(rot.verify().is_err());
}

#[test]
fn owner_signed_rotation_works() {
    let owner = RootKey::generate();
    let new = RootKey::generate();
    let rot = KeyRotation::by_owner(&owner, &new, 5);
    rot.verify().unwrap();
    assert_eq!(rot.request.old_root, owner.id());
    assert_eq!(rot.request.new_root, new.id());
}

#[test]
fn guardian_set_policy_is_enforced_at_create() {
    let owner = RootKey::generate();
    let g: Vec<_> = (0..3).map(|_| RootKey::generate().id()).collect();
    assert!(GuardianSet::create(&owner, g.clone(), 0, 1).is_err());
    assert!(GuardianSet::create(&owner, g.clone(), 4, 1).is_err());
    let too_many: Vec<_> = (0..17).map(|_| RootKey::generate().id()).collect();
    assert!(GuardianSet::create(&owner, too_many, 1, 1).is_err());
    assert!(GuardianSet::create(&owner, vec![owner.id()], 1, 1).is_err());
    assert!(GuardianSet::create(&owner, g, 3, 1).is_ok());
}

#[test]
fn links_roundtrip() {
    let s = setup(3, 2);
    let req_link = s.request.to_link().unwrap();
    assert!(req_link.starts_with("dissent://recover/"));
    assert_eq!(RotationRequest::parse(&req_link).unwrap(), s.request);

    let a = approve(&s, 0);
    let app_link = a.to_link().unwrap();
    assert!(app_link.starts_with("dissent://approve/"));
    let back = GuardianApproval::parse(&app_link).unwrap();
    assert_eq!(back, a);
    back.verify_for(&s.request).unwrap();

    assert!(RotationRequest::parse(&app_link).is_err());
}
