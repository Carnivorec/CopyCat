use std::sync::Arc;
use std::time::Duration;

use data_encoding::BASE64URL_NOPAD;
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::CryptoProvider;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer, ServerName, UnixTime};
use rustls::{DigitallySignedStruct, SignatureScheme};
use zeroize::Zeroizing;

use crate::{Result, TransportError};

pub const ALPN: &[u8] = b"copycat/1";

pub const PINNED_SERVER_NAME: &str = "copycat-relay";

fn provider() -> Arc<CryptoProvider> {
    Arc::new(rustls::crypto::ring::default_provider())
}

#[derive(Clone)]
pub struct RelayCert {
    pub cert_der: Vec<u8>,
    pub key_der: Vec<u8>,
}

impl core::fmt::Debug for RelayCert {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str("RelayCert(..)")
    }
}

impl RelayCert {
    pub fn generate(names: &[String]) -> Result<Self> {
        let ck = rcgen::generate_simple_self_signed(names.to_vec()).map_err(|e| TransportError::Tls(e.to_string()))?;
        Ok(Self { cert_der: ck.cert.der().to_vec(), key_der: ck.signing_key.serialize_der() })
    }
}

const RELAY_KEY_PREFIX: &str = "ccr1-";

#[derive(Clone)]
pub struct RelayKey {
    seed: Zeroizing<[u8; 32]>,
    group: [u8; 32],
}

impl core::fmt::Debug for RelayKey {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "RelayKey(pub={})", hex(&self.public()))
    }
}

impl RelayKey {
    pub fn generate(group: [u8; 32]) -> Result<Self> {
        let seed = copycat_crypto::random::bytes::<32>().map_err(|e| TransportError::Tls(e.to_string()))?;
        Ok(Self { seed: Zeroizing::new(seed), group })
    }

    pub fn encode(&self) -> String {
        let mut raw = Zeroizing::new([0u8; 64]);
        raw[..32].copy_from_slice(&self.seed[..]);
        raw[32..].copy_from_slice(&self.group);
        format!("{RELAY_KEY_PREFIX}{}", BASE64URL_NOPAD.encode(&raw[..]))
    }

    pub fn parse(s: &str) -> Result<Self> {
        let bad = || TransportError::Tls("中转密钥格式错误（应为 ccr1- 开头的一行文本）".into());
        let body = s.trim().trim_matches(['"', '\'']).strip_prefix(RELAY_KEY_PREFIX).ok_or_else(bad)?;
        let raw = Zeroizing::new(BASE64URL_NOPAD.decode(body.as_bytes()).map_err(|_| bad())?);
        if raw.len() != 64 {
            return Err(bad());
        }
        let mut seed = Zeroizing::new([0u8; 32]);
        seed.copy_from_slice(&raw[..32]);
        Ok(Self { seed, group: raw[32..].try_into().expect("长度已检查") })
    }

    pub fn group(&self) -> [u8; 32] {
        self.group
    }

    fn pkcs8(&self) -> Zeroizing<Vec<u8>> {
        const PREFIX: [u8; 16] =
            [0x30, 0x2e, 0x02, 0x01, 0x00, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, 0x04, 0x22, 0x04, 0x20];
        let mut v = Zeroizing::new(Vec::with_capacity(48));
        v.extend_from_slice(&PREFIX);
        v.extend_from_slice(&self.seed[..]);
        v
    }

    pub fn public(&self) -> [u8; 32] {
        use ring::signature::KeyPair;
        let kp = ring::signature::Ed25519KeyPair::from_seed_unchecked(&self.seed[..]).expect("32 字节种子总是有效");
        kp.public_key().as_ref().try_into().expect("Ed25519 公钥 32 字节")
    }

    pub fn cert(&self) -> Result<RelayCert> {
        let der = self.pkcs8();
        let kp = rcgen::KeyPair::from_pkcs8_der_and_sign_algo(
            &PrivatePkcs8KeyDer::from(der.as_slice()),
            &rcgen::PKCS_ED25519,
        )
        .map_err(|e| TransportError::Tls(e.to_string()))?;
        let params = rcgen::CertificateParams::new(vec![PINNED_SERVER_NAME.to_owned()])
            .map_err(|e| TransportError::Tls(e.to_string()))?;
        let cert = params.self_signed(&kp).map_err(|e| TransportError::Tls(e.to_string()))?;
        Ok(RelayCert { cert_der: cert.der().to_vec(), key_der: der.to_vec() })
    }
}

pub fn hex(b: &[u8]) -> String {
    b.iter().map(|b| format!("{b:02x}")).collect()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PathPref {
    Auto,
    QuicOnly,
    TcpOnly,
}

#[derive(Clone, Debug)]
pub struct Tuning {
    pub keep_alive: Option<Duration>,
    pub idle_timeout: Duration,

    pub quic_connect_timeout: Duration,
    pub path: PathPref,
}

impl Default for Tuning {
    fn default() -> Self {
        Self {
            keep_alive: Some(Duration::from_secs(15)),
            idle_timeout: Duration::from_secs(60),
            quic_connect_timeout: Duration::from_secs(4),
            path: PathPref::Auto,
        }
    }
}

impl Tuning {
    pub fn mobile() -> Self {
        Self { keep_alive: Some(Duration::from_secs(25)), idle_timeout: Duration::from_secs(600), ..Self::default() }
    }

    pub fn server() -> Self {
        Self { keep_alive: None, idle_timeout: Duration::from_secs(600), ..Self::default() }
    }
}

fn transport_config(t: &Tuning) -> Result<Arc<quinn::TransportConfig>> {
    let mut tc = quinn::TransportConfig::default();
    tc.keep_alive_interval(t.keep_alive);
    tc.max_idle_timeout(Some(
        quinn::IdleTimeout::try_from(t.idle_timeout).map_err(|e| TransportError::Tls(e.to_string()))?,
    ));
    Ok(Arc::new(tc))
}

pub fn server_tls_config(cert: &RelayCert) -> Result<Arc<rustls::ServerConfig>> {
    let key = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(cert.key_der.clone()));
    let mut tls = rustls::ServerConfig::builder_with_provider(provider())
        .with_protocol_versions(&[&rustls::version::TLS13])
        .map_err(|e| TransportError::Tls(e.to_string()))?
        .with_no_client_auth()
        .with_single_cert(vec![CertificateDer::from(cert.cert_der.clone())], key)
        .map_err(|e| TransportError::Tls(e.to_string()))?;
    tls.alpn_protocols = vec![ALPN.to_vec()];

    tls.max_early_data_size = 0;
    Ok(Arc::new(tls))
}

pub fn client_tls_config(relay_key: [u8; 32]) -> Result<Arc<rustls::ClientConfig>> {
    let p = provider();
    let verifier = Arc::new(PinnedKeyVerifier { key: relay_key });
    let mut tls = rustls::ClientConfig::builder_with_provider(p)
        .with_protocol_versions(&[&rustls::version::TLS13])
        .map_err(|e| TransportError::Tls(e.to_string()))?
        .dangerous()
        .with_custom_certificate_verifier(verifier)
        .with_no_client_auth();
    tls.alpn_protocols = vec![ALPN.to_vec()];
    tls.enable_early_data = false;
    Ok(Arc::new(tls))
}

pub fn server_config(cert: &RelayCert, tuning: &Tuning) -> Result<quinn::ServerConfig> {
    let qsc = quinn::crypto::rustls::QuicServerConfig::try_from(server_tls_config(cert)?)
        .map_err(|e| TransportError::Tls(e.to_string()))?;
    let mut sc = quinn::ServerConfig::with_crypto(Arc::new(qsc));
    sc.transport_config(transport_config(tuning)?);
    Ok(sc)
}

pub fn client_config(relay_key: [u8; 32], tuning: &Tuning) -> Result<quinn::ClientConfig> {
    let qcc = quinn::crypto::rustls::QuicClientConfig::try_from(client_tls_config(relay_key)?)
        .map_err(|e| TransportError::Tls(e.to_string()))?;
    let mut cc = quinn::ClientConfig::new(Arc::new(qcc));
    cc.transport_config(transport_config(tuning)?);
    Ok(cc)
}

#[derive(Debug)]
struct PinnedKeyVerifier {
    key: [u8; 32],
}

impl ServerCertVerifier for PinnedKeyVerifier {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        Err(rustls::Error::PeerIncompatible(rustls::PeerIncompatible::Tls12NotOffered))
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        _cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        if dss.scheme != SignatureScheme::ED25519 {
            return Err(rustls::Error::PeerMisbehaved(
                rustls::PeerMisbehaved::SignedHandshakeWithUnadvertisedSigScheme,
            ));
        }
        ring::signature::UnparsedPublicKey::new(&ring::signature::ED25519, &self.key)
            .verify(message, dss.signature())
            .map(|()| HandshakeSignatureValid::assertion())
            .map_err(|_| rustls::Error::InvalidCertificate(rustls::CertificateError::BadSignature))
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        vec![SignatureScheme::ED25519]
    }
}

pub fn client_endpoint(relay_key: [u8; 32], tuning: &Tuning) -> Result<quinn::Endpoint> {
    let mut ep = quinn::Endpoint::client("0.0.0.0:0".parse().expect("合法地址"))?;
    ep.set_default_client_config(client_config(relay_key, tuning)?);
    Ok(ep)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relay_key_roundtrip() {
        let k = RelayKey::generate([7u8; 32]).unwrap();
        let text = k.encode();
        assert!(text.starts_with("ccr1-"));
        let back = RelayKey::parse(&format!("  \"{text}\"\n")).unwrap();
        assert_eq!(back.public(), k.public());
        assert_eq!(back.group(), [7u8; 32]);
        assert!(RelayKey::parse("ccr1-abc").is_err());
        assert!(RelayKey::parse(&text[1..]).is_err());
    }

    #[test]
    fn relay_cert_uses_the_key() {
        let k = RelayKey::generate([1u8; 32]).unwrap();
        let c = k.cert().unwrap();

        assert!(c.cert_der.windows(32).any(|w| w == k.public()));
        server_tls_config(&c).unwrap();
    }
}
