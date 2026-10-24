use std::sync::Arc;
use std::time::Duration;

use copycat_crypto::pairing::PairingSecret;
use copycat_crypto::{DeviceIdentity, DeviceRecord, Platform, Roster};
use copycat_proto::relay::{self, client_message::Body as C, error_code};
use copycat_relay::{Limits, RelayServer};
use copycat_transport::relay_client::{self, AuthMode, RelayAddr, RelayEvent, RelayHandle};
use copycat_transport::tls::{RelayKey, Tuning, client_endpoint};
use tokio::sync::mpsc::Receiver;

struct Dev {
    id: Arc<DeviceIdentity>,
    rec: DeviceRecord,
}

fn dev(name: &str) -> Dev {
    let id = DeviceIdentity::generate().unwrap();
    let rec = DeviceRecord::issue(&id, name, Platform::Windows, 1).unwrap();
    Dev { id: Arc::new(id), rec }
}

async fn start_relay(group: [u8; 32]) -> RelayAddr {
    let key = RelayKey::generate(group).unwrap();
    let server = RelayServer::bind_ephemeral("127.0.0.1:0".parse().unwrap(), &key, Limits::default()).unwrap();
    let addr = RelayAddr { addr: server.local_addr().unwrap().to_string(), key: server.public_key() };
    assert_eq!(addr.key, key.public());
    tokio::spawn(server.run());
    addr
}

fn auth(d: &Dev) -> AuthMode {
    AuthMode::Device { identity: d.id.clone(), roster_ops: vec![] }
}

async fn connect(
    relay: &RelayAddr,
    auth: AuthMode,
) -> copycat_transport::Result<(RelayHandle, Receiver<RelayEvent>, relay_client::Session)> {
    let ep = client_endpoint(relay.key, &Tuning::default()).unwrap();
    relay_client::connect(&ep, relay, &Tuning::default(), auth).await
}

async fn next_event(rx: &mut Receiver<RelayEvent>, pred: impl Fn(&RelayEvent) -> bool) -> RelayEvent {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let ev = rx.recv().await.expect("事件通道已关闭");
            if pred(&ev) {
                return ev;
            }
        }
    })
    .await
    .expect("等待事件超时")
}

struct Group {
    relay: RelayAddr,
    a: Dev,
    b: Dev,
    roster: Roster,
    ha: RelayHandle,
    ra: Receiver<RelayEvent>,
}

async fn setup() -> Group {
    let (a, b) = (dev("a"), dev("b"));
    let (mut roster, genesis) = Roster::create(&a.id, &a.rec, 1).unwrap();
    let relay = start_relay(*roster.group_id()).await;
    let (ha, mut ra, _) =
        connect(&relay, AuthMode::Device { identity: a.id.clone(), roster_ops: vec![genesis] }).await.unwrap();
    next_event(&mut ra, |e| matches!(e, RelayEvent::Roster(_))).await;
    let op = roster.add(&a.id, &b.rec, 2).unwrap();
    ha.send(C::RosterPush(relay::RosterPush { ops: vec![op] })).await.unwrap();
    next_event(&mut ra, |e| matches!(e, RelayEvent::Roster(ops) if ops.len() == 2)).await;
    Group { relay, a, b, roster, ha, ra }
}

#[tokio::test]
async fn first_device_registers_and_forward() {
    let Group { relay, a, b, ha, mut ra, .. } = setup().await;

    let (hb, mut rb, sess) = connect(&relay, auth(&b)).await.unwrap();
    assert_eq!(sess.online, vec![a.id.id()]);
    next_event(&mut ra, |e| matches!(e, RelayEvent::Presence { online: true, .. })).await;

    ha.send_to(&b.id.id().0, b"hello b".to_vec()).await.unwrap();
    let ev = next_event(&mut rb, |e| matches!(e, RelayEvent::Deliver { .. })).await;
    let RelayEvent::Deliver { from, payload } = ev else { unreachable!() };
    assert_eq!(from, a.id.id().0.to_vec(), "from 字段必须由服务器按认证身份填写");
    assert_eq!(payload, b"hello b");

    hb.close();
    next_event(&mut ra, |e| matches!(e, RelayEvent::Presence { online: false, .. })).await;
    ha.send_to(&b.id.id().0, b"x".to_vec()).await.unwrap();
    let ev = next_event(&mut ra, |e| matches!(e, RelayEvent::Error { .. })).await;
    assert!(
        matches!(ev, RelayEvent::Error { code: error_code::PEER_OFFLINE, ref context, .. } if *context == b.id.id().0.to_vec())
    );
}

#[tokio::test]
async fn wrong_server_key_rejected() {
    let g = setup().await;
    let mut relay = g.relay.clone();
    relay.key[0] ^= 1;
    let r = connect(&relay, auth(&g.a)).await;
    assert!(r.is_err(), "服务器公钥不匹配必须拒绝连接");
}

#[tokio::test]
async fn foreign_group_cannot_register() {
    let a = dev("a");
    let (roster, _) = Roster::create(&a.id, &a.rec, 1).unwrap();
    let relay = start_relay(*roster.group_id()).await;
    let x = dev("x");
    let (_, genesis) = Roster::create(&x.id, &x.rec, 1).unwrap();
    let r = connect(&relay, AuthMode::Device { identity: x.id.clone(), roster_ops: vec![genesis] }).await;
    assert!(matches!(r, Err(copycat_transport::TransportError::Rejected(_))));

    let r = connect(&relay, auth(&a)).await;
    assert!(matches!(r, Err(copycat_transport::TransportError::Rejected(_))));
}

#[tokio::test]
async fn non_member_rejected() {
    let g = setup().await;
    let stranger = dev("stranger");
    let r = connect(&g.relay, auth(&stranger)).await;
    assert!(matches!(r, Err(copycat_transport::TransportError::Rejected(_))));
}

#[tokio::test]
async fn device_added_elsewhere_admitted_with_its_roster() {
    let mut g = setup().await;
    let c = dev("c");
    g.roster.add(&g.a.id, &c.rec, 3).unwrap();
    assert!(connect(&g.relay, auth(&c)).await.is_err(), "服务器还不认识 c");
    let (_hc, _rc, sess) =
        connect(&g.relay, AuthMode::Device { identity: c.id.clone(), roster_ops: g.roster.ops_raw() }).await.unwrap();
    assert_eq!(sess.online, vec![g.a.id.id()]);

    next_event(&mut g.ra, |e| matches!(e, RelayEvent::Roster(ops) if ops.len() == 3)).await;
}

#[tokio::test]
async fn roster_persists_and_key_change_resets_it() {
    let dir = std::env::temp_dir().join(format!("copycat-relay-test-{}", copycat_crypto::random::u64().unwrap()));
    let a = dev("a");
    let (roster, genesis) = Roster::create(&a.id, &a.rec, 1).unwrap();
    let key = RelayKey::generate(*roster.group_id()).unwrap();
    let start = |key: &RelayKey| {
        let server = RelayServer::bind("127.0.0.1:0".parse().unwrap(), &dir, key, Limits::default()).unwrap();
        let addr = RelayAddr { addr: server.local_addr().unwrap().to_string(), key: server.public_key() };
        (tokio::spawn(server.run()), addr)
    };

    let (task, relay) = start(&key);
    let (h, _r, _) =
        connect(&relay, AuthMode::Device { identity: a.id.clone(), roster_ops: vec![genesis] }).await.unwrap();
    h.close();
    task.abort();

    let (task, relay) = start(&key);
    let (h, _r, _) = connect(&relay, auth(&a)).await.unwrap();
    h.close();
    task.abort();

    let x = dev("x");
    let (other, _) = Roster::create(&x.id, &x.rec, 1).unwrap();
    let (task, relay) = start(&RelayKey::generate(*other.group_id()).unwrap());
    assert!(connect(&relay, auth(&a)).await.is_err());
    task.abort();
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn revoked_device_is_kicked_and_cannot_reconnect() {
    let Group { relay, a, b, mut roster, ha, ra: _ra } = setup().await;
    let (_hb, mut rb, _) = connect(&relay, auth(&b)).await.unwrap();

    let op = roster.remove(&a.id, b.id.id(), 3).unwrap();
    ha.send(C::RosterPush(relay::RosterPush { ops: vec![op] })).await.unwrap();
    let ev = next_event(&mut rb, |e| matches!(e, RelayEvent::Error { .. })).await;
    assert!(matches!(ev, RelayEvent::Error { code: error_code::REVOKED, .. }), "被吊销的设备应收到明确的通知");
    next_event(&mut rb, |e| matches!(e, RelayEvent::Closed(_))).await;

    let r = connect(&relay, auth(&b)).await;
    assert!(r.is_err(), "被吊销的设备不能重新连接");

    let r = connect(&relay, AuthMode::Device { identity: b.id.clone(), roster_ops: roster.ops_raw() }).await;
    assert!(r.is_err());
}

#[tokio::test]
async fn forged_roster_push_ignored() {
    let Group { relay, ha, mut ra, .. } = setup().await;

    let (x, y) = (dev("x"), dev("y"));
    let (mut foreign, _) = Roster::create(&x.id, &x.rec, 1).unwrap();
    let forged = foreign.add(&x.id, &y.rec, 2).unwrap();
    ha.send(C::RosterPush(relay::RosterPush { ops: foreign.ops_raw().into_iter().chain([forged.clone()]).collect() }))
        .await
        .unwrap();
    ha.send(C::Ping(relay::Ping { nonce: 7 })).await.unwrap();
    next_event(&mut ra, |e| matches!(e, RelayEvent::Pong(7))).await;
    for d in [&x, &y] {
        let r = connect(&relay, auth(d)).await;
        assert!(r.is_err(), "伪造名册中的设备不能通过认证");
    }

    let r = connect(&relay, AuthMode::Device { identity: y.id.clone(), roster_ops: foreign.ops_raw() }).await;
    assert!(r.is_err());
}

#[tokio::test]
async fn mailbox_store_fetch_ack() {
    let Group { relay, b, ha, ra: _ra, .. } = setup().await;
    ha.send(C::MailboxPut(relay::MailboxPut {
        to: b.id.id().0.to_vec(),
        msg_id: b"m1".to_vec(),
        payload: b"sealed".to_vec(),
        ttl_secs: 60,
    }))
    .await
    .unwrap();
    ha.send(C::Ping(relay::Ping { nonce: 1 })).await.unwrap();

    let (hb, mut rb, _) = connect(&relay, auth(&b)).await.unwrap();
    let ev = next_event(&mut rb, |e| matches!(e, RelayEvent::Mailbox(_))).await;
    let RelayEvent::Mailbox(items) = ev else { unreachable!() };
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].payload, b"sealed");

    hb.send(C::MailboxAck(relay::MailboxAck { msg_ids: vec![b"m1".to_vec()] })).await.unwrap();
    hb.send(C::MailboxFetch(relay::MailboxFetch {})).await.unwrap();
    hb.send(C::Ping(relay::Ping { nonce: 2 })).await.unwrap();

    let ev = next_event(&mut rb, |e| matches!(e, RelayEvent::Mailbox(_) | RelayEvent::Pong(2))).await;
    assert!(matches!(ev, RelayEvent::Pong(2)));
}

#[tokio::test]
async fn pairing_ticket_channel() {
    let Group { relay, a, ha, mut ra, .. } = setup().await;
    let secret = PairingSecret::generate().unwrap();
    ha.send(C::PairRegister(relay::PairRegister {
        ticket_id: secret.ticket_id().to_vec(),
        relay_auth: secret.relay_auth().to_vec(),
        ttl_secs: 120,
    }))
    .await
    .unwrap();
    ha.send(C::Ping(relay::Ping { nonce: 3 })).await.unwrap();
    next_event(&mut ra, |e| matches!(e, RelayEvent::Pong(3))).await;

    let bad = connect(&relay, AuthMode::PairJoin { ticket_id: secret.ticket_id(), relay_auth: [0u8; 32] }).await;
    assert!(bad.is_err());

    let (hj, mut rj, sess) =
        connect(&relay, AuthMode::PairJoin { ticket_id: secret.ticket_id(), relay_auth: secret.relay_auth() })
            .await
            .unwrap();
    let (peer, temp_id) = sess.pairing.unwrap();
    assert_eq!(peer, a.id.id());

    hj.send_to(&a.id.id().0, b"pair-init".to_vec()).await.unwrap();
    let ev = next_event(&mut ra, |e| matches!(e, RelayEvent::Deliver { .. })).await;
    let RelayEvent::Deliver { from, payload } = ev else { unreachable!() };
    assert_eq!(from, temp_id);
    assert_eq!(payload, b"pair-init");

    ha.send_to(&temp_id, b"pair-resp".to_vec()).await.unwrap();
    let ev = next_event(&mut rj, |e| matches!(e, RelayEvent::Deliver { .. })).await;
    assert!(matches!(ev, RelayEvent::Deliver { ref payload, .. } if payload == b"pair-resp"));

    hj.send_to(&[9u8; 16], b"x".to_vec()).await.unwrap();
    next_event(&mut rj, |e| matches!(e, RelayEvent::Error { code: error_code::BAD_REQUEST, .. })).await;

    let again =
        connect(&relay, AuthMode::PairJoin { ticket_id: secret.ticket_id(), relay_auth: secret.relay_auth() }).await;
    assert!(again.is_err());
}

use copycat_transport::tls::PathPref;

fn tuning(path: PathPref) -> Tuning {
    Tuning { path, quic_connect_timeout: Duration::from_millis(600), ..Tuning::default() }
}

async fn connect_via(
    relay: &RelayAddr,
    auth: AuthMode,
    path: PathPref,
) -> copycat_transport::Result<(RelayHandle, Receiver<RelayEvent>, relay_client::Session)> {
    let t = tuning(path);
    let ep = client_endpoint(relay.key, &t).unwrap();
    relay_client::connect(&ep, relay, &t, auth).await
}

#[tokio::test]
async fn tcp_path_full_flow_and_mixed_peers() {
    let (a, b) = (dev("a"), dev("b"));
    let (mut roster, genesis) = Roster::create(&a.id, &a.rec, 1).unwrap();
    let relay = start_relay(*roster.group_id()).await;

    let (ha, mut ra, sa) =
        connect_via(&relay, AuthMode::Device { identity: a.id.clone(), roster_ops: vec![genesis] }, PathPref::TcpOnly)
            .await
            .unwrap();
    assert!(!sa.quic);
    next_event(&mut ra, |e| matches!(e, RelayEvent::Roster(_))).await;
    let op = roster.add(&a.id, &b.rec, 2).unwrap();
    ha.send(C::RosterPush(relay::RosterPush { ops: vec![op] })).await.unwrap();
    next_event(&mut ra, |e| matches!(e, RelayEvent::Roster(ops) if ops.len() == 2)).await;

    let (hb, mut rb, sb) = connect_via(&relay, auth(&b), PathPref::QuicOnly).await.unwrap();
    assert!(sb.quic);
    ha.send_to(&b.id.id().0, b"tcp->quic".to_vec()).await.unwrap();
    let ev = next_event(&mut rb, |e| matches!(e, RelayEvent::Deliver { .. })).await;
    assert!(matches!(ev, RelayEvent::Deliver { ref payload, .. } if payload == b"tcp->quic"));
    hb.send_to(&a.id.id().0, b"quic->tcp".to_vec()).await.unwrap();
    let ev = next_event(&mut ra, |e| matches!(e, RelayEvent::Deliver { .. })).await;
    assert!(matches!(ev, RelayEvent::Deliver { ref payload, .. } if payload == b"quic->tcp"));

    let op = roster.remove(&b.id, a.id.id(), 3).unwrap();
    hb.send(C::RosterPush(relay::RosterPush { ops: vec![op] })).await.unwrap();
    next_event(&mut ra, |e| matches!(e, RelayEvent::Error { code: error_code::REVOKED, .. })).await;
    next_event(&mut ra, |e| matches!(e, RelayEvent::Closed(_))).await;
}

#[tokio::test]
async fn tcp_path_rejects_wrong_key_and_strangers() {
    let g = setup().await;
    let mut wrong = g.relay.clone();
    wrong.key[5] ^= 0x80;
    assert!(connect_via(&wrong, auth(&g.a), PathPref::TcpOnly).await.is_err());
    let x = dev("x");
    let r = connect_via(&g.relay, auth(&x), PathPref::TcpOnly).await;
    assert!(matches!(r, Err(copycat_transport::TransportError::Rejected(_))));
}

#[tokio::test]
async fn auto_falls_back_to_tcp_when_udp_blocked() {
    let Group { relay, a, ha: _ha, ra: _ra, .. } = setup().await;

    let proxy = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let proxy_addr = proxy.local_addr().unwrap();
    let target = relay.addr.clone();
    tokio::spawn(async move {
        loop {
            let (mut inbound, _) = proxy.accept().await.unwrap();
            let target = target.clone();
            tokio::spawn(async move {
                let mut outbound = tokio::net::TcpStream::connect(target).await.unwrap();
                let _ = tokio::io::copy_bidirectional(&mut inbound, &mut outbound).await;
            });
        }
    });

    let via_proxy = RelayAddr { addr: proxy_addr.to_string(), key: relay.key };
    let started = std::time::Instant::now();
    let (h, _rx, sess) = connect_via(&via_proxy, auth(&a), PathPref::Auto).await.expect("QUIC 不通时应自动回退到 TCP");
    assert!(!sess.quic, "应走 TCP 路径");
    assert!(started.elapsed() < Duration::from_secs(5));
    h.close();
}

#[tokio::test]
async fn tcp_idle_client_is_dropped_but_pinging_client_stays() {
    let (a, b) = (dev("a"), dev("b"));
    let (mut roster, genesis) = Roster::create(&a.id, &a.rec, 1).unwrap();
    let key = RelayKey::generate(*roster.group_id()).unwrap();
    let limits = Limits {
        tuning: Tuning { idle_timeout: Duration::from_millis(1500), ..Tuning::server() },
        ..Limits::default()
    };
    let server = RelayServer::bind_ephemeral("127.0.0.1:0".parse().unwrap(), &key, limits).unwrap();
    let relay = RelayAddr { addr: server.local_addr().unwrap().to_string(), key: server.public_key() };
    tokio::spawn(server.run());

    let tcp = |keep_alive| Tuning { path: PathPref::TcpOnly, keep_alive, ..Tuning::default() };
    let connect_tcp = |auth, t: Tuning| {
        let relay = relay.clone();
        async move {
            let ep = client_endpoint(relay.key, &t).unwrap();
            relay_client::connect(&ep, &relay, &t, auth).await.unwrap()
        }
    };

    let (ha, mut ra, _) = connect_tcp(
        AuthMode::Device { identity: a.id.clone(), roster_ops: vec![genesis] },
        tcp(Some(Duration::from_millis(300))),
    )
    .await;
    next_event(&mut ra, |e| matches!(e, RelayEvent::Roster(_))).await;
    let op = roster.add(&a.id, &b.rec, 2).unwrap();
    ha.send(C::RosterPush(relay::RosterPush { ops: vec![op] })).await.unwrap();
    next_event(&mut ra, |e| matches!(e, RelayEvent::Roster(ops) if ops.len() == 2)).await;
    let (_hb, _rb, _) = connect_tcp(auth(&b), tcp(None)).await;
    let b_id = b.id.id();
    next_event(&mut ra, |e| matches!(e, RelayEvent::Presence { device, online: true } if *device == b_id)).await;
    next_event(&mut ra, |e| matches!(e, RelayEvent::Presence { device, online: false } if *device == b_id)).await;
    tokio::time::sleep(Duration::from_secs(2)).await;
    assert!(!ha.is_closed(), "定期发 Ping 的客户端不应被断开");
}
