use std::sync::Arc;

use dissent_identity::DeviceId;
use iroh::endpoint::Connection;
use noq_proto::{
    crypto::rustls::HandshakeData,
    rustls::{
        NamedGroup,
        crypto::{
            CryptoProvider,
            aws_lc_rs::{self, cipher_suite, kx_group},
        },
    },
};

/// TLS provider that prefers hybrid X25519 + ML-KEM-768 and falls back to classical groups.
///
/// The hybrid group is offered first, so a peer that supports it negotiates it. Classical groups
/// stay in the list, so peers without ML-KEM still connect. Using the same provider on the relay
/// and discovery TLS is harmless: those servers also negotiate a classical group.
///
/// Cipher suites are pinned to the three TLS 1.3 suites with AES-256-GCM first. iroh speaks only
/// TLS 1.3, so the TLS 1.2 suites from the default list are dropped.
pub(crate) fn pq_preferred_provider() -> Arc<CryptoProvider> {
    let mut provider = aws_lc_rs::default_provider();
    provider.cipher_suites = vec![
        cipher_suite::TLS13_AES_256_GCM_SHA384,
        cipher_suite::TLS13_AES_128_GCM_SHA256,
        cipher_suite::TLS13_CHACHA20_POLY1305_SHA256,
    ];
    provider.kx_groups = vec![
        kx_group::X25519MLKEM768,
        kx_group::X25519,
        kx_group::SECP256R1,
        kx_group::SECP384R1,
    ];
    Arc::new(provider)
}

/// Key exchange group negotiated for `conn`. `None` if the handshake data is unavailable.
pub(crate) fn key_exchange(conn: &Connection) -> Option<NamedGroup> {
    let data = conn.handshake_data()?;
    data.downcast::<HandshakeData>().ok()?.negotiated_key_exchange_group
}

/// Whether the negotiated group is the hybrid X25519 + ML-KEM-768. `None` if unknown.
pub(crate) fn is_pq(group: Option<NamedGroup>) -> Option<bool> {
    group.map(|g| g == NamedGroup::X25519MLKEM768)
}

/// Logs the key exchange for one connection at debug level.
pub(crate) fn log_key_exchange(conn: &Connection, peer: DeviceId, side: &'static str) {
    let group = key_exchange(conn);
    tracing::debug!(%peer, side, ?group, pq = ?is_pq(group), "key exchange negotiated");
}

#[cfg(test)]
mod tests {
    use noq_proto::rustls::{
        ClientConfig, ClientConnection, RootCertStore, ServerConfig, ServerConnection,
        crypto::aws_lc_rs::cipher_suite::TLS13_AES_256_GCM_SHA384,
        pki_types::{CertificateDer, PrivatePkcs8KeyDer, PrivateKeyDer, ServerName},
        version::TLS13,
    };

    use super::*;

    /// Drives a TLS 1.3 handshake between two in-memory rustls connections.
    fn handshake(client: &mut ClientConnection, server: &mut ServerConnection) {
        for _ in 0..20 {
            if !client.is_handshaking() && !server.is_handshaking() {
                return;
            }
            let mut to_server = Vec::new();
            while client.wants_write() {
                client.write_tls(&mut to_server).expect("client write");
            }
            server.read_tls(&mut to_server.as_slice()).expect("server read");
            server.process_new_packets().expect("server handshake");

            let mut to_client = Vec::new();
            while server.wants_write() {
                server.write_tls(&mut to_client).expect("server write");
            }
            client.read_tls(&mut to_client.as_slice()).expect("client read");
            client.process_new_packets().expect("client handshake");
        }
        panic!("handshake did not complete");
    }

    /// Runs the production provider through a real handshake, so the policy is checked end to end.
    #[test]
    fn handshake_negotiates_aes256_and_hybrid_group() {
        let provider = pq_preferred_provider();
        let ck = rcgen::generate_simple_self_signed(vec!["localhost".to_string()]).expect("cert");
        let cert = CertificateDer::from(ck.cert.der().to_vec());
        let key = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(ck.signing_key.serialize_der()));

        let mut roots = RootCertStore::empty();
        roots.add(cert.clone()).expect("root");
        let client_cfg = ClientConfig::builder_with_provider(provider.clone())
            .with_protocol_versions(&[&TLS13])
            .expect("client versions")
            .with_root_certificates(roots)
            .with_no_client_auth();
        let server_cfg = ServerConfig::builder_with_provider(provider)
            .with_protocol_versions(&[&TLS13])
            .expect("server versions")
            .with_no_client_auth()
            .with_single_cert(vec![cert], key)
            .expect("server cert");

        let mut client = ClientConnection::new(
            Arc::new(client_cfg),
            ServerName::try_from("localhost").expect("name"),
        )
        .expect("client");
        let mut server = ServerConnection::new(Arc::new(server_cfg)).expect("server");
        handshake(&mut client, &mut server);

        let suite = client.negotiated_cipher_suite().map(|s| s.suite());
        assert_eq!(suite, Some(TLS13_AES_256_GCM_SHA384.suite()));
        let group = client.negotiated_key_exchange_group().map(|g| g.name());
        assert_eq!(group, Some(NamedGroup::X25519MLKEM768));
    }
}
