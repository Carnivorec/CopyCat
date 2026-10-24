use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use copycat_proto::frame::MAX_DATA_FRAME;
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::{CryptoProvider, WebPkiSupportedAlgorithms};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer, ServerName, UnixTime};
use rustls::{DigitallySignedStruct, SignatureScheme};

use crate::framed::{FramedRead, FramedWrite};
use crate::tls::RelayCert;
use crate::{Result, TransportError};

pub const LAN_ALPN: &[u8] = b"copycat-lan/1";

pub const DEFAULT_PORT: u16 = 47821;
const SERVER_NAME: &str = "copycat-lan";

pub struct LanConn {
    pub read: FramedRead<quinn::RecvStream>,
    pub write: FramedWrite<quinn::SendStream>,
    pub conn: quinn::Connection,
}

impl LanConn {
    pub fn remote(&self) -> SocketAddr {
        self.conn.remote_address()
    }
}

pub struct LanEndpoint {
    ep: quinn::Endpoint,
}

fn provider() -> Arc<CryptoProvider> {
    Arc::new(rustls::crypto::ring::default_provider())
}

fn transport() -> Arc<quinn::TransportConfig> {
    let mut tc = quinn::TransportConfig::default();

    tc.keep_alive_interval(None);
    tc.max_idle_timeout(Some(quinn::IdleTimeout::try_from(Duration::from_secs(30 * 60)).expect("合法时长")));

    tc.stream_receive_window(quinn::VarInt::from_u32(16 * 1024 * 1024));
    tc.receive_window(quinn::VarInt::from_u32(32 * 1024 * 1024));
    tc.send_window(32 * 1024 * 1024);
    Arc::new(tc)
}

impl LanEndpoint {
    pub fn bind(port: u16, loopback_only: bool) -> Result<Self> {
        let cert = RelayCert::generate(&[SERVER_NAME.to_owned()])?;
        let key = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(cert.key_der.clone()));
        let mut server = rustls::ServerConfig::builder_with_provider(provider())
            .with_protocol_versions(&[&rustls::version::TLS13])
            .map_err(|e| TransportError::Tls(e.to_string()))?
            .with_no_client_auth()
            .with_single_cert(vec![CertificateDer::from(cert.cert_der.clone())], key)
            .map_err(|e| TransportError::Tls(e.to_string()))?;
        server.alpn_protocols = vec![LAN_ALPN.to_vec()];
        server.max_early_data_size = 0;
        let qsc = quinn::crypto::rustls::QuicServerConfig::try_from(server)
            .map_err(|e| TransportError::Tls(e.to_string()))?;
        let mut sc = quinn::ServerConfig::with_crypto(Arc::new(qsc));
        sc.transport_config(transport());

        sc.max_incoming(32);

        let ip = if loopback_only { Ipv4Addr::LOCALHOST } else { Ipv4Addr::UNSPECIFIED };
        let ep = match quinn::Endpoint::server(sc.clone(), SocketAddr::new(IpAddr::V4(ip), port)) {
            Ok(ep) => ep,
            Err(_) if port != 0 => quinn::Endpoint::server(sc, SocketAddr::new(IpAddr::V4(ip), 0))?,
            Err(e) => return Err(e.into()),
        };

        let p = provider();
        let mut client = rustls::ClientConfig::builder_with_provider(p.clone())
            .with_protocol_versions(&[&rustls::version::TLS13])
            .map_err(|e| TransportError::Tls(e.to_string()))?
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(AnyCert(p.signature_verification_algorithms)))
            .with_no_client_auth();
        client.alpn_protocols = vec![LAN_ALPN.to_vec()];
        let qcc = quinn::crypto::rustls::QuicClientConfig::try_from(client)
            .map_err(|e| TransportError::Tls(e.to_string()))?;
        let mut cc = quinn::ClientConfig::new(Arc::new(qcc));
        cc.transport_config(transport());
        let mut ep = ep;
        ep.set_default_client_config(cc);
        Ok(Self { ep })
    }

    pub fn port(&self) -> u16 {
        self.ep.local_addr().map(|a| a.port()).unwrap_or(0)
    }

    pub async fn accept(&self) -> Option<Result<LanConn>> {
        let incoming = self.ep.accept().await?;
        Some(
            async {
                let conn = tokio::time::timeout(Duration::from_secs(5), incoming)
                    .await
                    .map_err(|_| TransportError::Timeout)?
                    .map_err(|e| TransportError::Connect(e.to_string()))?;
                let (send, recv) = tokio::time::timeout(Duration::from_secs(5), conn.accept_bi())
                    .await
                    .map_err(|_| TransportError::Timeout)?
                    .map_err(|e| TransportError::Connect(e.to_string()))?;
                Ok(LanConn {
                    read: FramedRead::new(recv, MAX_DATA_FRAME),
                    write: FramedWrite::new(send, MAX_DATA_FRAME),
                    conn,
                })
            }
            .await,
        )
    }

    pub async fn connect(&self, addr: SocketAddr, timeout: Duration) -> Result<LanConn> {
        let connecting = self.ep.connect(addr, SERVER_NAME).map_err(|e| TransportError::Connect(e.to_string()))?;
        let conn = tokio::time::timeout(timeout, connecting)
            .await
            .map_err(|_| TransportError::Timeout)?
            .map_err(|e| TransportError::Connect(e.to_string()))?;
        let (send, recv) = conn.open_bi().await.map_err(|e| TransportError::Connect(e.to_string()))?;
        Ok(LanConn { read: FramedRead::new(recv, MAX_DATA_FRAME), write: FramedWrite::new(send, MAX_DATA_FRAME), conn })
    }

    pub fn close(&self) {
        self.ep.close(0u32.into(), b"bye");
    }
}

#[derive(Debug)]
struct AnyCert(WebPkiSupportedAlgorithms);

impl ServerCertVerifier for AnyCert {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        _m: &[u8],
        _c: &CertificateDer<'_>,
        _d: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        Err(rustls::Error::PeerIncompatible(rustls::PeerIncompatible::Tls12NotOffered))
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(message, cert, dss, &self.0)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.0.supported_schemes()
    }
}

pub fn is_lan_ip(ip: Ipv4Addr, allow_loopback: bool) -> bool {
    ip.is_private() || (allow_loopback && ip.is_loopback())
}

pub fn local_lan_ips() -> Vec<Ipv4Addr> {
    crate::discovery::lan_ifaces().into_iter().map(|i| i.ip).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use copycat_proto::relay::Ping;

    #[test]
    fn lan_ip_ranges() {
        assert!(is_lan_ip("192.168.1.5".parse().unwrap(), false));
        assert!(is_lan_ip("10.1.2.3".parse().unwrap(), false));
        assert!(is_lan_ip("172.20.0.1".parse().unwrap(), false));
        assert!(!is_lan_ip("203.0.113.1".parse().unwrap(), false));
        assert!(!is_lan_ip("127.0.0.1".parse().unwrap(), false));
        assert!(is_lan_ip("127.0.0.1".parse().unwrap(), true));
    }

    #[tokio::test]
    async fn connect_and_exchange() {
        let a = LanEndpoint::bind(0, true).unwrap();
        let b = LanEndpoint::bind(0, true).unwrap();
        let addr: SocketAddr = format!("127.0.0.1:{}", b.port()).parse().unwrap();
        let server = tokio::spawn(async move {
            let mut c = b.accept().await.unwrap().unwrap();
            let p: Ping = c.read.recv().await.unwrap().unwrap();
            c.write.send(&Ping { nonce: p.nonce + 1 }).await.unwrap();

            let _ = c.read.recv::<Ping>().await;
        });
        let mut c = a.connect(addr, Duration::from_secs(2)).await.unwrap();
        c.write.send(&Ping { nonce: 41 }).await.unwrap();
        let p: Ping = c.read.recv().await.unwrap().unwrap();
        assert_eq!(p.nonce, 42);
        c.conn.close(0u32.into(), b"");
        server.await.unwrap();
    }
}
