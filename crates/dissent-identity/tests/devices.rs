use dissent_identity::{DeviceKey, DeviceRevocation, Identity, LinkRequest, RootKey};

#[test]
fn request_grant_then_cert_binds_the_device() {
    let root = RootKey::generate();
    let dev = DeviceKey::generate();
    let req = LinkRequest::new(&dev, "  Laptop  ", 10).unwrap();
    assert_eq!(req.device_name, "Laptop");
    req.verify().unwrap();

    let grant = root.grant(&req, 11).unwrap();
    grant.cert.verify().unwrap();
    assert_eq!(grant.cert.root, root.id());
    assert_eq!(grant.cert.device, dev.id());
    assert_eq!(grant.nonce, req.nonce);
    assert!(grant.root_seed_wrapped.is_none());

    let linked = Identity::from_grant(&req, dev.clone(), grant).unwrap();
    assert!(linked.root.is_none());
    assert_eq!(linked.root_id(), root.id());
    assert_eq!(linked.device_id(), dev.id());
    assert_eq!(linked.device_name, "Laptop");
}

#[test]
fn tampered_request_is_rejected() {
    let root = RootKey::generate();
    let dev = DeviceKey::generate();
    let req = LinkRequest::new(&dev, "Phone", 1).unwrap();

    let mut renamed = req.clone();
    renamed.device_name = "Evil".into();
    assert!(renamed.verify().is_err());
    assert!(root.grant(&renamed, 2).is_err());

    let mut other_nonce = req.clone();
    other_nonce.nonce[0] ^= 1;
    assert!(root.grant(&other_nonce, 2).is_err());

    let mut other_device = req.clone();
    other_device.device = DeviceKey::generate().id();
    assert!(root.grant(&other_device, 2).is_err());
}

#[test]
fn grant_nonce_must_match_pending_request() {
    let root = RootKey::generate();
    let dev = DeviceKey::generate();
    let req = LinkRequest::new(&dev, "Phone", 1).unwrap();
    let other = LinkRequest::new(&dev, "Phone", 2).unwrap();
    let grant = root.grant(&other, 3).unwrap();
    assert!(Identity::from_grant(&req, dev, grant).is_err());
}

#[test]
fn bad_names_are_rejected() {
    let dev = DeviceKey::generate();
    assert!(LinkRequest::new(&dev, "   ", 1).is_err());
    assert!(LinkRequest::new(&dev, &"x".repeat(33), 1).is_err());
    assert!(LinkRequest::new(&dev, &"x".repeat(32), 1).is_ok());
}

#[test]
fn revocation_verifies_and_foreign_root_fails() {
    let a = RootKey::generate();
    let b = RootKey::generate();
    let dev = DeviceKey::generate().id();

    let rev = a.revoke(dev, 5);
    rev.verify().unwrap();
    assert_eq!(rev.root, a.id());

    let forged = DeviceRevocation {
        root: a.id(),
        ..b.revoke(dev, 5)
    };
    assert!(forged.verify().is_err());
}

#[test]
fn link_roundtrips_as_url() {
    let dev = DeviceKey::generate();
    let req = LinkRequest::new(&dev, "Tablet", 9).unwrap();
    let link = req.to_link().unwrap();
    assert!(link.starts_with("dissent://link/"));
    let back = LinkRequest::parse(&link).unwrap();
    assert_eq!(back, req);
    back.verify().unwrap();
    assert!(LinkRequest::parse("dissent://recover/abc").is_err());
}
