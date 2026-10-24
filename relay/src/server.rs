use std::collections::{HashMap, VecDeque};
use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use copycat_crypto::{DeviceId, Roster, random, relay_auth};
use copycat_proto::frame::MAX_DATA_FRAME;
use copycat_proto::relay::{
    self, ClientMessage, ServerMessage, client_message::Body as C, error_code, server_message::Body as S,
};
use copycat_transport::conn::{BoxRead, BoxWrite, Closer, RawConn};
use copycat_transport::framed::{FramedRead, FramedWrite};
use copycat_transport::tls::{self, RelayKey, Tuning};
use tokio::net::TcpListener;
use tokio::sync::OwnedSemaphorePermit;
use tokio::sync::{Semaphore, mpsc};
use tokio_rustls::TlsAcceptor;
use tracing::{debug, info, warn};

use crate::persist::DataDir;

const MAX_AUTH_OPS: usize = 512;

#[derive(Clone, Debug)]
pub struct Limits {
    pub max_unauth: usize,
    pub auth_timeout: Duration,

    pub conns_per_ip_per_min: u32,

    pub msg_rate: f64,
    pub msg_burst: f64,
    pub mailbox_per_device: usize,
    pub mailbox_item_max: usize,
    pub mailbox_ttl_max: Duration,
    pub ticket_ttl_max: Duration,
    pub tickets_per_device: usize,
    pub pair_session_ttl: Duration,
    pub tuning: Tuning,

    pub tcp_listen: Option<SocketAddr>,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_unauth: 200,
            auth_timeout: Duration::from_secs(5),
            conns_per_ip_per_min: 30,
            msg_rate: 200.0,
            msg_burst: 400.0,
            mailbox_per_device: 50,
            mailbox_item_max: 320 * 1024,
            mailbox_ttl_max: Duration::from_secs(24 * 3600),
            ticket_ttl_max: Duration::from_secs(300),
            tickets_per_device: 4,
            pair_session_ttl: Duration::from_secs(180),
            tuning: Tuning::server(),
            tcp_listen: None,
        }
    }
}

const CLIENT_QUEUE: usize = 512;
const MAX_MSG_ID: usize = 32;

const WRITE_STALL: Duration = Duration::from_secs(45);

struct Client {
    conn_id: u64,
    tx: mpsc::Sender<ServerMessage>,
    closer: Closer,
}

struct PairClient {
    inviter: DeviceId,
    tx: mpsc::Sender<ServerMessage>,
}

struct Ticket {
    inviter: DeviceId,
    relay_auth: [u8; 32],
    expires: Instant,
}

struct Mail {
    from: DeviceId,
    msg_id: Vec<u8>,
    payload: Vec<u8>,
    expires: Instant,
}

#[derive(Default)]
struct State {
    roster: Option<Roster>,
    online: HashMap<DeviceId, Client>,
    pairing: HashMap<Vec<u8>, PairClient>,
    tickets: HashMap<[u8; 32], Ticket>,
    mailbox: HashMap<DeviceId, VecDeque<Mail>>,
    ip_window: HashMap<IpAddr, (Instant, u32)>,
    next_conn_id: u64,
}

struct Shared {
    state: Mutex<State>,
    limits: Limits,

    public: [u8; 32],
    group: [u8; 32],
    data: Option<DataDir>,
    unauth: Arc<Semaphore>,
}

pub struct RelayServer {
    endpoint: quinn::Endpoint,

    tcp: std::net::TcpListener,
    tls: TlsAcceptor,
    shared: Arc<Shared>,
}

fn msg(body: S) -> ServerMessage {
    ServerMessage::new(body)
}

fn err(code: u32, message: &str, context: &[u8]) -> ServerMessage {
    msg(S::Error(relay::Error { code, message: message.to_owned(), context: context.to_vec() }))
}

async fn reject(closer: &Closer, mut w: FramedWrite<BoxWrite>, last: ServerMessage) {
    let _ = w.send(&last).await;
    let _ = w.shutdown().await;
    let _ = tokio::time::timeout(Duration::from_secs(2), closer.closed()).await;
    closer.close();
}

async fn idle_sleep(d: Option<Duration>) {
    match d {
        Some(d) => tokio::time::sleep(d).await,
        None => std::future::pending().await,
    }
}

fn close_soon(closer: Closer) {
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(500)).await;
        closer.close();
    });
}

impl RelayServer {
    pub fn bind(listen: SocketAddr, data_dir: impl Into<PathBuf>, key: &RelayKey, limits: Limits) -> Result<Self> {
        let dir = DataDir::new(data_dir)?;
        let mut state = State::default();
        if let Some((group, ops)) = dir.load_roster()? {
            if group == key.group() {
                state.roster = Some(Roster::from_ops(&group, ops.iter().map(Vec::as_slice)).context("名册文件无效")?);
            } else {
                warn!("数据目录中的名册属于另一个设备组（中转密钥已更换），将由新设备组的设备重新上传");
            }
        }
        Self::start(listen, key, state, Some(dir), limits)
    }

    pub fn bind_ephemeral(listen: SocketAddr, key: &RelayKey, limits: Limits) -> Result<Self> {
        Self::start(listen, key, State::default(), None, limits)
    }

    fn start(listen: SocketAddr, key: &RelayKey, state: State, data: Option<DataDir>, limits: Limits) -> Result<Self> {
        let cert = key.cert()?;

        let mut attempt = 0;
        let (endpoint, tcp) = loop {
            let endpoint = quinn::Endpoint::server(tls::server_config(&cert, &limits.tuning)?, listen)?;
            let tcp_addr = limits.tcp_listen.unwrap_or(endpoint.local_addr()?);
            match std::net::TcpListener::bind(tcp_addr) {
                Ok(tcp) => break (endpoint, tcp),
                Err(_) if listen.port() == 0 && limits.tcp_listen.is_none() && attempt < 8 => attempt += 1,
                Err(e) => return Err(e).context("无法监听 TCP 端口"),
            }
        };
        tcp.set_nonblocking(true)?;
        let tls = TlsAcceptor::from(tls::server_tls_config(&cert)?);
        let shared = Arc::new(Shared {
            state: Mutex::new(state),
            unauth: Arc::new(Semaphore::new(limits.max_unauth)),
            limits,
            public: key.public(),
            group: key.group(),
            data,
        });
        Ok(Self { endpoint, tcp, tls, shared })
    }

    pub fn local_addr(&self) -> Result<SocketAddr> {
        Ok(self.endpoint.local_addr()?)
    }

    pub fn public_key(&self) -> [u8; 32] {
        self.shared.public
    }

    pub async fn run(self) {
        info!(
            addr = ?self.endpoint.local_addr().ok(),
            key = %tls::hex(&self.shared.public),
            group = %tls::hex(&self.shared.group[..8]),
            "中转服务器已启动"
        );
        let janitor = Arc::downgrade(&self.shared);
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(Duration::from_secs(30));
            loop {
                tick.tick().await;
                let Some(s) = janitor.upgrade() else { break };
                s.sweep();
            }
        });
        match TcpListener::from_std(self.tcp) {
            Ok(listener) => {
                let shared = self.shared.clone();
                let tls = self.tls.clone();
                tokio::spawn(async move {
                    loop {
                        let Ok((stream, peer)) = listener.accept().await else { continue };
                        let (shared, tls) = (shared.clone(), tls.clone());
                        tokio::spawn(async move {
                            if let Err(e) = shared.handle_tcp(stream, peer, tls).await {
                                debug!(error = %e, "TCP 连接结束");
                            }
                        });
                    }
                });
            }
            Err(e) => warn!(error = %e, "TCP 兜底监听启动失败"),
        }
        while let Some(incoming) = self.endpoint.accept().await {
            let shared = self.shared.clone();
            tokio::spawn(async move {
                if let Err(e) = shared.handle_quic(incoming).await {
                    debug!(error = %e, "连接结束");
                }
            });
        }
    }
}

struct Bucket {
    tokens: f64,
    last: Instant,
    rate: f64,
    burst: f64,
}

impl Bucket {
    fn new(rate: f64, burst: f64) -> Self {
        Self { tokens: burst, last: Instant::now(), rate, burst }
    }

    fn take(&mut self) -> bool {
        let now = Instant::now();
        self.tokens = (self.tokens + now.duration_since(self.last).as_secs_f64() * self.rate).min(self.burst);
        self.last = now;
        if self.tokens >= 1.0 {
            self.tokens -= 1.0;
            true
        } else {
            false
        }
    }
}

enum Authed {
    Device(DeviceId),
    Pair { inviter: DeviceId, temp_id: Vec<u8> },
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|p| p.into_inner())
    }

    fn sweep(&self) {
        let now = Instant::now();
        let mut st = self.lock();
        st.tickets.retain(|_, t| t.expires > now);
        for q in st.mailbox.values_mut() {
            q.retain(|m| m.expires > now);
        }
        st.mailbox.retain(|_, q| !q.is_empty());
        st.ip_window.retain(|_, (start, _)| now.duration_since(*start) < Duration::from_secs(60));
    }

    fn admit_ip(&self, ip: IpAddr) -> bool {
        let now = Instant::now();
        let mut st = self.lock();
        let e = st.ip_window.entry(ip).or_insert((now, 0));
        if now.duration_since(e.0) >= Duration::from_secs(60) {
            *e = (now, 0);
        }
        e.1 += 1;
        e.1 <= self.limits.conns_per_ip_per_min
    }

    async fn handle_quic(self: Arc<Self>, incoming: quinn::Incoming) -> Result<()> {
        let peer = incoming.remote_address();
        if !self.admit_ip(peer.ip()) {
            incoming.refuse();
            bail!("IP {} 连接过于频繁", peer.ip());
        }
        let Ok(permit) = self.unauth.clone().try_acquire_owned() else {
            incoming.refuse();
            bail!("未认证连接过多");
        };
        let t = self.limits.auth_timeout;
        let conn = tokio::time::timeout(t, incoming).await??;
        let (send, recv) = tokio::time::timeout(t, conn.accept_bi()).await??;
        let mut binding = [0u8; relay_auth::EXPORTER_LEN];
        conn.export_keying_material(&mut binding, relay_auth::EXPORTER_LABEL, b"")
            .map_err(|_| anyhow::anyhow!("无法导出通道绑定值"))?;
        let raw = RawConn { read: Box::new(recv), write: Box::new(send), binding, closer: Closer::Quic(conn), peer };
        self.serve(raw, permit).await
    }

    async fn handle_tcp(
        self: Arc<Self>,
        stream: tokio::net::TcpStream,
        peer: SocketAddr,
        tls: TlsAcceptor,
    ) -> Result<()> {
        if !self.admit_ip(peer.ip()) {
            bail!("IP {} 连接过于频繁", peer.ip());
        }
        let Ok(permit) = self.unauth.clone().try_acquire_owned() else {
            bail!("未认证连接过多");
        };
        stream.set_nodelay(true)?;
        let tls_stream = tokio::time::timeout(self.limits.auth_timeout, tls.accept(stream)).await??;
        let binding = tls_stream
            .get_ref()
            .1
            .export_keying_material([0u8; relay_auth::EXPORTER_LEN], relay_auth::EXPORTER_LABEL, Some(b""))
            .map_err(|_| anyhow::anyhow!("无法导出通道绑定值"))?;
        let (read, write) = tokio::io::split(tls_stream);
        let raw = RawConn { read: Box::new(read), write: Box::new(write), binding, closer: Closer::stream(), peer };
        self.serve(raw, permit).await
    }

    async fn serve(self: Arc<Self>, raw: RawConn, permit: OwnedSemaphorePermit) -> Result<()> {
        let RawConn { read, write, binding, closer, peer } = raw;
        let ip = peer.ip();
        let t = self.limits.auth_timeout;
        let mut w = FramedWrite::new(write, MAX_DATA_FRAME);
        let mut r = FramedRead::new(read, MAX_DATA_FRAME);

        let nonce = random::bytes::<32>()?;
        w.send(&msg(S::Challenge(relay::Challenge { nonce: nonce.to_vec(), relay_id: self.public.to_vec() }))).await?;

        let first = tokio::time::timeout(t, async {
            loop {
                match r.recv::<ClientMessage>().await?.and_then(|m| m.body) {
                    Some(C::Ping(_)) => continue,
                    other => return anyhow::Ok(other),
                }
            }
        })
        .await;
        let first = match first {
            Ok(Ok(f)) => f,
            Ok(Err(e)) => {
                closer.close();
                return Err(e);
            }
            Err(_) => {
                closer.close();
                bail!("认证超时");
            }
        };

        let (tx, rx) = mpsc::channel::<ServerMessage>(CLIENT_QUEUE);
        let authed = match first {
            Some(C::Auth(a)) => match self.authenticate(&a, &nonce, &binding) {
                Ok(id) => Authed::Device(id),
                Err(reason) => {
                    warn!(%ip, %reason, "认证失败");
                    let m =
                        msg(S::AuthResult(relay::AuthResult {
                            ok: false, reason: "认证失败".into(), online: vec![]
                        }));
                    reject(&closer, w, m).await;
                    bail!(reason);
                }
            },
            Some(C::PairJoin(p)) => match self.join_pairing(&p, &nonce, &binding, tx.clone()) {
                Ok((inviter, temp_id)) => Authed::Pair { inviter, temp_id },
                Err(reason) => {
                    reject(&closer, w, err(error_code::UNAUTHORIZED, "配对票据无效", &[])).await;
                    bail!(reason);
                }
            },
            _ => {
                closer.close();
                bail!("首条消息不是认证请求");
            }
        };
        drop(permit);

        match authed {
            Authed::Device(id) => self.serve_device(id, closer, w, r, tx, rx).await,
            Authed::Pair { inviter, temp_id } => self.serve_pairing(inviter, temp_id, closer, w, r, tx, rx).await,
        }
    }

    fn authenticate(&self, a: &relay::Auth, nonce: &[u8], binding: &[u8]) -> Result<DeviceId, String> {
        let id = DeviceId::from_slice(&a.device_id).map_err(|_| "设备 ID 格式错误".to_owned())?;
        if a.roster_ops.len() > MAX_AUTH_OPS {
            return Err("名册操作过多".into());
        }
        let mut st = self.lock();

        let known = st.roster.as_ref().is_some_and(|r| r.is_member(&id));
        if !known && !a.roster_ops.is_empty() {
            self.merge_roster(&mut st, &a.roster_ops);
        }
        let roster = st.roster.as_ref().ok_or("服务器还没有设备组名册")?;
        let member = roster.member(&id).ok_or("设备不在名册中或已被吊销")?;
        relay_auth::verify(&member.sign_pub, nonce, &self.public, binding, &a.signature).map_err(|e| e.to_string())?;

        Ok(id)
    }

    fn join_pairing(
        &self,
        p: &relay::PairJoin,
        nonce: &[u8],
        binding: &[u8],
        tx: mpsc::Sender<ServerMessage>,
    ) -> Result<(DeviceId, Vec<u8>), String> {
        let ticket_id: [u8; 32] = p.ticket_id.as_slice().try_into().map_err(|_| "票据格式错误")?;
        let mut st = self.lock();
        let t = st.tickets.get(&ticket_id).ok_or("票据不存在")?;
        if t.expires <= Instant::now() {
            st.tickets.remove(&ticket_id);
            return Err("票据已过期".into());
        }
        if !relay_auth::verify_pair_join_proof(&t.relay_auth, nonce, binding, &p.proof) {
            return Err("票据证明无效".into());
        }
        let inviter = t.inviter;
        st.tickets.remove(&ticket_id);
        let inviter_tx = st.online.get(&inviter).ok_or("邀请方不在线")?.tx.clone();
        let mut temp_id = random::bytes::<16>().map_err(|e| e.to_string())?.to_vec();
        temp_id[0] = 0xff;
        st.pairing.insert(temp_id.clone(), PairClient { inviter, tx: tx.clone() });
        let _ = inviter_tx.try_send(msg(S::Presence(relay::Presence { device: temp_id.clone(), online: true })));
        let _ = tx
            .try_send(msg(S::PairAccepted(relay::PairAccepted { peer: inviter.0.to_vec(), temp_id: temp_id.clone() })));
        Ok((inviter, temp_id))
    }

    fn spawn_writer(mut w: FramedWrite<BoxWrite>, mut rx: mpsc::Receiver<ServerMessage>, closer: Closer) {
        tokio::spawn(async move {
            loop {
                let m = tokio::select! {
                    m = rx.recv() => match m {
                        Some(m) => m,
                        None => break,
                    },
                    _ = closer.closed() => break,
                };

                let ok = tokio::select! {
                    r = tokio::time::timeout(WRITE_STALL, w.send(&m)) => matches!(r, Ok(Ok(()))),
                    _ = closer.closed() => false,
                };
                if !ok {
                    break;
                }
            }
            closer.close();
            let _ = tokio::time::timeout(Duration::from_secs(1), w.shutdown()).await;
        });
    }

    fn idle_limit(&self, closer: &Closer) -> Option<Duration> {
        (!closer.is_quic()).then_some(self.limits.tuning.idle_timeout)
    }

    async fn serve_device(
        self: &Arc<Self>,
        me: DeviceId,
        closer: Closer,
        w: FramedWrite<BoxWrite>,
        mut r: FramedRead<BoxRead>,
        tx: mpsc::Sender<ServerMessage>,
        rx: mpsc::Receiver<ServerMessage>,
    ) -> Result<()> {
        let conn_id = {
            let mut st = self.lock();
            st.next_conn_id += 1;
            let conn_id = st.next_conn_id;
            if let Some(old) = st.online.insert(me, Client { conn_id, tx: tx.clone(), closer: closer.clone() }) {
                old.closer.close();
            }
            let online: Vec<Vec<u8>> = st.online.keys().filter(|k| **k != me).map(|k| k.0.to_vec()).collect();
            let _ = tx.try_send(msg(S::AuthResult(relay::AuthResult { ok: true, reason: String::new(), online })));
            if let Some(roster) = &st.roster {
                let _ = tx.try_send(msg(S::RosterState(relay::RosterState { ops: roster.ops_raw() })));
            }
            Self::push_mailbox(&st, me, &tx);
            Self::broadcast_presence(&st, me, true);
            conn_id
        };
        info!(device = %me.short(), "设备上线");
        Self::spawn_writer(w, rx, closer.clone());

        let mut bucket = Bucket::new(self.limits.msg_rate, self.limits.msg_burst);
        let idle = self.idle_limit(&closer);
        let result = loop {
            let m = tokio::select! {
                m = r.recv::<ClientMessage>() => m,
                _ = closer.closed() => break Ok(()),
                () = idle_sleep(idle) => break Err(anyhow::anyhow!("长时间没有收到客户端的消息")),
            };
            let m = match m {
                Ok(Some(m)) => m,
                Ok(None) => break Ok(()),
                Err(e) => break Err(anyhow::anyhow!(e)),
            };
            if !bucket.take() {
                let _ = tx.try_send(err(error_code::RATE_LIMITED, "消息过于频繁", &[]));
                close_soon(closer.clone());
                break Err(anyhow::anyhow!("速率超限"));
            }
            if let Some(body) = m.body {
                self.on_device_message(me, &tx, body);
            }
        };

        let mut st = self.lock();
        if st.online.get(&me).is_some_and(|c| c.conn_id == conn_id) {
            st.online.remove(&me);
            Self::broadcast_presence(&st, me, false);
            info!(device = %me.short(), "设备离线");
        }
        result
    }

    fn broadcast_presence(st: &State, device: DeviceId, online: bool) {
        for (id, c) in &st.online {
            if *id != device {
                let _ = c.tx.try_send(msg(S::Presence(relay::Presence { device: device.0.to_vec(), online })));
            }
        }
    }

    fn push_mailbox(st: &State, me: DeviceId, tx: &mpsc::Sender<ServerMessage>) {
        let now = Instant::now();
        let Some(q) = st.mailbox.get(&me) else { return };
        let items: Vec<relay::MailboxItem> = q
            .iter()
            .filter(|m| m.expires > now)
            .map(|m| relay::MailboxItem {
                from: m.from.0.to_vec(),
                msg_id: m.msg_id.clone(),
                payload: m.payload.clone(),
            })
            .collect();
        if !items.is_empty() {
            let _ = tx.try_send(msg(S::MailboxItems(relay::MailboxItems { items })));
        }
    }

    fn deliver(c: &Client, m: ServerMessage) {
        if c.tx.try_send(m).is_err() {
            c.closer.close();
        }
    }

    fn on_device_message(&self, me: DeviceId, tx: &mpsc::Sender<ServerMessage>, body: C) {
        let mut st = self.lock();
        match body {
            C::Send(s) => {
                let deliver = msg(S::Deliver(relay::Deliver { from: me.0.to_vec(), payload: s.payload }));
                if let Ok(to) = DeviceId::from_slice(&s.to)
                    && let Some(c) = st.online.get(&to)
                {
                    Self::deliver(c, deliver);
                    return;
                }
                if let Some(p) = st.pairing.get(&s.to).filter(|p| p.inviter == me) {
                    let _ = p.tx.try_send(deliver);
                    return;
                }
                let _ = tx.try_send(err(error_code::PEER_OFFLINE, "对端不在线", &s.to));
            }
            C::MailboxPut(p) => {
                let Ok(to) = DeviceId::from_slice(&p.to) else { return };
                let is_member = st.roster.as_ref().is_some_and(|r| r.is_member(&to));
                if !is_member || p.payload.len() > self.limits.mailbox_item_max || p.msg_id.len() > MAX_MSG_ID {
                    let _ = tx.try_send(err(error_code::BAD_REQUEST, "信箱请求无效", &p.to));
                    return;
                }
                let ttl = Duration::from_secs(u64::from(p.ttl_secs)).min(self.limits.mailbox_ttl_max);
                let mail = Mail { from: me, msg_id: p.msg_id, payload: p.payload, expires: Instant::now() + ttl };
                if let Some(c) = st.online.get(&to) {
                    let item = relay::MailboxItem {
                        from: me.0.to_vec(),
                        msg_id: mail.msg_id.clone(),
                        payload: mail.payload.clone(),
                    };
                    Self::deliver(c, msg(S::MailboxItems(relay::MailboxItems { items: vec![item] })));
                }
                let limit = self.limits.mailbox_per_device;
                let q = st.mailbox.entry(to).or_default();
                q.retain(|m| !(m.from == me && m.msg_id == mail.msg_id));
                q.push_back(mail);

                while q.len() > limit {
                    q.pop_front();
                }
            }
            C::MailboxAck(a) => {
                if let Some(q) = st.mailbox.get_mut(&me) {
                    q.retain(|m| !a.msg_ids.contains(&m.msg_id));
                }
            }
            C::MailboxFetch(_) => Self::push_mailbox(&st, me, tx),
            C::RosterPush(p) => self.merge_roster(&mut st, &p.ops),
            C::PairRegister(p) => {
                let (Ok(ticket_id), Ok(relay_auth)) =
                    (<[u8; 32]>::try_from(p.ticket_id.as_slice()), <[u8; 32]>::try_from(p.relay_auth.as_slice()))
                else {
                    let _ = tx.try_send(err(error_code::BAD_REQUEST, "票据格式错误", &[]));
                    return;
                };
                let now = Instant::now();
                st.tickets.retain(|_, t| t.expires > now);
                if st.tickets.values().filter(|t| t.inviter == me).count() >= self.limits.tickets_per_device {
                    let _ = tx.try_send(err(error_code::QUOTA_EXCEEDED, "待用票据过多", &[]));
                    return;
                }
                let ttl = Duration::from_secs(u64::from(p.ttl_secs)).min(self.limits.ticket_ttl_max);
                st.tickets.insert(ticket_id, Ticket { inviter: me, relay_auth, expires: now + ttl });
            }
            C::Ping(p) => {
                let _ = tx.try_send(msg(S::Pong(p)));
            }
            C::Auth(_) | C::PairJoin(_) => {
                let _ = tx.try_send(err(error_code::BAD_REQUEST, "已认证", &[]));
            }
        }
    }

    fn merge_roster(&self, st: &mut State, ops: &[Vec<u8>]) {
        let Some(roster) = st.roster.as_mut() else {
            match Roster::from_ops(&self.group, ops.iter().map(Vec::as_slice)) {
                Ok(r) => {
                    if let Some(d) = &self.data
                        && let Err(e) = d.save_roster(r.group_id(), &r.ops_raw())
                    {
                        warn!(error = %e, "保存名册失败");
                    }
                    info!(members = r.members().count(), "已建立设备组名册");
                    st.roster = Some(r);
                }
                Err(e) => debug!(error = %e, "收到的名册不属于本服务器的设备组"),
            }
            return;
        };
        if !roster.merge(ops.iter().map(Vec::as_slice)) {
            return;
        }
        let all = roster.ops_raw();
        if let Some(d) = &self.data
            && let Err(e) = d.save_roster(roster.group_id(), &all)
        {
            warn!(error = %e, "保存名册失败");
        }

        let revoked: Vec<DeviceId> = st.online.keys().filter(|id| !roster.is_member(id)).copied().collect();
        for id in &revoked {
            if let Some(c) = st.online.remove(id) {
                let _ = c.tx.try_send(msg(S::RosterState(relay::RosterState { ops: all.clone() })));
                let _ = c.tx.try_send(err(error_code::REVOKED, "设备已被移除", &[]));
                close_soon(c.closer);
                info!(device = %id.short(), "设备已被吊销，连接断开");
            }
            st.mailbox.remove(id);
        }
        st.pairing.retain(|_, p| !revoked.contains(&p.inviter));
        st.tickets.retain(|_, t| !revoked.contains(&t.inviter));
        for c in st.online.values() {
            Self::deliver(c, msg(S::RosterState(relay::RosterState { ops: all.clone() })));
        }
        for id in revoked {
            Self::broadcast_presence(st, id, false);
        }
    }

    #[allow(clippy::too_many_arguments)]
    async fn serve_pairing(
        self: &Arc<Self>,
        inviter: DeviceId,
        temp_id: Vec<u8>,
        closer: Closer,
        w: FramedWrite<BoxWrite>,
        mut r: FramedRead<BoxRead>,
        tx: mpsc::Sender<ServerMessage>,
        rx: mpsc::Receiver<ServerMessage>,
    ) -> Result<()> {
        Self::spawn_writer(w, rx, closer.clone());
        let mut bucket = Bucket::new(20.0, 40.0);
        let deadline = tokio::time::sleep(self.limits.pair_session_ttl);
        tokio::pin!(deadline);
        let result = loop {
            let m = tokio::select! {
                _ = &mut deadline => break Ok(()),
                _ = closer.closed() => break Ok(()),
                m = r.recv::<ClientMessage>() => m,
            };
            let m = match m {
                Ok(Some(m)) => m,
                Ok(None) => break Ok(()),
                Err(e) => break Err(anyhow::anyhow!(e)),
            };
            if !bucket.take() {
                break Err(anyhow::anyhow!("配对通道速率超限"));
            }
            match m.body {
                Some(C::Send(s)) if s.to == inviter.0 => {
                    let st = self.lock();
                    if let Some(c) = st.online.get(&inviter) {
                        Self::deliver(c, msg(S::Deliver(relay::Deliver { from: temp_id.clone(), payload: s.payload })));
                    } else {
                        let _ = tx.try_send(err(error_code::PEER_OFFLINE, "邀请方不在线", &s.to));
                    }
                }
                Some(C::Ping(p)) => {
                    let _ = tx.try_send(msg(S::Pong(p)));
                }
                _ => {
                    let _ = tx.try_send(err(error_code::BAD_REQUEST, "配对通道不支持该请求", &[]));
                }
            }
        };
        let mut st = self.lock();
        st.pairing.remove(&temp_id);
        if let Some(c) = st.online.get(&inviter) {
            let _ = c.tx.try_send(msg(S::Presence(relay::Presence { device: temp_id, online: false })));
        }
        closer.close();
        result
    }
}
