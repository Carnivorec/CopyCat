use std::collections::{HashMap, HashSet};
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use copycat_crypto::mailbox::{self, MailboxContext};
use copycat_crypto::pairing::{self, PairingInvite, PairingSecret, PendingJoin, validate_addr};
use copycat_crypto::session::PathKind;
use copycat_crypto::{DeviceId, DeviceIdentity, DeviceRecord, Roster, Session, kdf};
use copycat_proto::app::{self, AppMessage, app_message::Body as A};
use copycat_proto::peer::{PeerFrame, peer_frame::Kind};
use copycat_proto::relay::{self, client_message::Body as C, error_code};
use copycat_proto::{Message, PROTOCOL_VERSION};
use copycat_store::Store;
use copycat_sync::{ClipItem, Effect, Engine};
use copycat_transport::discovery::{self, Beacon, Discovery};
use copycat_transport::lan::{self as tlan, LanEndpoint};
use copycat_transport::relay_client::{RelayAddr, RelayEvent, RelayHandle};
use copycat_transport::tls::RelayKey;
use tokio::sync::{Notify, mpsc};
use tracing::{debug, info, warn};

mod fetch;
pub(crate) mod files;

use crate::lan::{self, LanIn};
use crate::link::{self, Link};
use crate::supervisor::{RelayIn, Supervisor};
use crate::{
    Cmd, CoreError, Node, NodeConfig, NodeEvent, NodeStatus, Options, PeerStatus, RelayCmd, RelayConf, Result,
    load_relay, load_relay_secret, now_ms, save_relay, save_relay_secret,
};
use fetch::Fetches;
use files::{FileIn, Files};

const HANDSHAKE_RETRY: Duration = Duration::from_secs(10);
const PRUNE_EVERY: Duration = Duration::from_secs(600);
const PAIR_TTL_SECS: u32 = 120;

const LAN_AUTH_TIMEOUT: Duration = Duration::from_secs(10);

const LAN_REDIAL: Duration = Duration::from_secs(30);
const MAX_LAN_CONNS: usize = 32;
const MAX_LAN_HINTS: usize = 8;

const MAX_INVITE_LAN: usize = 4;

const LAN_FALLBACK_DIAL: Duration = Duration::from_secs(6);

const ANNOUNCE_EVERY: Duration = Duration::from_secs(30);
const ANNOUNCE_BURST_GAP: Duration = Duration::from_secs(2);
const ANNOUNCE_BURST: u8 = 2;

const BEACON_REPLY_GAP: Duration = Duration::from_secs(3);

const CAP_SCREENSHOTS: &str = "screenshots";

const PROBE_AFTER: Duration = Duration::from_secs(3);

const PROBE_TIMEOUT: Duration = Duration::from_secs(5);

const RESUME_RESYNC: Duration = Duration::from_secs(30);

const RESUME_RECONNECT: Duration = Duration::from_secs(5 * 60);

#[derive(Clone, Debug, PartialEq, Eq)]
enum PairVia {
    Relay(Vec<u8>),

    Lan(u64),
}

enum JoinerState {
    Handshaking { via: PairVia, sid: u64, pending: PendingJoin },
    AwaitingConfirm { via: PairVia, sid: u64, session: Session, record: DeviceRecord },
}

impl JoinerState {
    fn via(&self) -> &PairVia {
        match self {
            JoinerState::Handshaking { via, .. } | JoinerState::AwaitingConfirm { via, .. } => via,
        }
    }
}

struct Invite {
    secret: PairingSecret,
    expires: Instant,
    joiner: Option<JoinerState>,
}

struct Probe {
    since: Instant,
    pinged: Option<Instant>,
}

struct LanConnState {
    tx: mpsc::Sender<PeerFrame>,

    peer: Option<DeviceId>,
    since: Instant,
}

struct Actor {
    identity: Arc<DeviceIdentity>,
    me: DeviceId,
    cfg: NodeConfig,
    roster: Roster,
    engine: Engine,

    store: Mutex<Store>,
    events: mpsc::Sender<NodeEvent>,
    invite: Option<Invite>,
    dirty: bool,
    last_prune: Instant,
    revoked: bool,

    relay_conf: RelayConf,
    relay: Option<RelayHandle>,
    relay_tx: mpsc::Sender<(u64, RelayIn)>,

    relay_gen: u64,

    roster_shared: Arc<Mutex<Vec<Vec<u8>>>>,
    online: HashSet<DeviceId>,
    links: HashMap<DeviceId, Link>,
    kick: Arc<Notify>,
    stop: Arc<AtomicBool>,
    relay_enabled: bool,

    lan_ep: Option<Arc<LanEndpoint>>,
    lan_tx: mpsc::Sender<LanIn>,
    lan_ids: Arc<AtomicU64>,
    lan_conns: HashMap<u64, LanConnState>,
    lan_links: HashMap<DeviceId, (u64, Link)>,
    lan_hints: HashMap<DeviceId, Vec<SocketAddr>>,
    lan_dialing: HashSet<DeviceId>,
    lan_last_dial: HashMap<DeviceId, Instant>,

    lan_hint_at: HashMap<DeviceId, Instant>,
    disc: Option<Arc<Discovery>>,
    disc_key: [u8; 32],
    disc_next: Instant,
    disc_burst: u8,
    disc_replied: HashMap<DeviceId, Instant>,

    lan_rx_at: HashMap<DeviceId, Instant>,
    lan_probes: HashMap<DeviceId, Probe>,

    peer_caps: HashMap<DeviceId, HashSet<String>>,

    files: Files,
    fetches: Fetches,
}

pub(crate) fn spawn(
    store: Store,
    cfg: NodeConfig,
    identity: Arc<DeviceIdentity>,
    roster: Roster,
    hints: Vec<(DeviceId, Vec<SocketAddr>)>,
) -> (Node, mpsc::Receiver<NodeEvent>) {
    let me = identity.id();
    let items = store.items().unwrap_or_default();
    let mut engine = match store.load_snapshot().ok().flatten() {
        Some(snap) => Engine::restore(me, cfg.engine.clone(), snap, items),
        None => Engine::restore(me, cfg.engine.clone(), Engine::new(me, cfg.engine.clone()).snapshot(), items),
    };
    engine.cap_future(now_ms());
    engine.set_members(roster.members().map(|m| m.id));
    let (keep, keep_ms) = cfg.options.retention();
    engine.set_retention(keep, keep_ms);

    let (cmd_tx, cmd_rx) = mpsc::channel(64);
    let (ev_tx, ev_rx) = mpsc::channel(1024);
    let (relay_tx, relay_rx) = mpsc::channel(256);
    let (lan_tx, lan_rx) = mpsc::channel(256);
    let (file_tx, file_rx) = mpsc::unbounded_channel();
    let lan_ids = Arc::new(AtomicU64::new(1));
    let relay_conf = load_relay(&store).unwrap_or_else(|e| {
        warn!(error = %e, "读取中转配置失败");
        RelayConf::default()
    });
    let relay_enabled = cfg.relay_enabled;
    let now = Instant::now();
    let lan_hint_at = hints.iter().map(|(p, _)| (*p, now)).collect();
    let lan_hints = hints.into_iter().collect();

    let lan_ep = if cfg.lan.enabled {
        match LanEndpoint::bind(cfg.lan.port, cfg.lan.loopback_for_tests) {
            Ok(ep) => {
                let ep = Arc::new(ep);
                info!(port = ep.port(), "局域网直连已启用");
                lan::spawn_accept_loop(ep.clone(), lan_ids.clone(), lan_tx.clone());
                Some(ep)
            }
            Err(e) => {
                warn!(error = %e, "局域网监听启动失败，仅使用中转");
                None
            }
        }
    } else {
        None
    };
    let disc = if lan_ep.is_some() && cfg.lan.discovery {
        match Discovery::bind(cfg.lan.discovery_port, cfg.lan.loopback_for_tests) {
            Ok(d) => {
                let d = Arc::new(d);
                if cfg.lan.discovery_targets.is_empty() {
                    d.join(&discovery::lan_ifaces());
                }
                lan::spawn_discovery_loop(d.clone(), lan_tx.clone());
                Some(d)
            }
            Err(e) => {
                warn!(error = %e, "局域网发现启动失败，只能经中转交换局域网地址");
                None
            }
        }
    } else {
        None
    };
    let disc_key = lan::discovery_key(roster.group_id());
    let roster_shared = Arc::new(Mutex::new(roster.ops_raw()));

    let mut actor = Actor {
        identity,
        me,
        cfg,
        roster,
        engine,
        store: Mutex::new(store),
        events: ev_tx,
        invite: None,
        dirty: false,
        last_prune: Instant::now() - PRUNE_EVERY,
        revoked: false,
        relay_conf,
        relay: None,
        relay_tx,
        relay_gen: 0,
        roster_shared,
        online: HashSet::new(),
        links: HashMap::new(),
        kick: Arc::new(Notify::new()),
        stop: Arc::new(AtomicBool::new(true)),
        relay_enabled,
        lan_ep,
        lan_tx,
        lan_ids,
        lan_conns: HashMap::new(),
        lan_links: HashMap::new(),
        lan_hints,
        lan_dialing: HashSet::new(),
        lan_last_dial: HashMap::new(),
        lan_hint_at,
        disc,
        disc_key,
        disc_next: Instant::now(),
        disc_burst: ANNOUNCE_BURST,
        disc_replied: HashMap::new(),
        lan_rx_at: HashMap::new(),
        lan_probes: HashMap::new(),
        peer_caps: HashMap::new(),
        files: Files::new(file_tx),
        fetches: Fetches::default(),
    };
    actor.start_supervisor();
    tokio::spawn(actor.run(cmd_rx, relay_rx, lan_rx, file_rx));
    (Node { tx: cmd_tx, id: me }, ev_rx)
}

impl Actor {
    async fn run(
        mut self,
        mut cmds: mpsc::Receiver<Cmd>,
        mut relay_rx: mpsc::Receiver<(u64, RelayIn)>,
        mut lan_rx: mpsc::Receiver<LanIn>,
        mut file_rx: mpsc::UnboundedReceiver<FileIn>,
    ) {
        let mut tick = tokio::time::interval(Duration::from_secs(1));
        let mut done_tx = None;
        loop {
            let wake = self.files.wake;
            tokio::select! {
                c = cmds.recv() => match c {
                    Some(Cmd::Shutdown(done)) => {
                        self.shutdown();
                        done_tx = Some(done);
                        break;
                    }
                    Some(c) => self.on_cmd(c).await,
                    None => { self.shutdown(); break; }
                },
                Some((generation, r)) = relay_rx.recv() => {
                    if generation == self.relay_gen {
                        self.on_relay(r).await;
                    } else if let RelayIn::Connected(h, _) = r {
                        h.close();
                    }
                }
                Some(l) = lan_rx.recv() => self.on_lan(l).await,
                Some(f) = file_rx.recv() => self.on_file_io(f).await,
                () = sleep_until(wake) => self.files_pump().await,
                _ = tick.tick() => self.on_tick().await,
            }
        }

        cmds.close();
        drop(self);
        if let Some(done) = done_tx {
            let _ = done.send(());
        }
    }

    fn shutdown(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        self.kick.notify_one();
        self.save_state();
        if let Some(h) = self.relay.take() {
            h.close();
        }
        self.lan_conns.clear();
        if let Some(ep) = self.lan_ep.take() {
            ep.close();
        }
    }

    async fn emit(&self, ev: NodeEvent) {
        let _ = self.events.send(ev).await;
    }

    fn db(&self) -> MutexGuard<'_, Store> {
        self.store.lock().unwrap_or_else(|p| p.into_inner())
    }

    fn group(&self) -> [u8; 32] {
        *self.roster.group_id()
    }

    fn save_state(&mut self) {
        if self.dirty {
            if let Err(e) = self.db().save_snapshot(&self.engine.snapshot()) {
                warn!(error = %e, "保存同步状态失败");
            }
            self.dirty = false;
        }
    }

    fn relay_up(&self, p: &DeviceId) -> bool {
        self.links.get(p).is_some_and(Link::is_up)
    }

    fn lan_up(&self, p: &DeviceId) -> bool {
        self.lan_links.get(p).is_some_and(|(_, l)| l.is_up())
    }

    fn peer_up(&self, p: &DeviceId) -> bool {
        self.relay_up(p) || self.lan_up(p)
    }

    fn up_peers(&self) -> Vec<DeviceId> {
        let mut v: Vec<DeviceId> = self.links.iter().filter(|(_, l)| l.is_up()).map(|(id, _)| *id).collect();
        for (id, (_, l)) in &self.lan_links {
            if l.is_up() && !v.contains(id) {
                v.push(*id);
            }
        }
        v
    }

    async fn on_cmd(&mut self, c: Cmd) {
        match c {
            Cmd::Copy(reps, reply) => {
                let r = self.local_copy(reps).await;
                let _ = reply.send(r);
            }
            Cmd::IsEcho(fp, reply) => {
                let _ = reply.send(self.engine.is_echo(&fp, now_ms()));
            }
            Cmd::IsCurrent(fp, reply) => {
                let _ = reply.send(self.engine.is_current_content(&fp));
            }
            Cmd::CreateInvite(reply) => {
                let r = self.create_invite().await;
                let _ = reply.send(r);
            }
            Cmd::RespondPairing(accept, reply) => {
                let r = self.respond_pairing(accept).await;
                let _ = reply.send(r);
            }
            Cmd::Remove(id, reply) => {
                let r = self.remove_device(id).await;
                let _ = reply.send(r);
            }
            Cmd::Leave(reply) => {
                let r = self.leave_group().await;
                let _ = reply.send(r);
            }
            Cmd::Members(reply) => {
                let _ = reply.send(self.roster.members().cloned().collect());
            }
            Cmd::History(reply) => {
                let _ = reply.send(self.engine.history(now_ms()).into_iter().cloned().collect());
            }
            Cmd::OnlinePeers(reply) => {
                let _ = reply.send(self.up_peers());
            }
            Cmd::Status(reply) => {
                let peers = self
                    .roster
                    .members()
                    .filter(|m| m.id != self.me)
                    .map(|m| PeerStatus {
                        id: m.id,
                        relay: self.relay_up(&m.id),
                        lan: self.lan_up(&m.id),
                        screenshots: self.peer_caps.get(&m.id).is_some_and(|c| c.contains(CAP_SCREENSHOTS)),
                    })
                    .collect();
                let _ = reply.send(NodeStatus {
                    me: self.me,
                    relay_addr: self.relay_conf.relay.as_ref().map(|r| r.addr.clone()),
                    relay_connected: self.relay.is_some(),
                    relay_enabled: self.relay_enabled,
                    lan_port: self.lan_ep.as_ref().map(|e| e.port()),
                    peers,
                });
            }
            Cmd::Forget(id, reply) => {
                let fx = self.engine.forget(&id);
                self.run_effects(fx).await;
                let _ = reply.send(());
            }
            Cmd::ForgetAll(reply) => {
                let fx = self.engine.forget_all();
                self.run_effects(fx).await;
                let _ = reply.send(());
            }
            Cmd::ForgetEverywhere(item, reply) => {
                let msg =
                    app::HistoryClear { all: item.is_none(), items: item.map(|i| i.to_vec()).into_iter().collect() };
                let peers = self.up_peers();
                for p in &peers {
                    self.send_app(*p, AppMessage::new(A::HistoryClear(msg.clone()))).await;
                }
                let _ = reply.send(peers.len());
            }
            Cmd::Resumed(slept) => self.on_resumed(slept).await,
            Cmd::NetworkChanged => {
                self.kick.notify_one();

                self.probe_all_lan().await;

                self.lan_last_dial.clear();
                for p in self.up_peers() {
                    self.send_lan_hint(p).await;
                }
                if let Some(d) = &self.disc
                    && self.cfg.lan.discovery_targets.is_empty()
                {
                    d.join(&discovery::lan_ifaces());
                }
                self.disc_next = Instant::now();
                self.disc_burst = ANNOUNCE_BURST;
            }
            Cmd::SetRelayEnabled(enabled) => self.set_relay_enabled(enabled).await,
            Cmd::Relay(c, reply) => {
                let r = self.relay_cmd(c).await;
                let _ = reply.send(r);
            }
            Cmd::SendFiles(peer, files, kind, reply) => {
                let r = self.send_files(peer, files, files::Kind::from_public(kind)).await;
                let _ = reply.send(r);
            }
            Cmd::ShareText(peer, text, reply) => {
                let r = self.share_text(peer, text).await;
                let _ = reply.send(r);
            }
            Cmd::AcceptTransfer(id, dir, reply) => {
                let r = self.accept_transfer(id, dir).await;
                let _ = reply.send(r);
            }
            Cmd::CancelTransfer(id, reply) => {
                let r = self.cancel_transfer(id).await;
                let _ = reply.send(r);
            }
            Cmd::Transfers(reply) => {
                let _ = reply.send(self.transfers());
            }
            Cmd::ClearTransfers(reply) => {
                self.clear_transfers();
                let _ = reply.send(());
            }
            Cmd::SetOptions(o) => self.set_options(o).await,
            Cmd::FetchItem(id, reply) => self.fetch_item(id, reply).await,
            Cmd::Shutdown(_) => unreachable!("在 run 中处理"),
        }
    }

    async fn set_relay_enabled(&mut self, enabled: bool) {
        if enabled == self.relay_enabled || self.revoked {
            return;
        }
        self.relay_enabled = enabled;
        if enabled {
            self.start_supervisor();
        } else {
            self.stop_supervisor("已暂停中转").await;
        }
    }

    fn start_supervisor(&mut self) {
        let Some(relay) = self.relay_conf.relay.clone() else { return };
        if !self.relay_enabled || self.revoked {
            return;
        }
        self.relay_gen += 1;
        self.stop = Arc::new(AtomicBool::new(false));
        self.kick = Arc::new(Notify::new());
        tokio::spawn(
            Supervisor {
                relay,
                tuning: self.cfg.tuning.clone(),
                identity: self.identity.clone(),
                roster: self.roster_shared.clone(),
                generation: self.relay_gen,
                out: self.relay_tx.clone(),
                kick: self.kick.clone(),
                stop: self.stop.clone(),
            }
            .run(),
        );
    }

    async fn stop_supervisor(&mut self, reason: &str) {
        self.relay_gen += 1;
        self.stop.store(true, Ordering::Relaxed);
        self.kick.notify_one();
        if let Some(h) = self.relay.take() {
            h.close();
        }
        Box::pin(self.on_relay(RelayIn::Disconnected(reason.into()))).await;
    }

    async fn relay_cmd(&mut self, c: RelayCmd) -> Result<Option<String>> {
        match c {
            RelayCmd::Secret => {
                let Some(r) = &self.relay_conf.relay else { return Ok(None) };
                let key = load_relay_secret(&self.db())?;
                Ok(key.filter(|k| k.public() == r.key && k.group() == self.group()).map(|k| k.encode()))
            }
            RelayCmd::Generate(addr) => {
                let addr = check_relay_addr(&addr)?;
                let key = RelayKey::generate(self.group())?;
                let text = key.encode();
                save_relay_secret(&self.db(), &text)?;
                self.set_relay_conf(Some(RelayAddr { addr, key: key.public() })).await;
                Ok(Some(text))
            }
            RelayCmd::Address(addr) => {
                let addr = check_relay_addr(&addr)?;
                let key = self.relay_conf.relay.as_ref().ok_or(CoreError::InvalidState("还没有配置中转服务器"))?.key;
                self.set_relay_conf(Some(RelayAddr { addr, key })).await;
                Ok(None)
            }
            RelayCmd::Remove => {
                self.set_relay_conf(None).await;
                Ok(None)
            }
        }
    }

    async fn set_relay_conf(&mut self, relay: Option<RelayAddr>) {
        let version = now_ms().max(self.relay_conf.version + 1);
        self.apply_relay_conf(RelayConf { relay, version, author: Some(self.me) }, None).await;
    }

    async fn apply_relay_conf(&mut self, conf: RelayConf, from: Option<DeviceId>) {
        if let Err(e) = save_relay(&self.db(), &conf) {
            warn!(error = %e, "保存中转配置失败");
        }
        let server_changed = conf.relay != self.relay_conf.relay;
        self.relay_conf = conf;
        if server_changed {
            info!(addr = ?self.relay_conf.relay.as_ref().map(|r| &r.addr), "中转配置已更改");
            self.stop_supervisor("中转配置已更改").await;
            self.start_supervisor();
        }
        let msg = self.relay_conf_msg();
        for p in self.up_peers() {
            if Some(p) != from {
                self.send_app(p, msg.clone()).await;
            }
        }
        self.emit(NodeEvent::RelayChanged).await;
    }

    fn relay_conf_msg(&self) -> AppMessage {
        let c = &self.relay_conf;
        AppMessage::new(A::RelayConfig(app::RelayConfig {
            addr: c.relay.as_ref().map(|r| r.addr.clone()).unwrap_or_default(),
            key: c.relay.as_ref().map(|r| r.key.to_vec()).unwrap_or_default(),
            version: c.version,
            author: c.author.map(|a| a.0.to_vec()).unwrap_or_default(),
        }))
    }

    async fn on_relay_config(&mut self, peer: DeviceId, c: &app::RelayConfig) {
        let Some(conf) = parse_relay_conf(c) else {
            warn!(peer = %peer.short(), "收到的中转配置无效，已忽略");
            return;
        };
        if conf.order() > self.relay_conf.order() {
            info!(peer = %peer.short(), "从其他设备同步了中转配置");
            self.apply_relay_conf(conf, Some(peer)).await;
        } else if conf.order() < self.relay_conf.order() {
            let msg = self.relay_conf_msg();
            self.send_app(peer, msg).await;
        }
    }

    async fn on_resumed(&mut self, slept: Duration) {
        if slept >= RESUME_RECONNECT {
            info!(secs = slept.as_secs(), "从深度睡眠醒来，重连");
            Box::pin(self.on_cmd(Cmd::NetworkChanged)).await;
        } else if slept >= RESUME_RESYNC {
            debug!(secs = slept.as_secs(), "从深度睡眠醒来，请对方补发");
            self.probe_all_lan().await;
        } else {
            return;
        }
        for p in self.up_peers() {
            let fx = self.engine.resync(p);
            self.run_effects(fx).await;
        }
    }

    async fn set_options(&mut self, o: Options) {
        let screenshots_changed = o.accept_screenshots != self.cfg.options.accept_screenshots;
        let (n, ms) = o.retention();
        self.engine.set_retention(n, ms);
        self.cfg.options = o;
        if screenshots_changed {
            for p in self.up_peers() {
                let hello = self.hello();
                self.send_app(p, hello).await;
            }
        }
    }

    fn hello(&self) -> AppMessage {
        let mut capabilities = vec!["lan".to_owned(), "clip-fetch".to_owned()];
        if self.cfg.options.accept_screenshots {
            capabilities.push(CAP_SCREENSHOTS.to_owned());
        }
        AppMessage::new(A::Hello(app::Hello {
            protocol_version: PROTOCOL_VERSION,
            device_name: self.cfg.device_name.clone(),
            roster_digest: self.roster.digest().to_vec(),
            capabilities,
        }))
    }

    async fn local_copy(&mut self, reps: Vec<copycat_sync::Rep>) -> Result<()> {
        if self.revoked {
            return Err(CoreError::InvalidState("本机已被移出设备组"));
        }
        let fx = self.engine.local_copy(reps, now_ms());
        let new_items: Vec<ClipItem> =
            fx.iter().filter_map(|e| if let Effect::Store(i) = e { Some(i.clone()) } else { None }).collect();
        self.run_effects(fx).await;
        for item in &new_items {
            self.mail_to_offline_peers(item).await;
        }
        Ok(())
    }

    async fn mail_to_offline_peers(&mut self, item: &ClipItem) {
        if self.cfg.mailbox_ttl_secs == 0 || item.reps.iter().any(|r| r.inline.is_none()) {
            return;
        }
        let Some(relay) = self.relay.clone() else { return };
        let payload = Engine::batch_for(item).encode_to_vec();
        if payload.len() > mailbox::MAX_PLAINTEXT {
            return;
        }
        let group = self.group();
        let targets: Vec<DeviceRecord> =
            self.roster.members().filter(|m| m.id != self.me && !self.peer_up(&m.id)).cloned().collect();
        for m in targets {
            let ctx = MailboxContext { group_id: &group, sender: self.me, recipient: m.id, msg_id: &item.id };
            match mailbox::seal(&self.identity, &m.dh_pub, &ctx, &payload) {
                Ok(sealed) => {
                    let _ = relay
                        .send(C::MailboxPut(relay::MailboxPut {
                            to: m.id.0.to_vec(),
                            msg_id: item.id.to_vec(),
                            payload: sealed,
                            ttl_secs: self.cfg.mailbox_ttl_secs,
                        }))
                        .await;
                }
                Err(e) => warn!(error = %e, "离线信箱加密失败"),
            }
        }
    }

    async fn create_invite(&mut self) -> Result<String> {
        let lan = self.invite_lan_addrs();

        let relay = self.relay.clone().zip(self.relay_conf.relay.clone());
        if relay.is_none() && lan.is_empty() {
            return Err(CoreError::InvalidState("没有可用的连接方式：未连上中转服务器，也没有局域网地址"));
        }
        let invite =
            PairingInvite::new(&self.identity, relay.as_ref().map(|(_, r)| (r.addr.clone(), r.key)), lan, now_ms())?;
        if let Some((handle, _)) = &relay {
            handle
                .send(C::PairRegister(relay::PairRegister {
                    ticket_id: invite.secret.ticket_id().to_vec(),
                    relay_auth: invite.secret.relay_auth().to_vec(),
                    ttl_secs: PAIR_TTL_SECS,
                }))
                .await?;
        }
        self.invite = Some(Invite {
            secret: invite.secret.clone(),
            expires: Instant::now() + Duration::from_secs(u64::from(PAIR_TTL_SECS)),
            joiner: None,
        });
        Ok(invite.to_uri())
    }

    fn invite_lan_addrs(&self) -> Vec<String> {
        let Some(ep) = &self.lan_ep else { return Vec::new() };
        let ips = if self.cfg.lan.loopback_for_tests { vec![Ipv4Addr::LOCALHOST] } else { tlan::local_lan_ips() };
        ips.iter().take(MAX_INVITE_LAN).map(|ip| format!("{ip}:{}", ep.port())).collect()
    }

    async fn respond_pairing(&mut self, accept: bool) -> Result<DeviceId> {
        let Some(Invite { joiner: Some(JoinerState::AwaitingConfirm { via, sid, mut session, record }), .. }) =
            self.invite.take()
        else {
            return Err(CoreError::InvalidState("没有待确认的配对请求"));
        };
        if !accept {
            self.send_pair_frame(&via, link::frame(sid, Kind::Close(true))).await;
            return Err(CoreError::Pairing("已拒绝".into()));
        }
        let op = self.roster.add(&self.identity, &record, now_ms())?;
        self.after_local_roster_change(vec![op]).await;

        let msg = AppMessage::new(A::RosterOps(app::RosterOps { ops: self.ops_genesis_first() })).encode_to_vec();
        for rec in session.seal(&msg)? {
            self.send_pair_frame(&via, link::frame(sid, Kind::Data(rec))).await;
        }
        if let PairVia::Lan(conn_id) = via
            && let Some(st) = self.lan_conns.get_mut(&conn_id)
        {
            st.since = Instant::now();
        }
        info!(device = %record.id.short(), "新设备已加入");
        self.emit(NodeEvent::Paired(record.id)).await;
        Ok(record.id)
    }

    fn ops_genesis_first(&self) -> Vec<Vec<u8>> {
        let group = self.group();
        let mut ops = self.roster.ops_raw();
        ops.sort_by_key(|o| kdf::hash(o) != group);
        ops
    }

    async fn remove_device(&mut self, id: DeviceId) -> Result<()> {
        let op = self.roster.remove(&self.identity, id, now_ms())?;
        self.after_local_roster_change(vec![op]).await;
        Ok(())
    }

    async fn leave_group(&mut self) -> Result<crate::LeaveReport> {
        if self.revoked || !self.roster.is_member(&self.me) {
            return Ok(crate::LeaveReport { peers: 0, relay: false });
        }
        self.roster.remove(&self.identity, self.me, now_ms())?;
        let all = self.roster.ops_raw();
        if let Err(e) = self.db().add_roster_ops(&all) {
            warn!(error = %e, "保存名册失败");
        }
        let mut relay = false;
        if let Some(r) = &self.relay {
            relay = r.send(C::RosterPush(relay::RosterPush { ops: all.clone() })).await.is_ok();
        }
        let peers = self.up_peers();
        for p in &peers {
            self.send_app(*p, AppMessage::new(A::RosterOps(app::RosterOps { ops: all.clone() }))).await;
        }
        if relay || !peers.is_empty() {
            tokio::time::sleep(Duration::from_millis(1500)).await;
        }
        self.apply_roster_state().await;
        Ok(crate::LeaveReport { peers: peers.len(), relay })
    }

    async fn after_local_roster_change(&mut self, _ops: Vec<Vec<u8>>) {
        let all = self.roster.ops_raw();
        if let Err(e) = self.db().add_roster_ops(&all) {
            warn!(error = %e, "保存名册失败");
        }

        if let Some(r) = &self.relay {
            let _ = r.send(C::RosterPush(relay::RosterPush { ops: all.clone() })).await;
        }
        for p in self.up_peers() {
            self.send_app(p, AppMessage::new(A::RosterOps(app::RosterOps { ops: all.clone() }))).await;
        }
        self.apply_roster_state().await;
    }

    async fn merge_roster(&mut self, ops: &[Vec<u8>]) {
        if !self.roster.merge(ops.iter().map(Vec::as_slice)) {
            return;
        }
        if let Err(e) = self.db().add_roster_ops(&self.roster.ops_raw()) {
            warn!(error = %e, "保存名册失败");
        }
        self.apply_roster_state().await;
    }

    async fn apply_roster_state(&mut self) {
        *self.roster_shared.lock().unwrap_or_else(|p| p.into_inner()) = self.roster.ops_raw();
        self.engine.set_members(self.roster.members().map(|m| m.id));
        let gone: Vec<DeviceId> = self.links.keys().filter(|id| !self.roster.is_member(id)).copied().collect();
        for id in gone {
            self.drop_link(id).await;
        }
        let gone_lan: Vec<DeviceId> = self.lan_links.keys().filter(|id| !self.roster.is_member(id)).copied().collect();
        for id in gone_lan {
            self.drop_lan(id).await;
        }
        self.online.retain(|id| self.roster.is_member(id));
        self.lan_hints.retain(|id, _| self.roster.is_member(id));
        self.lan_hint_at.retain(|id, _| self.roster.is_member(id));
        self.disc_replied.retain(|id, _| self.roster.is_member(id));
        if !self.roster.is_member(&self.me) && !self.revoked {
            self.revoked = true;
            warn!("本机已被移出设备组");

            self.relay_gen += 1;
            self.stop.store(true, Ordering::Relaxed);
            self.kick.notify_one();
            if let Some(h) = self.relay.take() {
                h.close();
            }
            let all: Vec<DeviceId> = self.lan_links.keys().copied().collect();
            for id in all {
                self.drop_lan(id).await;
            }
            self.lan_conns.clear();
            if let Some(ep) = self.lan_ep.take() {
                ep.close();
            }
            self.emit(NodeEvent::Revoked).await;
        }
        self.emit(NodeEvent::RosterChanged).await;
    }

    async fn on_relay(&mut self, r: RelayIn) {
        match r {
            RelayIn::Connected(handle, online) => {
                if !self.relay_enabled {
                    handle.close();
                    return;
                }
                self.relay = Some(handle.clone());
                self.emit(NodeEvent::RelayConnected).await;

                let _ = handle.send(C::RosterPush(relay::RosterPush { ops: self.roster.ops_raw() })).await;
                self.online = online.into_iter().filter(|id| self.roster.is_member(id)).collect();
                let peers: Vec<DeviceId> = self.online.iter().copied().collect();
                for p in peers {
                    self.maybe_initiate(p).await;
                }
            }
            RelayIn::Disconnected(reason) => {
                self.relay = None;
                self.emit(NodeEvent::RelayDisconnected(reason)).await;
                self.online.clear();
                let ids: Vec<DeviceId> = self.links.keys().copied().collect();
                for id in ids {
                    self.drop_link(id).await;
                }
            }
            RelayIn::Event(ev) => self.on_relay_event(ev).await,
        }
    }

    async fn on_relay_event(&mut self, ev: RelayEvent) {
        match ev {
            RelayEvent::Deliver { from, payload } => {
                let Ok(frame) = PeerFrame::decode(payload.as_slice()) else { return };
                if let Some(id) = DeviceId::from_slice(&from).ok().filter(|id| self.roster.is_member(id)) {
                    self.on_peer_frame(id, frame).await;
                } else {
                    self.on_pairing_frame(PairVia::Relay(from), frame).await;
                }
            }
            RelayEvent::Presence { device, online } => {
                if !self.roster.is_member(&device) || device == self.me {
                    if !online {
                        self.on_pairing_gone(&PairVia::Relay(device.0.to_vec()));
                    }
                    return;
                }
                if online {
                    self.online.insert(device);

                    if self.relay_up(&device) {
                        info!(peer = %device.short(), "对方重新连上中转服务器，重建会话");
                    }
                    self.drop_link(device).await;
                    self.maybe_initiate(device).await;
                } else {
                    self.online.remove(&device);
                    self.drop_link(device).await;
                }
            }
            RelayEvent::Mailbox(items) => self.on_mailbox(items).await,
            RelayEvent::Roster(ops) => self.merge_roster(&ops).await,
            RelayEvent::Error { code: error_code::PEER_OFFLINE, context, .. } => {
                if let Ok(id) = DeviceId::from_slice(&context) {
                    self.online.remove(&id);
                    self.drop_link(id).await;
                }
            }
            RelayEvent::Error { code, message, .. } => debug!(code, %message, "中转服务器返回错误"),
            RelayEvent::Pong(_) | RelayEvent::Closed(_) => {}
        }
    }

    async fn on_mailbox(&mut self, items: Vec<relay::MailboxItem>) {
        let group = self.group();
        let mut ids = Vec::with_capacity(items.len());
        let mut fx = Vec::new();
        for it in items {
            ids.push(it.msg_id.clone());
            let Ok(from) = DeviceId::from_slice(&it.from) else { continue };
            let Some(sender) = self.roster.member(&from).cloned() else { continue };
            let ctx = MailboxContext { group_id: &group, sender: from, recipient: self.me, msg_id: &it.msg_id };
            let Ok(pt) = mailbox::open(&self.identity, &sender.dh_pub, &ctx, &it.payload) else {
                warn!(from = %from.short(), "离线信箱内容无法解密，已丢弃");
                continue;
            };
            if let Ok(AppMessage { body: Some(A::ClipBatch(b)) }) = AppMessage::decode(pt.as_slice()) {
                fx.extend(self.engine.on_offline_batch(b, now_ms()));
            }
        }
        self.run_effects(fx).await;
        if let Some(r) = &self.relay {
            let _ = r.send(C::MailboxAck(relay::MailboxAck { msg_ids: ids })).await;
        }
    }

    async fn maybe_initiate(&mut self, peer: DeviceId) {
        if self.me >= peer || self.revoked {
            return;
        }
        let stale = match self.links.get(&peer) {
            None => true,
            Some(Link::Initiating { since, .. }) => since.elapsed() > HANDSHAKE_RETRY,
            Some(Link::Up { .. }) => false,
        };
        if !stale {
            return;
        }
        let Some(rec) = self.roster.member(&peer).cloned() else { return };
        match link::initiate(&self.identity, &self.group(), &rec, PathKind::Relay) {
            Ok((l, f)) => {
                self.links.insert(peer, l);
                self.send_relay_frame(&peer.0, f).await;
            }
            Err(e) => warn!(error = %e, "发起握手失败"),
        }
    }

    async fn on_peer_frame(&mut self, peer: DeviceId, f: PeerFrame) {
        let Some(rec) = self.roster.member(&peer).cloned() else { return };
        let sid = f.session_id;
        match f.kind {
            Some(Kind::HandshakeInit(m1)) => {
                match link::respond(&self.identity, &self.group(), &rec, PathKind::Relay, sid, &m1) {
                    Ok((l, reply)) => {
                        let was_up = self.relay_up(&peer);
                        self.links.insert(peer, l);
                        self.send_relay_frame(&peer.0, reply).await;
                        self.on_path_up(peer, false, was_up).await;
                    }
                    Err(e) => warn!(peer = %peer.short(), error = %e, "握手被拒绝"),
                }
            }
            Some(Kind::HandshakeResp(m2)) => {
                if !matches!(self.links.get(&peer), Some(Link::Initiating { sid: s, .. }) if *s == sid) {
                    return;
                }
                let l = self.links.remove(&peer).expect("上面已确认存在");
                match link::finish(l, &rec, &m2) {
                    Ok(up) => {
                        self.links.insert(peer, up);
                        self.on_path_up(peer, false, false).await;
                    }
                    Err(e) => warn!(peer = %peer.short(), error = %e, "握手失败"),
                }
            }
            Some(Kind::Data(record)) => {
                let opened = match self.links.get_mut(&peer) {
                    Some(Link::Up { session, sid: s }) if *s == sid => session.open(&record),
                    _ => {
                        self.send_relay_frame(&peer.0, link::frame(sid, Kind::Close(true))).await;
                        return;
                    }
                };
                match opened {
                    Ok(Some(bytes)) => {
                        if self.lan_up(&peer) {
                            self.probe_lan(peer, true).await;
                        }
                        self.on_app_bytes(peer, &bytes).await
                    }
                    Ok(None) => {}
                    Err(e) => {
                        warn!(peer = %peer.short(), error = %e, "会话解密失败，断开重建");
                        self.drop_link(peer).await;
                        self.send_relay_frame(&peer.0, link::frame(sid, Kind::Close(true))).await;
                        self.maybe_initiate(peer).await;
                    }
                }
            }
            Some(Kind::Close(_)) => {
                if self.links.get(&peer).is_some_and(|l| l.sid() == sid) {
                    self.drop_link(peer).await;
                    self.maybe_initiate(peer).await;
                }
            }
            Some(Kind::PairInit(_) | Kind::PairResp(_)) | None => {}
        }
    }

    async fn on_lan(&mut self, ev: LanIn) {
        match ev {
            LanIn::Conn { conn_id, tx, dialed } => {
                if let Some(p) = dialed {
                    self.lan_dialing.remove(&p);
                }
                if self.revoked || self.lan_conns.len() >= MAX_LAN_CONNS {
                    return;
                }
                self.lan_conns.insert(conn_id, LanConnState { tx, peer: dialed, since: Instant::now() });
                if let Some(peer) = dialed {
                    if self.lan_up(&peer) {
                        self.lan_conns.remove(&conn_id);
                        return;
                    }
                    let Some(rec) = self.roster.member(&peer).cloned() else { return };
                    match link::initiate(&self.identity, &self.group(), &rec, PathKind::Lan) {
                        Ok((l, f)) => {
                            self.lan_links.insert(peer, (conn_id, l));
                            self.send_lan_frame(conn_id, f);
                        }
                        Err(e) => warn!(error = %e, "局域网握手发起失败"),
                    }
                }
            }
            LanIn::Frame { conn_id, frame } => self.on_lan_frame(conn_id, frame).await,
            LanIn::Closed { conn_id } => {
                self.lan_conns.remove(&conn_id);
                self.on_pairing_gone(&PairVia::Lan(conn_id));
                let peer = self.lan_links.iter().find(|(_, (c, _))| *c == conn_id).map(|(p, _)| *p);
                if let Some(p) = peer {
                    self.drop_lan(p).await;
                }
            }
            LanIn::Beacon { beacon, from } => self.on_beacon(beacon, from).await,
            LanIn::DialFailed { peer } => {
                self.lan_dialing.remove(&peer);
            }
        }
    }

    async fn on_lan_frame(&mut self, conn_id: u64, f: PeerFrame) {
        let Some(state) = self.lan_conns.get(&conn_id) else { return };
        let sid = f.session_id;
        let Some(peer) = state.peer else {
            let m1 = match f.kind {
                Some(Kind::HandshakeInit(m1)) => m1,
                Some(Kind::PairInit(_) | Kind::Data(_) | Kind::Close(_)) if self.invite.is_some() => {
                    return self.on_pairing_frame(PairVia::Lan(conn_id), f).await;
                }
                _ => {
                    self.lan_conns.remove(&conn_id);
                    return;
                }
            };
            let roster = &self.roster;
            let me = self.me;
            let res = link::respond_unknown(&self.identity, &self.group(), PathKind::Lan, sid, &m1, |dh| {
                roster.member_by_dh(dh).filter(|r| r.id != me).cloned()
            });
            match res {
                Ok((rec, l, reply)) => {
                    let peer = rec.id;
                    if let Some(st) = self.lan_conns.get_mut(&conn_id) {
                        st.peer = Some(peer);
                    }
                    let was_up = self.lan_up(&peer);
                    if let Some((old, _)) = self.lan_links.insert(peer, (conn_id, l))
                        && old != conn_id
                    {
                        self.lan_conns.remove(&old);
                    }
                    self.send_lan_frame(conn_id, reply);
                    self.on_path_up(peer, true, was_up).await;
                }
                Err(e) => {
                    debug!(error = %e, "局域网入站握手被拒绝");
                    self.lan_conns.remove(&conn_id);
                }
            }
            return;
        };
        let Some(rec) = self.roster.member(&peer).cloned() else {
            self.lan_conns.remove(&conn_id);
            return;
        };
        match f.kind {
            Some(Kind::HandshakeResp(m2)) => {
                let ok = matches!(self.lan_links.get(&peer), Some((c, Link::Initiating { sid: s, .. })) if *c == conn_id && *s == sid);
                if !ok {
                    return;
                }
                let (_, l) = self.lan_links.remove(&peer).expect("上面已确认存在");
                match link::finish(l, &rec, &m2) {
                    Ok(up) => {
                        self.lan_links.insert(peer, (conn_id, up));
                        self.on_path_up(peer, true, false).await;
                    }
                    Err(e) => {
                        warn!(peer = %peer.short(), error = %e, "局域网握手失败");
                        self.lan_conns.remove(&conn_id);
                    }
                }
            }
            Some(Kind::Data(record)) => {
                let opened = match self.lan_links.get_mut(&peer) {
                    Some((c, Link::Up { session, sid: s })) if *c == conn_id && *s == sid => session.open(&record),
                    _ => return,
                };
                self.lan_rx_at.insert(peer, Instant::now());
                self.lan_probes.remove(&peer);
                match opened {
                    Ok(Some(bytes)) => self.on_app_bytes(peer, &bytes).await,
                    Ok(None) => {}
                    Err(e) => {
                        warn!(peer = %peer.short(), error = %e, "局域网会话解密失败，断开");
                        self.drop_lan(peer).await;
                    }
                }
            }
            Some(Kind::Close(_)) if self.lan_links.get(&peer).is_some_and(|(c, _)| *c == conn_id) => {
                self.drop_lan(peer).await;
            }
            _ => {}
        }
    }

    fn send_lan_frame(&mut self, conn_id: u64, f: PeerFrame) -> bool {
        match self.lan_conns.get(&conn_id) {
            Some(st) => st.tx.try_send(f).is_ok(),
            None => false,
        }
    }

    async fn send_lan_hint(&mut self, peer: DeviceId) {
        let Some(ep) = &self.lan_ep else { return };
        let port = ep.port();
        let ips: Vec<Ipv4Addr> =
            if self.cfg.lan.loopback_for_tests { vec![Ipv4Addr::LOCALHOST] } else { tlan::local_lan_ips() };
        if ips.is_empty() {
            return;
        }
        let hint = app::LanHint { addrs: ips.iter().map(ToString::to_string).collect(), port: u32::from(port) };
        self.send_app(peer, AppMessage::new(A::LanHint(hint))).await;
    }

    fn on_lan_hint(&mut self, peer: DeviceId, h: &app::LanHint) {
        let Ok(port) = u16::try_from(h.port) else { return };
        if port == 0 {
            return;
        }
        let allow_lo = self.cfg.lan.loopback_for_tests;
        let addrs: Vec<SocketAddr> = h
            .addrs
            .iter()
            .take(MAX_LAN_HINTS)
            .filter_map(|a| a.parse::<Ipv4Addr>().ok())
            .filter(|ip| tlan::is_lan_ip(*ip, allow_lo))
            .map(|ip| SocketAddr::new(IpAddr::V4(ip), port))
            .collect();
        if addrs.is_empty() {
            self.lan_hints.remove(&peer);
            self.lan_hint_at.remove(&peer);
        } else {
            self.lan_hints.insert(peer, addrs);
            self.lan_hint_at.entry(peer).or_insert_with(Instant::now);
            self.lan_last_dial.remove(&peer);
            self.maybe_dial(peer);
        }
    }

    fn maybe_dial(&mut self, peer: DeviceId) {
        let my_turn = self.me < peer || self.lan_hint_at.get(&peer).is_some_and(|t| t.elapsed() >= LAN_FALLBACK_DIAL);
        if !my_turn || self.revoked || self.lan_up(&peer) || self.lan_dialing.contains(&peer) {
            return;
        }
        if self
            .lan_links
            .get(&peer)
            .is_some_and(|(_, l)| matches!(l, Link::Initiating { since, .. } if since.elapsed() < LAN_AUTH_TIMEOUT))
        {
            return;
        }
        let (Some(ep), Some(addrs)) = (self.lan_ep.clone(), self.lan_hints.get(&peer).cloned()) else { return };
        if self.lan_last_dial.get(&peer).is_some_and(|t| t.elapsed() < LAN_REDIAL) {
            return;
        }
        self.lan_last_dial.insert(peer, Instant::now());
        self.lan_dialing.insert(peer);
        let conn_id = self.lan_ids.fetch_add(1, Ordering::Relaxed);
        lan::spawn_dial(ep, peer, addrs, conn_id, self.lan_tx.clone());
    }

    async fn on_beacon(&mut self, b: Beacon, from: SocketAddr) {
        let SocketAddr::V4(src) = from else { return };
        if self.revoked || b.port == 0 || !tlan::is_lan_ip(*src.ip(), self.cfg.lan.loopback_for_tests) {
            return;
        }

        let bucket = lan::bucket(now_ms());
        if b.bucket.abs_diff(bucket) > 1 {
            return;
        }
        let (me, key) = (self.me, self.disc_key);
        let Some(peer) = self
            .roster
            .members()
            .map(|m| m.id)
            .find(|id| *id != me && kdf::ct_eq(&lan::beacon_tag(&key, b.bucket, id), &b.tag))
        else {
            return;
        };
        let addr = SocketAddr::new(IpAddr::V4(*src.ip()), b.port);
        let hints = self.lan_hints.entry(peer).or_default();
        if !hints.contains(&addr) {
            debug!(peer = %peer.short(), %addr, "收到局域网信标");
            hints.insert(0, addr);
            hints.truncate(MAX_LAN_HINTS);
            self.lan_last_dial.remove(&peer);
        }
        self.lan_hint_at.entry(peer).or_insert_with(Instant::now);

        if b.want_reply
            && !self.lan_up(&peer)
            && self.disc_replied.get(&peer).is_none_or(|t| t.elapsed() >= BEACON_REPLY_GAP)
            && let (Some(d), Some(ep)) = (self.disc.clone(), &self.lan_ep)
        {
            self.disc_replied.insert(peer, Instant::now());
            let reply = Beacon { want_reply: false, port: ep.port(), bucket, tag: lan::beacon_tag(&key, bucket, &me) };
            d.send_to(&reply.encode(), from).await;
        }
        self.maybe_dial(peer);
    }

    async fn maybe_announce(&mut self) {
        let Some(d) = self.disc.clone() else { return };
        let Some(port) = self.lan_ep.as_ref().map(|e| e.port()) else { return };
        if self.revoked || Instant::now() < self.disc_next {
            return;
        }
        self.disc_next = Instant::now()
            + if self.disc_burst > 0 {
                self.disc_burst -= 1;
                ANNOUNCE_BURST_GAP
            } else {
                ANNOUNCE_EVERY
            };
        if !self.roster.members().any(|m| m.id != self.me && !self.lan_up(&m.id)) {
            return;
        }
        let bucket = lan::bucket(now_ms());
        let b =
            Beacon { want_reply: true, port, bucket, tag: lan::beacon_tag(&self.disc_key, bucket, &self.me) }.encode();
        if self.cfg.lan.discovery_targets.is_empty() {
            if !self.cfg.lan.loopback_for_tests {
                d.announce(&b, &discovery::lan_ifaces(), self.cfg.lan.discovery_port).await;
            }
        } else {
            for t in &self.cfg.lan.discovery_targets {
                d.send_to(&b, *t).await;
            }
        }
    }

    async fn on_path_up(&mut self, peer: DeviceId, lan: bool, was_up: bool) {
        let other_up = if lan { self.relay_up(&peer) } else { self.lan_up(&peer) };
        if was_up || other_up {
            info!(peer = %peer.short(), lan, was_up, "路径重新握手，重新交换进度");
            self.engine.peer_disconnected(&peer);
        } else {
            info!(peer = %peer.short(), lan, "加密会话已建立");
            self.emit(NodeEvent::PeerUp(peer)).await;
        }

        let hello = self.hello();
        self.send_app_via(peer, lan, hello).await;
        if !(was_up || other_up) && self.relay_conf.version > 0 {
            let msg = self.relay_conf_msg();
            self.send_app(peer, msg).await;
        }
        if lan && !was_up {
            info!(peer = %peer.short(), "局域网直连已建立");
            self.emit(NodeEvent::LanPath { peer, up: true }).await;
        }
        let fx = self.engine.peer_connected(peer);
        self.run_effects(fx).await;
        if !lan {
            self.send_lan_hint(peer).await;
        }
        self.files_path_changed(peer).await;
        self.fetch_peer_up().await;
    }

    async fn after_path_down(&mut self, peer: DeviceId) {
        self.engine.peer_disconnected(&peer);
        if self.peer_up(&peer) {
            let fx = self.engine.peer_connected(peer);
            self.run_effects(fx).await;
        } else {
            self.emit(NodeEvent::PeerDown(peer)).await;
        }
        Box::pin(self.files_path_changed(peer)).await;
    }

    async fn drop_link(&mut self, peer: DeviceId) {
        if let Some(l) = self.links.remove(&peer)
            && l.is_up()
        {
            info!(peer = %peer.short(), "中转会话已断开");
            self.after_path_down(peer).await;
        }
    }

    async fn drop_lan(&mut self, peer: DeviceId) {
        self.lan_probes.remove(&peer);
        if let Some((conn_id, l)) = self.lan_links.remove(&peer) {
            self.lan_conns.remove(&conn_id);
            if l.is_up() {
                self.lan_hint_at.insert(peer, Instant::now());
                info!(peer = %peer.short(), "局域网直连已断开");
                self.emit(NodeEvent::LanPath { peer, up: false }).await;
                self.after_path_down(peer).await;
            }
        }
    }

    async fn on_app_bytes(&mut self, peer: DeviceId, bytes: &[u8]) {
        match AppMessage::decode(bytes) {
            Ok(msg) => self.on_app(peer, msg).await,
            Err(_) => warn!(peer = %peer.short(), "应用消息解码失败"),
        }
    }

    async fn on_app(&mut self, peer: DeviceId, msg: AppMessage) {
        if let Some(
            body @ (A::FileOffer(_)
            | A::FileAccept(_)
            | A::FileChunk(_)
            | A::FileChunkAck(_)
            | A::FileCancel(_)
            | A::FileDone(_)),
        ) = msg.body
        {
            return self.on_file_msg(peer, body).await;
        }
        match msg.body {
            Some(A::ClipFetch(f)) => return self.on_clip_fetch(peer, f).await,
            Some(A::ClipData(d)) => return self.on_clip_data(peer, d).await,
            _ => {}
        }
        match &msg.body {
            Some(A::Hello(h)) => {
                let caps = h.capabilities.iter().take(32).filter(|c| c.len() <= 64).cloned().collect();
                self.peer_caps.insert(peer, caps);
                if h.roster_digest != self.roster.digest() {
                    let ops = self.roster.ops_raw();
                    self.send_app(peer, AppMessage::new(A::RosterOps(app::RosterOps { ops }))).await;
                }
            }
            Some(A::RosterOps(r)) => {
                let ops = r.ops.clone();
                self.merge_roster(&ops).await;
            }
            Some(A::HistoryClear(c)) => {
                info!(peer = %peer.short(), all = c.all, "对方请求删除历史");
                let fx = if c.all {
                    self.engine.forget_all()
                } else {
                    c.items
                        .iter()
                        .take(1000)
                        .filter_map(|i| <[u8; 16]>::try_from(i.as_slice()).ok())
                        .flat_map(|i| self.engine.forget(&i))
                        .collect()
                };
                self.run_effects(fx).await;
            }
            Some(A::LanHint(h)) => {
                let h = h.clone();
                self.on_lan_hint(peer, &h);
            }
            Some(A::RelayConfig(c)) => {
                let c = c.clone();
                self.on_relay_config(peer, &c).await;
            }
            Some(A::Ping(p)) => {
                let pong = AppMessage::new(A::Pong(p.clone()));
                self.send_app(peer, pong).await;
            }
            _ => {
                if let Some(A::ClipBatch(b)) = &msg.body
                    && !b.items.is_empty()
                {
                    info!(peer = %peer.short(), items = b.items.len(), "收到剪贴板条目");
                }
                let fx = self.engine.on_message(peer, msg, now_ms());
                self.run_effects(fx).await;
            }
        }
    }

    async fn send_app(&mut self, peer: DeviceId, msg: AppMessage) {
        let lan = self.lan_up(&peer);
        self.send_app_via(peer, lan, msg).await;
    }

    async fn send_app_via(&mut self, peer: DeviceId, lan: bool, msg: AppMessage) {
        let expect_reply = !matches!(msg.body, Some(A::Pong(_)) | Some(A::Ping(_)));
        let bytes = msg.encode_to_vec();
        if lan {
            let Some((conn_id, Link::Up { session, sid })) = self.lan_links.get_mut(&peer) else { return };
            let (conn_id, sid) = (*conn_id, *sid);
            match session.seal(&bytes) {
                Ok(records) => {
                    let ok =
                        records.into_iter().all(|rec| self.send_lan_frame(conn_id, link::frame(sid, Kind::Data(rec))));
                    if ok {
                        if expect_reply {
                            self.lan_probes.entry(peer).or_insert(Probe { since: Instant::now(), pinged: None });
                        }
                        return;
                    }

                    warn!(peer = %peer.short(), "局域网发送失败，切回中转");
                    Box::pin(self.drop_lan(peer)).await;
                    return;
                }
                Err(e) => {
                    warn!(error = %e, "加密失败");
                    return;
                }
            }
        }
        let (records, sid) = match self.links.get_mut(&peer) {
            Some(Link::Up { session, sid }) => match session.seal(&bytes) {
                Ok(r) => (r, *sid),
                Err(e) => {
                    warn!(error = %e, "加密失败");
                    return;
                }
            },
            _ => return,
        };
        for rec in records {
            self.send_relay_frame(&peer.0, link::frame(sid, Kind::Data(rec))).await;
        }
    }

    async fn probe_lan(&mut self, peer: DeviceId, now: bool) {
        if !self.lan_up(&peer) {
            return;
        }
        let p = self.lan_probes.entry(peer).or_insert(Probe { since: Instant::now(), pinged: None });
        if now && p.pinged.is_none() {
            p.pinged = Some(Instant::now());
            let ping = AppMessage::new(A::Ping(app::Ping { nonce: random_nonce() }));
            Box::pin(self.send_app(peer, ping)).await;
        }
    }

    async fn probe_all_lan(&mut self) {
        let peers: Vec<DeviceId> = self.lan_links.iter().filter(|(_, (_, l))| l.is_up()).map(|(p, _)| *p).collect();
        for p in peers {
            self.probe_lan(p, true).await;
        }
    }

    async fn lan_probe_tick(&mut self) {
        let mut ping = Vec::new();
        let mut dead = Vec::new();
        for (peer, p) in &self.lan_probes {
            match p.pinged {
                None if p.since.elapsed() >= PROBE_AFTER => ping.push(*peer),
                Some(t) if t.elapsed() >= PROBE_TIMEOUT => dead.push(*peer),
                _ => {}
            }
        }
        for p in ping {
            self.probe_lan(p, true).await;
        }
        for p in dead {
            warn!(peer = %p.short(), "局域网路径没有回应，切回中转");
            self.drop_lan(p).await;
        }
    }

    async fn send_relay_frame(&self, to: &[u8], f: PeerFrame) {
        if let Some(r) = &self.relay {
            let _ = r.send_to(to, f.encode_to_vec()).await;
        }
    }

    async fn run_effects(&mut self, fx: Vec<Effect>) {
        if fx.is_empty() {
            return;
        }
        if let Err(e) = self.db().apply_effects(&fx) {
            warn!(error = %e, "写入存储失败");
        }
        self.dirty = true;
        let mut history_changed = false;
        for e in fx {
            match e {
                Effect::Send { to, msg } => {
                    if let Some(A::ClipBatch(b)) = &msg.body
                        && !b.items.is_empty()
                    {
                        info!(peer = %to.short(), items = b.items.len(), lan = self.lan_up(&to), relay = self.relay_up(&to), "发送剪贴板条目");
                    }
                    Box::pin(self.send_app(to, msg)).await
                }
                Effect::Apply(item) => Box::pin(self.apply_item(item)).await,
                Effect::Store(_) | Effect::Forget(_) => history_changed = true,
            }
        }
        if history_changed {
            self.emit(NodeEvent::HistoryChanged).await;
        }
    }

    async fn send_pair_frame(&mut self, via: &PairVia, f: PeerFrame) {
        match via {
            PairVia::Relay(temp_id) => self.send_relay_frame(temp_id, f).await,
            PairVia::Lan(conn_id) => {
                self.send_lan_frame(*conn_id, f);
            }
        }
    }

    fn pairing_conn(&self) -> Option<u64> {
        match self.invite.as_ref()?.joiner.as_ref()?.via() {
            PairVia::Lan(c) => Some(*c),
            PairVia::Relay(_) => None,
        }
    }

    async fn on_pairing_frame(&mut self, via: PairVia, f: PeerFrame) {
        let sid = f.session_id;
        let Some(inv) = self.invite.as_mut() else { return };
        if inv.expires < Instant::now() {
            self.invite = None;
            return;
        }
        match (f.kind, inv.joiner.take()) {
            (Some(Kind::PairInit(m1)), None) => {
                let Some(own) = self.roster.member(&self.me).cloned() else { return };
                match pairing::respond(&self.identity, &own, &inv.secret, &m1) {
                    Ok((pending, m2)) => {
                        inv.joiner = Some(JoinerState::Handshaking { via: via.clone(), sid, pending });
                        self.send_pair_frame(&via, link::frame(sid, Kind::PairResp(m2))).await;
                    }
                    Err(e) => warn!(error = %e, "配对握手失败"),
                }
            }
            (Some(Kind::Data(rec)), Some(JoinerState::Handshaking { via: v, sid: s, pending }))
                if v == via && s == sid =>
            {
                match pending.confirm(&rec) {
                    Ok((session, req)) => {
                        if self.roster.is_revoked(&req.record.id) {
                            warn!("被吊销的设备尝试重新加入，已拒绝");
                            self.invite = None;
                            return;
                        }
                        let ev = NodeEvent::PairRequest {
                            name: req.record.name.clone(),
                            platform: req.record.platform,
                            sas: req.sas.clone(),
                        };
                        inv.joiner = Some(JoinerState::AwaitingConfirm { via, sid, session, record: req.record });
                        self.emit(ev).await;
                    }
                    Err(e) => {
                        warn!(error = %e, "配对确认失败，邀请已作废");
                        self.invite = None;
                    }
                }
            }
            (_, j) => inv.joiner = j,
        }
    }

    fn on_pairing_gone(&mut self, via: &PairVia) {
        if self.invite.as_ref().and_then(|i| i.joiner.as_ref()).is_some_and(|j| j.via() == via) {
            self.invite = None;
        }
    }

    async fn on_tick(&mut self) {
        let peers: Vec<DeviceId> = self.online.iter().copied().collect();
        for p in peers {
            self.maybe_initiate(p).await;
        }

        let pairing = self.pairing_conn();
        let stale: Vec<u64> = self
            .lan_conns
            .iter()
            .filter(|(id, st)| {
                st.since.elapsed() > LAN_AUTH_TIMEOUT
                    && Some(**id) != pairing
                    && !st.peer.is_some_and(|p| self.lan_links.get(&p).is_some_and(|(c, l)| c == *id && l.is_up()))
            })
            .map(|(id, _)| *id)
            .collect();
        for id in stale {
            self.lan_conns.remove(&id);
            let peer = self.lan_links.iter().find(|(_, (c, _))| *c == id).map(|(p, _)| *p);
            if let Some(p) = peer {
                self.drop_lan(p).await;
            }
        }
        self.lan_probe_tick().await;
        self.maybe_announce().await;

        let candidates: Vec<DeviceId> = self.lan_hints.keys().copied().collect();
        for p in candidates {
            self.maybe_dial(p);
        }
        if self.invite.as_ref().is_some_and(|i| i.expires < Instant::now()) {
            self.invite = None;
        }
        self.files_tick().await;
        self.fetch_tick().await;
        let fx = self.engine.tick(now_ms());
        for e in &fx {
            if let Effect::Send { to, .. } = e {
                warn!(peer = %to.short(), "一直没收到对方的进度向量（暂停向它推送），重新请求");
            }
        }
        self.run_effects(fx).await;
        if self.last_prune.elapsed() >= PRUNE_EVERY {
            self.last_prune = Instant::now();
            let fx = self.engine.prune(now_ms());
            self.run_effects(fx).await;
        }
        self.save_state();
    }
}

fn check_relay_addr(addr: &str) -> Result<String> {
    let addr = addr.trim();
    validate_addr(addr)
        .map_err(|_| CoreError::InvalidState("中转服务器地址应为“域名或 IP:端口”，例如 relay.example.com:8443"))?;
    Ok(addr.to_owned())
}

fn parse_relay_conf(c: &app::RelayConfig) -> Option<RelayConf> {
    let author = DeviceId::from_slice(&c.author).ok()?;
    if c.version == 0 {
        return None;
    }
    let relay = if c.addr.is_empty() {
        None
    } else {
        validate_addr(&c.addr).ok()?;
        Some(RelayAddr { addr: c.addr.clone(), key: c.key.as_slice().try_into().ok()? })
    };
    Some(RelayConf { relay, version: c.version, author: Some(author) })
}

fn random_nonce() -> u64 {
    copycat_crypto::random::u64().unwrap_or(0)
}

async fn sleep_until(t: Option<Instant>) {
    match t {
        Some(t) => tokio::time::sleep_until(t.into()).await,
        None => std::future::pending().await,
    }
}
