//! Probe: does one MLS group keep encrypting past a few thousand messages?

use dissent_identity::{DeviceKey, RootKey};
use dissent_mls::MlsClient;

#[test]
fn six_thousand_messages_encrypt_in_one_group() {
    let root = RootKey::generate();
    let device = DeviceKey::generate();
    let cert = root.certify(device.id(), 0);
    let mut client = MlsClient::new(&device, &cert).expect("client");
    let group = [9u8; 32];
    client.create_group(group).expect("group");
    let started = std::time::Instant::now();
    for i in 0..6000 {
        client
            .encrypt(&group, format!("m{i}").as_bytes())
            .expect("encrypt");
        if i % 500 == 0 {
            eprintln!("PROBE-MLS {i} in {:?}", started.elapsed());
        }
    }
    eprintln!("PROBE-MLS done in {:?}", started.elapsed());
}
