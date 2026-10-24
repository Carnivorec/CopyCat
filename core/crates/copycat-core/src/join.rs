use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use copycat_crypto::pairing::{Joiner, PairingInvite};
use copycat_crypto::{DeviceId, DeviceIdentity, DeviceRecord, Roster, kdf, random};
use copycat_proto::Message;
use copycat_proto::app::{AppMessage, app_message::Body as A};
use copycat_proto::peer::{PeerFrame, peer_frame::Kind};
use copycat_store::Store;
use copycat_transport::lan::{self as tlan, LanConn, LanEndpoint};
use copycat_transport::relay_client::{self, AuthMode, RelayAddr, RelayEvent, RelayHandle};
use copycat_transport::tls::client_endpoint;
use tokio::sync::mpsc;
use tracing::info;

use crate::link::frame;
use crate::{CoreError, Node, NodeConfig, NodeEvent, RelayConf, Result, now_ms, save_relay};

const CONFIRM_TIMEOUT: Duration = Duration::from_secs(180);
const STEP_TIMEOUT: Duration = Duration::from_secs(30);

const LAN_DIAL: Duration = Duration::from_secs(3);

enum Channel {
    Lan { conn: LanConn, _ep: Arc<LanEndpoint> },
    Relay { handle: RelayHandle, rx: mpsc::Receiver<RelayEvent>, inviter: DeviceId },
}

impl Channel {
    async fn send(&mut self, f: PeerFrame) -> Result<()> {
        match self {
            Channel::Lan { conn, .. } => conn.write.send(&f).await.map_err(|e| CoreError::Pairing(e.to_string())),
            Channel::Relay { handle, inviter, .. } => Ok(handle.send_to(&inviter.0, f.encode_to_vec()).await?),
        }
    }

    async fn recv(&mut self, timeout: Duration) -> Result<PeerFrame> {
        let gone = || CoreError::Pairing("连接已断开".into());
        tokio::time::timeout(timeout, async {
            match self {
                Channel::Lan { conn, .. } => conn.read.recv::<PeerFrame>().await.ok().flatten().ok_or_else(gone),
                Channel::Relay { rx, inviter, .. } => loop {
                    match rx.recv().await {
                        Some(RelayEvent::Deliver { from, payload }) if from == inviter.0 => {
                            if let Ok(pf) = PeerFrame::decode(payload.as_slice()) {
                                return Ok(pf);
                            }
                        }
                        Some(RelayEvent::Closed(r)) => return Err(CoreError::Pairing(format!("连接已断开: {r}"))),
                        None => return Err(gone()),
                        Some(_) => {}
                    }
                },
            }
        })
        .await
        .map_err(|_| CoreError::Pairing("等待对方响应超时".into()))?
    }

    fn close(&self) {
        match self {
            Channel::Lan { conn, .. } => conn.conn.close(0u32.into(), b""),
            Channel::Relay { handle, .. } => handle.close(),
        }
    }
}

fn lan_addrs(invite: &PairingInvite, allow_loopback: bool) -> Vec<SocketAddr> {
    invite
        .lan
        .iter()
        .filter_map(|a| a.parse::<SocketAddr>().ok())
        .filter(|a| matches!(a.ip(), IpAddr::V4(ip) if tlan::is_lan_ip(ip, allow_loopback)))
        .collect()
}

async fn dial_lan(addrs: &[SocketAddr], loopback: bool) -> Option<Channel> {
    if addrs.is_empty() {
        return None;
    }
    let ep = Arc::new(LanEndpoint::bind(0, loopback).ok()?);
    let (tx, mut rx) = mpsc::channel(addrs.len());
    for a in addrs {
        let (ep, tx, a) = (ep.clone(), tx.clone(), *a);
        tokio::spawn(async move {
            let _ = tx.send(ep.connect(a, LAN_DIAL).await).await;
        });
    }
    drop(tx);
    while let Some(r) = rx.recv().await {
        if let Ok(conn) = r {
            info!(addr = %conn.remote(), "已在局域网内连上邀请方");
            return Some(Channel::Lan { conn, _ep: ep });
        }
    }
    None
}

async fn dial_relay(invite: &PairingInvite, cfg: &NodeConfig) -> Result<Option<(Channel, RelayAddr)>> {
    let (Some(addr), Some(key)) = (invite.relay.clone(), invite.relay_key) else { return Ok(None) };
    let relay = RelayAddr { addr, key };
    let ticket_id = invite.secret.ticket_id();
    let relay_auth = invite.secret.relay_auth();
    let ep = client_endpoint(relay.key, &cfg.tuning)?;
    let (handle, rx, sess) =
        relay_client::connect(&ep, &relay, &cfg.tuning, AuthMode::PairJoin { ticket_id, relay_auth }).await?;
    let (peer, _temp_id) = sess.pairing.ok_or_else(|| CoreError::Pairing("服务器未建立配对通道".into()))?;
    if peer != invite.inviter_id {
        handle.close();
        return Err(CoreError::Pairing("配对通道指向了错误的设备".into()));
    }
    Ok(Some((Channel::Relay { handle, rx, inviter: peer }, relay)))
}

async fn pair(
    ch: &mut Channel,
    identity: &DeviceIdentity,
    record: &DeviceRecord,
    invite: PairingInvite,
    on_sas: impl FnOnce(&str),
) -> Result<Roster> {
    let inviter_id = invite.inviter_id;
    let sid = random::u64()?;
    let (joiner, m1) = Joiner::start(identity, invite, now_ms())?;
    ch.send(frame(sid, Kind::PairInit(m1))).await?;

    let f = ch.recv(STEP_TIMEOUT).await?;
    let Some(Kind::PairResp(m2)) = f.kind else {
        return Err(CoreError::Pairing("对方拒绝了配对".into()));
    };
    let out = joiner.finish(&m2, record)?;
    for rec in out.confirm {
        ch.send(frame(sid, Kind::Data(rec))).await?;
    }
    on_sas(&out.sas);

    let mut session = out.session;
    let ops = loop {
        let f = ch.recv(CONFIRM_TIMEOUT).await?;
        match f.kind {
            Some(Kind::Data(rec)) => {
                if let Some(bytes) = session.open(&rec)? {
                    match AppMessage::decode(bytes.as_slice()) {
                        Ok(AppMessage { body: Some(A::RosterOps(r)) }) => break r.ops,
                        _ => return Err(CoreError::Pairing("收到意外的消息".into())),
                    }
                }
            }
            Some(Kind::Close(_)) => return Err(CoreError::Pairing("对方拒绝了配对".into())),
            _ => {}
        }
    };
    let genesis = ops.first().ok_or_else(|| CoreError::Pairing("名册为空".into()))?;
    let roster = Roster::from_ops(&kdf::hash(genesis), ops.iter().map(Vec::as_slice))?;
    if !roster.is_member(&identity.id()) || !roster.is_member(&inviter_id) {
        return Err(CoreError::Pairing("收到的名册中不包含本机".into()));
    }
    Ok(roster)
}

pub(crate) async fn join(
    mut store: Store,
    cfg: NodeConfig,
    invite_uri: &str,
    on_sas: impl FnOnce(&str) + Send,
) -> Result<(Node, mpsc::Receiver<NodeEvent>)> {
    if store.load_identity()?.is_some() {
        return Err(CoreError::AlreadyInitialized);
    }
    let invite = PairingInvite::parse_uri(invite_uri)?;
    let inviter_id = invite.inviter_id;
    let identity = DeviceIdentity::generate()?;
    let record = DeviceRecord::issue(&identity, &cfg.device_name, cfg.platform, now_ms())?;

    let lan = if cfg.lan.enabled { lan_addrs(&invite, cfg.lan.loopback_for_tests) } else { Vec::new() };
    let invite_relay = invite.relay.clone().zip(invite.relay_key).map(|(addr, key)| RelayAddr { addr, key });
    let mut ch = match dial_lan(&lan, cfg.lan.loopback_for_tests).await {
        Some(ch) => ch,
        None => match dial_relay(&invite, &cfg).await? {
            Some((ch, _)) => ch,
            None if lan.is_empty() => {
                return Err(CoreError::Pairing("配对码中既没有局域网地址，也没有中转服务器".into()));
            }
            None => {
                return Err(CoreError::Pairing(
                    "连不上邀请方：请确认两台设备连着同一个局域网（Wi-Fi 未开启访客隔离），或先在邀请方配置中转服务器"
                        .into(),
                ));
            }
        },
    };
    let result = pair(&mut ch, &identity, &record, invite, on_sas).await;
    ch.close();
    let roster = result?;

    store.save_identity(&identity)?;
    store.set_group(roster.group_id())?;
    store.add_roster_ops(&roster.ops_raw())?;

    save_relay(&store, &RelayConf { relay: invite_relay, version: 0, author: None })?;
    let hints = if lan.is_empty() { Vec::new() } else { vec![(inviter_id, lan)] };
    Ok(crate::actor::spawn(store, cfg, Arc::new(identity), roster, hints))
}
