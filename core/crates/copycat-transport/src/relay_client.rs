use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use copycat_crypto::{DeviceId, DeviceIdentity, relay_auth};
use copycat_proto::frame::MAX_DATA_FRAME;
use copycat_proto::relay::{self, ClientMessage, ServerMessage, client_message::Body as C, server_message::Body as S};
use rustls::pki_types::ServerName;
use tokio::sync::mpsc;
use tokio::time::{Instant, Interval};
use tracing::debug;

use crate::conn::{BoxRead, BoxWrite, Closer, RawConn};
use crate::framed::{FramedRead, FramedWrite};
use crate::tls::{self, PINNED_SERVER_NAME, PathPref, Tuning};
use crate::{Result, TransportError};

const AUTH_TIMEOUT: Duration = Duration::from_secs(10);
const TCP_CONNECT_TIMEOUT: Duration = Duration::from_secs(8);
const OUTBOX: usize = 256;
const RESOLVE_TIMEOUT: Duration = Duration::from_secs(10);

const WRITE_STALL: Duration = Duration::from_secs(45);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RelayAddr {
    pub addr: String,

    pub key: [u8; 32],
}

pub enum AuthMode {
    Device { identity: Arc<DeviceIdentity>, roster_ops: Vec<Vec<u8>> },

    PairJoin { ticket_id: [u8; 32], relay_auth: [u8; 32] },
}

#[derive(Debug)]
pub struct Session {
    pub online: Vec<DeviceId>,

    pub pairing: Option<(DeviceId, Vec<u8>)>,

    pub quic: bool,
}

#[derive(Debug)]
pub enum RelayEvent {
    Deliver { from: Vec<u8>, payload: Vec<u8> },
    Presence { device: DeviceId, online: bool },
    Mailbox(Vec<relay::MailboxItem>),
    Roster(Vec<Vec<u8>>),
    Error { code: u32, message: String, context: Vec<u8> },
    Pong(u64),
    Closed(String),
}

#[derive(Clone)]
pub struct RelayHandle {
    tx: mpsc::Sender<ClientMessage>,
    closer: Closer,
}

impl RelayHandle {
    pub async fn send(&self, body: C) -> Result<()> {
        if self.closer.is_closed() {
            return Err(TransportError::Closed);
        }
        tokio::select! {
            r = self.tx.send(ClientMessage::new(body)) => r.map_err(|_| TransportError::Closed),
            () = self.closer.closed() => Err(TransportError::Closed),
        }
    }

    pub async fn send_to(&self, to: &[u8], payload: Vec<u8>) -> Result<()> {
        self.send(C::Send(relay::Send { to: to.to_vec(), payload })).await
    }

    pub fn close(&self) {
        self.closer.close();
    }

    pub fn is_closed(&self) -> bool {
        self.closer.is_closed()
    }

    pub fn rtt(&self) -> Duration {
        self.closer.rtt()
    }
}

async fn resolve(addr: &str) -> Result<SocketAddr> {
    tokio::time::timeout(RESOLVE_TIMEOUT, tokio::net::lookup_host(addr))
        .await
        .map_err(|_| TransportError::Connect(format!("解析地址 {addr} 超时")))??
        .find(|a| a.is_ipv4())
        .ok_or_else(|| TransportError::Connect(format!("无法解析地址 {addr}")))
}

async fn connect_quic(
    endpoint: &quinn::Endpoint,
    sock: SocketAddr,
    relay: &RelayAddr,
    tuning: &Tuning,
) -> Result<RawConn> {
    let cfg = tls::client_config(relay.key, tuning)?;
    let connecting =
        endpoint.connect_with(cfg, sock, PINNED_SERVER_NAME).map_err(|e| TransportError::Connect(e.to_string()))?;
    let conn = tokio::time::timeout(tuning.quic_connect_timeout, connecting)
        .await
        .map_err(|_| TransportError::Timeout)?
        .map_err(|e| TransportError::Connect(e.to_string()))?;
    let (send, recv) = conn.open_bi().await.map_err(|e| TransportError::Connect(e.to_string()))?;
    let mut binding = [0u8; relay_auth::EXPORTER_LEN];
    conn.export_keying_material(&mut binding, relay_auth::EXPORTER_LABEL, b"")
        .map_err(|_| TransportError::Protocol("无法导出通道绑定值".into()))?;
    Ok(RawConn { read: Box::new(recv), write: Box::new(send), binding, closer: Closer::Quic(conn), peer: sock })
}

async fn connect_tcp(sock: SocketAddr, relay: &RelayAddr) -> Result<RawConn> {
    let tcp = tokio::time::timeout(TCP_CONNECT_TIMEOUT, tokio::net::TcpStream::connect(sock))
        .await
        .map_err(|_| TransportError::Timeout)??;
    tcp.set_nodelay(true)?;
    let connector = tokio_rustls::TlsConnector::from(tls::client_tls_config(relay.key)?);
    let name = ServerName::try_from(PINNED_SERVER_NAME).map_err(|e| TransportError::Tls(e.to_string()))?;
    let stream = tokio::time::timeout(TCP_CONNECT_TIMEOUT, connector.connect(name, tcp))
        .await
        .map_err(|_| TransportError::Timeout)?
        .map_err(|e| TransportError::Connect(e.to_string()))?;
    let binding = stream
        .get_ref()
        .1
        .export_keying_material([0u8; relay_auth::EXPORTER_LEN], relay_auth::EXPORTER_LABEL, Some(b""))
        .map_err(|_| TransportError::Protocol("无法导出通道绑定值".into()))?;
    let (read, write) = tokio::io::split(stream);
    Ok(RawConn { read: Box::new(read), write: Box::new(write), binding, closer: Closer::stream(), peer: sock })
}

pub async fn connect(
    endpoint: &quinn::Endpoint,
    relay: &RelayAddr,
    tuning: &Tuning,
    auth: AuthMode,
) -> Result<(RelayHandle, mpsc::Receiver<RelayEvent>, Session)> {
    let sock = resolve(&relay.addr).await?;
    let raw = match tuning.path {
        PathPref::QuicOnly => connect_quic(endpoint, sock, relay, tuning).await?,
        PathPref::TcpOnly => connect_tcp(sock, relay).await?,
        PathPref::Auto => match connect_quic(endpoint, sock, relay, tuning).await {
            Ok(raw) => raw,
            Err(e) => {
                debug!(error = %e, "QUIC 不可用，回退到 TCP");
                connect_tcp(sock, relay).await?
            }
        },
    };
    authenticate(raw, relay, tuning, auth).await
}

async fn authenticate(
    raw: RawConn,
    relay: &RelayAddr,
    tuning: &Tuning,
    auth: AuthMode,
) -> Result<(RelayHandle, mpsc::Receiver<RelayEvent>, Session)> {
    let RawConn { read, write, binding, closer, .. } = raw;
    let mut w = FramedWrite::new(write, MAX_DATA_FRAME);
    let mut r = FramedRead::new(read, MAX_DATA_FRAME);

    w.send(&ClientMessage::new(C::Ping(relay::Ping { nonce: 0 }))).await?;

    let result = tokio::time::timeout(AUTH_TIMEOUT, async {
        let challenge = match r.recv::<ServerMessage>().await?.and_then(|m| m.body) {
            Some(S::Challenge(c)) => c,
            _ => return Err(TransportError::Protocol("未收到挑战".into())),
        };
        let first = match &auth {
            AuthMode::Device { identity, roster_ops } => {
                let sig = relay_auth::sign(identity, &challenge.nonce, &relay.key, &binding);
                C::Auth(relay::Auth {
                    device_id: identity.id().0.to_vec(),
                    signature: sig.to_vec(),
                    roster_ops: roster_ops.clone(),
                })
            }
            AuthMode::PairJoin { ticket_id, relay_auth: key } => C::PairJoin(relay::PairJoin {
                ticket_id: ticket_id.to_vec(),
                proof: relay_auth::pair_join_proof(key, &challenge.nonce, &binding).to_vec(),
            }),
        };
        w.send(&ClientMessage::new(first)).await?;

        let quic = closer.is_quic();
        match r.recv::<ServerMessage>().await?.and_then(|m| m.body) {
            Some(S::AuthResult(res)) if res.ok => Ok(Session {
                online: res.online.iter().filter_map(|b| DeviceId::from_slice(b).ok()).collect(),
                pairing: None,
                quic,
            }),
            Some(S::PairAccepted(p)) => Ok(Session {
                online: Vec::new(),
                pairing: Some((
                    DeviceId::from_slice(&p.peer).map_err(|_| TransportError::Protocol("配对对端 ID 无效".into()))?,
                    p.temp_id,
                )),
                quic,
            }),
            Some(S::AuthResult(res)) => Err(TransportError::Rejected(res.reason)),
            Some(S::Error(e)) => Err(TransportError::Rejected(e.message)),
            _ => Err(TransportError::Protocol("认证响应异常".into())),
        }
    })
    .await;
    let session = match result {
        Ok(Ok(s)) => s,
        Ok(Err(e)) => {
            closer.close();
            return Err(e);
        }
        Err(_) => {
            closer.close();
            return Err(TransportError::Timeout);
        }
    };

    let (out_tx, out_rx) = mpsc::channel::<ClientMessage>(OUTBOX);
    let (ev_tx, ev_rx) = mpsc::channel::<RelayEvent>(OUTBOX);

    let stream = !closer.is_quic();
    let ping_every = tuning.keep_alive.filter(|_| stream);
    let dead_after =
        stream.then(|| tuning.keep_alive.map_or(tuning.idle_timeout, |k| (k * 3).min(tuning.idle_timeout)));
    tokio::spawn(write_loop(w, out_rx, closer.clone(), ping_every));
    tokio::spawn(read_loop(r, ev_tx, closer.clone(), dead_after));
    Ok((RelayHandle { tx: out_tx, closer }, ev_rx, session))
}

async fn write_loop(
    mut w: FramedWrite<BoxWrite>,
    mut out_rx: mpsc::Receiver<ClientMessage>,
    wc: Closer,
    ping_every: Option<Duration>,
) {
    let mut ping = ping_every.map(|d| tokio::time::interval_at(Instant::now() + d, d));
    loop {
        let m = tokio::select! {
            m = out_rx.recv() => match m {
                Some(m) => m,
                None => break,
            },
            () = tick(&mut ping) => ClientMessage::new(C::Ping(relay::Ping { nonce: 0 })),
            () = wc.closed() => break,
        };

        let ok = tokio::select! {
            r = tokio::time::timeout(WRITE_STALL, w.send(&m)) => match r {
                Ok(r) => r.is_ok(),
                Err(_) => {
                    debug!("写入中转连接超时，断开");
                    false
                }
            },
            () = wc.closed() => false,
        };
        if !ok {
            break;
        }
    }
    wc.close();
    let _ = tokio::time::timeout(Duration::from_secs(1), w.shutdown()).await;
}

async fn read_loop(
    mut r: FramedRead<BoxRead>,
    ev_tx: mpsc::Sender<RelayEvent>,
    rc: Closer,
    dead_after: Option<Duration>,
) {
    let reason = loop {
        let deadline = dead_after.map(|d| Instant::now() + d);
        let m = tokio::select! {
            m = r.recv::<ServerMessage>() => m,
            () = rc.closed() => break "连接已关闭".to_owned(),
            () = sleep_until(deadline) => break "长时间没有收到服务器的任何消息，连接可能已失效".to_owned(),
        };
        match m {
            Ok(Some(m)) => {
                let ev = match m.body {
                    Some(S::Deliver(d)) => RelayEvent::Deliver { from: d.from, payload: d.payload },
                    Some(S::Presence(p)) => match DeviceId::from_slice(&p.device) {
                        Ok(device) => RelayEvent::Presence { device, online: p.online },
                        Err(_) => continue,
                    },
                    Some(S::MailboxItems(m)) => RelayEvent::Mailbox(m.items),
                    Some(S::RosterState(s)) => RelayEvent::Roster(s.ops),
                    Some(S::Error(e)) => RelayEvent::Error { code: e.code, message: e.message, context: e.context },
                    Some(S::Pong(p)) => RelayEvent::Pong(p.nonce),
                    _ => continue,
                };
                if ev_tx.send(ev).await.is_err() {
                    break "接收端已关闭".to_owned();
                }
            }
            Ok(None) => break "服务器关闭了连接".to_owned(),
            Err(e) => break e.to_string(),
        }
    };
    debug!(%reason, "中转连接结束");
    rc.close();
    let _ = ev_tx.send(RelayEvent::Closed(reason)).await;
}

async fn tick(i: &mut Option<Interval>) {
    match i {
        Some(i) => {
            i.tick().await;
        }
        None => std::future::pending().await,
    }
}

async fn sleep_until(t: Option<Instant>) {
    match t {
        Some(t) => tokio::time::sleep_until(t).await,
        None => std::future::pending().await,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::DuplexStream;

    async fn connect_stream(
        buf: usize,
        tuning: Tuning,
    ) -> (
        RelayHandle,
        mpsc::Receiver<RelayEvent>,
        FramedRead<tokio::io::ReadHalf<DuplexStream>>,
        FramedWrite<tokio::io::WriteHalf<DuplexStream>>,
    ) {
        let (client, server) = tokio::io::duplex(buf);
        let (cr, cw) = tokio::io::split(client);
        let raw = RawConn {
            read: Box::new(cr),
            write: Box::new(cw),
            binding: [0; 32],
            closer: Closer::stream(),
            peer: "127.0.0.1:1".parse().unwrap(),
        };
        let (sr, sw) = tokio::io::split(server);
        let mut sr = FramedRead::new(sr, MAX_DATA_FRAME);
        let mut sw = FramedWrite::new(sw, MAX_DATA_FRAME);
        let server = tokio::spawn(async move {
            let _: ClientMessage = sr.recv().await.unwrap().unwrap();
            sw.send(&ServerMessage::new(S::Challenge(relay::Challenge { nonce: vec![1; 32], relay_id: vec![0; 32] })))
                .await
                .unwrap();
            let _: ClientMessage = sr.recv().await.unwrap().unwrap();
            sw.send(&ServerMessage::new(S::AuthResult(relay::AuthResult {
                ok: true,
                reason: String::new(),
                online: vec![],
            })))
            .await
            .unwrap();
            (sr, sw)
        });
        let relay = RelayAddr { addr: "127.0.0.1:1".into(), key: [0; 32] };
        let identity = Arc::new(DeviceIdentity::generate().unwrap());
        let auth = AuthMode::Device { identity, roster_ops: vec![] };
        let (h, rx, _) = authenticate(raw, &relay, &tuning, auth).await.unwrap();
        let (sr, sw) = server.await.unwrap();
        (h, rx, sr, sw)
    }

    fn no_keepalive() -> Tuning {
        Tuning { keep_alive: None, idle_timeout: Duration::from_secs(600), ..Tuning::default() }
    }

    #[tokio::test]
    async fn close_releases_sender_blocked_on_stalled_stream() {
        let (h, _rx, _sr, _sw) = connect_stream(128, no_keepalive()).await;
        let mut queued = 0;
        loop {
            let send = h.send_to(&[2; 16], vec![0; 8192]);
            match tokio::time::timeout(Duration::from_millis(100), send).await {
                Ok(r) => {
                    r.unwrap();
                    queued += 1;
                }
                Err(_) => break,
            }
        }
        assert!(queued >= OUTBOX, "队列应先被填满（{queued}）");
        h.close();
        assert!(h.is_closed());
        let r = tokio::time::timeout(Duration::from_millis(500), h.send(C::Ping(relay::Ping { nonce: 1 }))).await;
        assert!(matches!(r, Ok(Err(TransportError::Closed))), "关闭后发送仍然卡住");
    }

    #[tokio::test(start_paused = true)]
    async fn stalled_write_closes_connection() {
        let (h, mut rx, _sr, _sw) = connect_stream(128, no_keepalive()).await;
        h.send_to(&[2; 16], vec![0; 8192]).await.unwrap();
        let ev = tokio::time::timeout(WRITE_STALL * 2, rx.recv()).await.expect("写入卡住后没有断开");
        assert!(matches!(ev, Some(RelayEvent::Closed(_))));
        assert!(h.is_closed());
    }

    #[tokio::test(start_paused = true)]
    async fn silent_server_is_detected_over_tcp() {
        let tuning = Tuning { keep_alive: Some(Duration::from_secs(10)), ..Tuning::default() };
        let (h, mut rx, _sr, _sw) = connect_stream(64 * 1024, tuning).await;
        let ev = tokio::time::timeout(Duration::from_secs(35), rx.recv()).await.expect("没有检测到连接失效");
        assert!(matches!(ev, Some(RelayEvent::Closed(_))));
        assert!(h.is_closed());
    }

    #[tokio::test(start_paused = true)]
    async fn keepalive_pings_keep_connection_alive() {
        let tuning = Tuning { keep_alive: Some(Duration::from_secs(10)), ..Tuning::default() };
        let (h, mut rx, mut sr, mut sw) = connect_stream(64 * 1024, tuning).await;
        tokio::spawn(async move {
            while let Ok(Some(m)) = sr.recv::<ClientMessage>().await {
                if let Some(C::Ping(p)) = m.body {
                    let _ = sw.send(&ServerMessage::new(S::Pong(p))).await;
                }
            }
        });
        let deadline = Instant::now() + Duration::from_secs(120);
        while Instant::now() < deadline {
            match tokio::time::timeout_at(deadline, rx.recv()).await {
                Ok(Some(RelayEvent::Pong(_))) => {}
                Ok(other) => panic!("连接意外结束：{other:?}"),
                Err(_) => break,
            }
        }
        assert!(!h.is_closed());
    }
}
