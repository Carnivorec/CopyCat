use std::path::PathBuf;
use std::time::Duration;

use copycat_core::{
    ClipItem, FileSource, Node, NodeConfig, NodeEvent, OutgoingFile, Platform, RelayKey, Rep, TransferInfo,
    TransferState,
};
use copycat_relay::{Limits, RelayServer};
use copycat_store::Store;
use copycat_transport::tls::PathPref;
use tokio::sync::mpsc::Receiver;

const WAIT: Duration = Duration::from_secs(15);

fn init_log() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "warn".into()))
        .with_test_writer()
        .try_init();
}

fn temp_db(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("copycat-e2e-{}-{name}-{}", std::process::id(), rand_suffix()));
    std::fs::create_dir_all(&dir).unwrap();
    dir.join("db.sqlite")
}

fn rand_suffix() -> u64 {
    copycat_crypto::random::u64().unwrap()
}

fn store(path: &PathBuf) -> Store {
    Store::open(path, &[42u8; 32]).unwrap()
}

async fn start_relay_for(a: &Node) -> String {
    let addr = format!("127.0.0.1:{}", free_udp_port());
    let text = a.setup_relay(addr.clone()).await.unwrap();
    assert!(text.starts_with("ccr1-"));
    let key = RelayKey::parse(&text).unwrap();
    let server = RelayServer::bind_ephemeral(addr.parse().unwrap(), &key, Limits::default()).unwrap();
    tokio::spawn(server.run());
    addr
}

async fn wait_for<T>(rx: &mut Receiver<NodeEvent>, what: &str, mut f: impl FnMut(&NodeEvent) -> Option<T>) -> T {
    tokio::time::timeout(WAIT, async {
        loop {
            let ev = rx.recv().await.unwrap_or_else(|| panic!("事件通道关闭，仍在等待: {what}"));
            if let Some(v) = f(&ev) {
                return v;
            }
        }
    })
    .await
    .unwrap_or_else(|_| panic!("等待超时: {what}"))
}

fn text(s: &str) -> Vec<Rep> {
    vec![Rep::from_bytes("text/plain", s.as_bytes().to_vec())]
}

fn text_of(it: &ClipItem) -> String {
    String::from_utf8(it.reps[0].inline.clone().unwrap()).unwrap()
}

async fn wait_apply(rx: &mut Receiver<NodeEvent>, expect: &str) -> ClipItem {
    let item = wait_for(rx, &format!("剪贴板内容 {expect}"), |e| match e {
        NodeEvent::Apply(i) if text_of(i) == expect => Some(i.clone()),
        _ => None,
    })
    .await;
    assert_eq!(text_of(&item), expect);
    item
}

struct Pair {
    relay: String,
    a: Node,
    ra: Receiver<NodeEvent>,
    a_db: PathBuf,
    b: Node,
    rb: Receiver<NodeEvent>,
    b_db: PathBuf,
}

fn cfg(name: &str, platform: Platform, path: PathPref) -> NodeConfig {
    let mut c = NodeConfig::new(name, platform);
    c.tuning.path = path;

    c.lan.port = 0;
    c.lan.loopback_for_tests = true;

    c.lan.discovery = false;
    c.cache_dir = temp_dir("cache");
    c
}

fn free_udp_port() -> u16 {
    std::net::UdpSocket::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port()
}

fn cfg_discovery(name: &str, platform: Platform, port: u16, peer: Option<u16>) -> NodeConfig {
    let mut c = cfg_default(name, platform);
    c.relay_enabled = false;
    c.lan.discovery = true;
    c.lan.discovery_port = port;
    c.lan.discovery_targets = peer.map(|p| format!("127.0.0.1:{p}").parse().unwrap()).into_iter().collect();
    c
}

fn cfg_default(name: &str, platform: Platform) -> NodeConfig {
    cfg(name, platform, PathPref::Auto)
}

async fn setup_pair() -> Pair {
    setup_pair_via(PathPref::Auto).await
}

async fn setup_pair_via(path: PathPref) -> Pair {
    init_log();
    let a_db = temp_db("a");
    let (a, mut ra) = Node::create_group(store(&a_db), cfg("台式机", Platform::Windows, path)).await.unwrap();
    let relay = start_relay_for(&a).await;
    wait_for(&mut ra, "A 连上中转", |e| matches!(e, NodeEvent::RelayConnected).then_some(())).await;

    let uri = a.create_invite().await.unwrap();
    assert!(uri.starts_with("copycat://pair?"));

    let b_db = temp_db("b");
    let (sas_tx, sas_rx) = tokio::sync::oneshot::channel::<String>();
    let b_store = store(&b_db);
    let join = tokio::spawn(async move {
        Node::join(b_store, cfg("手机", Platform::Android, path), &uri, move |sas| {
            let _ = sas_tx.send(sas.to_owned());
        })
        .await
    });

    let (name, sas_a) = wait_for(&mut ra, "配对请求", |e| match e {
        NodeEvent::PairRequest { name, sas, .. } => Some((name.clone(), sas.clone())),
        _ => None,
    })
    .await;
    assert_eq!(name, "手机");
    let sas_b = sas_rx.await.unwrap();
    assert_eq!(sas_a, sas_b, "两端确认码必须一致");
    let b_id = a.respond_pairing(true).await.unwrap();

    let (b, mut rb) = join.await.unwrap().unwrap();
    assert_eq!(b.id(), b_id);
    wait_for(&mut ra, "A 与 B 建立会话", |e| matches!(e, NodeEvent::PeerUp(id) if *id == b_id).then_some(())).await;
    wait_for(&mut rb, "B 与 A 建立会话", |e| matches!(e, NodeEvent::PeerUp(_)).then_some(())).await;
    Pair { relay, a, ra, a_db, b, rb, b_db }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn pair_and_sync_both_ways() {
    let mut p = setup_pair().await;
    assert_eq!(p.a.members().await.unwrap().len(), 2);
    assert_eq!(p.b.members().await.unwrap().len(), 2);

    p.a.copy(text("来自台式机的问候")).await.unwrap();
    let item = wait_apply(&mut p.rb, "来自台式机的问候").await;

    assert!(p.b.is_echo(item.reps[0].hash).await.unwrap());

    p.b.copy(text("手机上复制的验证码 482913")).await.unwrap();
    wait_apply(&mut p.ra, "手机上复制的验证码 482913").await;

    assert_eq!(p.a.history().await.unwrap().len(), 2);
    assert_eq!(p.b.history().await.unwrap().len(), 2);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn offline_device_catches_up_via_mailbox() {
    let mut p = setup_pair().await;

    p.b.shutdown().await;
    drop(p.rb);
    wait_for(&mut p.ra, "A 发现 B 离线", |e| matches!(e, NodeEvent::PeerDown(_)).then_some(())).await;

    p.a.copy(text("B 离线时复制的内容")).await.unwrap();

    tokio::time::sleep(Duration::from_millis(300)).await;
    p.a.shutdown().await;

    let (b, mut rb) = Node::open(store(&p.b_db), cfg_default("手机", Platform::Android)).await.unwrap();
    wait_apply(&mut rb, "B 离线时复制的内容").await;
    assert_eq!(b.history().await.unwrap().len(), 1);
    let _ = p.relay;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn reconnect_resyncs_after_restart() {
    let mut p = setup_pair().await;
    p.a.shutdown().await;
    drop(p.ra);
    wait_for(&mut p.rb, "B 发现 A 离线", |e| matches!(e, NodeEvent::PeerDown(_)).then_some(())).await;
    p.b.copy(text("A 重启期间的内容")).await.unwrap();

    let (a, mut ra) = Node::open(store(&p.a_db), cfg_default("台式机", Platform::Windows)).await.unwrap();
    wait_apply(&mut ra, "A 重启期间的内容").await;
    a.copy(text("重启后继续同步")).await.unwrap();
    wait_apply(&mut p.rb, "重启后继续同步").await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn removed_device_is_cut_off() {
    let mut p = setup_pair().await;
    let b_id = p.b.id();
    p.a.remove_device(b_id).await.unwrap();
    wait_for(&mut p.rb, "B 收到被移除通知", |e| matches!(e, NodeEvent::Revoked).then_some(())).await;
    assert!(p.b.copy(text("x")).await.is_err(), "被移除的设备不能再发送");
    assert_eq!(p.a.members().await.unwrap().len(), 1);

    p.a.copy(text("B 不应看到")).await.unwrap();
    let got = tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            match p.rb.recv().await {
                Some(NodeEvent::Apply(_)) => return true,
                Some(_) => {}
                None => return false,
            }
        }
    })
    .await;
    assert!(!matches!(got, Ok(true)), "被移除的设备收到了新内容");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn device_leaves_group() {
    let mut p = setup_pair().await;
    let b_id = p.b.id();
    let report = p.b.leave_group().await.unwrap();
    assert_eq!(report.peers, 1);
    assert!(report.relay);
    wait_for(&mut p.rb, "B 停止同步", |e| matches!(e, NodeEvent::Revoked).then_some(())).await;
    wait_for(&mut p.ra, "A 收到 B 退出", |e| matches!(e, NodeEvent::PeerDown(id) if *id == b_id).then_some(())).await;
    let members = p.a.members().await.unwrap();
    assert!(members.iter().all(|m| m.id != b_id), "A 的名册里仍有 B");
    assert!(p.b.copy(text("x")).await.is_err(), "退出后不能再发送");

    assert_eq!(p.b.leave_group().await.unwrap(), copycat_core::LeaveReport { peers: 0, relay: false });
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn offline_device_learns_about_leave_from_relay() {
    let mut p = setup_pair().await;
    let b_id = p.b.id();
    p.a.shutdown().await;
    drop(p.ra);
    wait_for(&mut p.rb, "B 发现 A 离线", |e| matches!(e, NodeEvent::PeerDown(_)).then_some(())).await;
    let report = p.b.leave_group().await.unwrap();
    assert_eq!(report.peers, 0);
    assert!(report.relay);

    let (a, mut ra) = Node::open(store(&p.a_db), cfg_default("台式机", Platform::Windows)).await.unwrap();
    wait_for(&mut ra, "A 从中转得知 B 退出", |e| matches!(e, NodeEvent::RosterChanged).then_some(())).await;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while a.members().await.unwrap().iter().any(|m| m.id == b_id) {
        assert!(tokio::time::Instant::now() < deadline, "A 的名册里仍有 B");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let _ = p.relay;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn rejected_pairing_does_not_join() {
    init_log();
    let (a, mut ra) = Node::create_group(store(&temp_db("ra")), cfg_default("pc", Platform::Windows)).await.unwrap();
    let uri = a.create_invite().await.unwrap();
    let b_store = store(&temp_db("rb"));
    let join =
        tokio::spawn(
            async move { Node::join(b_store, cfg_default("陌生设备", Platform::Android), &uri, |_| {}).await },
        );
    wait_for(&mut ra, "配对请求", |e| matches!(e, NodeEvent::PairRequest { .. }).then_some(())).await;
    assert!(a.respond_pairing(false).await.is_err());
    assert!(join.await.unwrap().is_err());
    assert_eq!(a.members().await.unwrap().len(), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn lan_only_group_pair_and_sync() {
    init_log();
    let (a, mut ra) =
        Node::create_group(store(&temp_db("la")), cfg_default("台式机", Platform::Windows)).await.unwrap();
    let uri = a.create_invite().await.unwrap();
    assert!(uri.contains("&lan=127.0.0.1:") && !uri.contains("relay="), "二维码只带局域网地址：{uri}");
    let b_store = store(&temp_db("lb"));
    let join =
        tokio::spawn(async move { Node::join(b_store, cfg_default("手机", Platform::Android), &uri, |_| {}).await });
    wait_for(&mut ra, "配对请求", |e| matches!(e, NodeEvent::PairRequest { .. }).then_some(())).await;
    let b_id = a.respond_pairing(true).await.unwrap();
    let (b, mut rb) = join.await.unwrap().unwrap();
    wait_for(&mut ra, "A 与 B 局域网直连", |e| {
        matches!(e, NodeEvent::LanPath { peer, up: true } if *peer == b_id).then_some(())
    })
    .await;
    let st = b.status().await.unwrap();
    assert!(st.relay_addr.is_none() && !st.relay_connected);

    a.copy(text("没有服务器也能用")).await.unwrap();
    wait_apply(&mut rb, "没有服务器也能用").await;
    b.copy(text("手机发回电脑")).await.unwrap();
    wait_apply(&mut ra, "手机发回电脑").await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn relay_added_later_reaches_other_devices() {
    init_log();
    let (a, mut ra) =
        Node::create_group(store(&temp_db("ra2")), cfg_default("台式机", Platform::Windows)).await.unwrap();
    let uri = a.create_invite().await.unwrap();
    let b_db = temp_db("rb2");
    let b_store = store(&b_db);
    let join =
        tokio::spawn(async move { Node::join(b_store, cfg_default("手机", Platform::Android), &uri, |_| {}).await });
    wait_for(&mut ra, "配对请求", |e| matches!(e, NodeEvent::PairRequest { .. }).then_some(())).await;
    let b_id = a.respond_pairing(true).await.unwrap();
    let (b, mut rb) = join.await.unwrap().unwrap();
    wait_for(&mut ra, "A 与 B 建立会话", |e| matches!(e, NodeEvent::PeerUp(id) if *id == b_id).then_some(())).await;

    let relay = start_relay_for(&a).await;
    wait_for(&mut rb, "B 收到中转配置", |e| matches!(e, NodeEvent::RelayChanged).then_some(())).await;
    assert_eq!(b.status().await.unwrap().relay_addr.as_deref(), Some(relay.as_str()));
    wait_for(&mut ra, "A 连上中转", |e| matches!(e, NodeEvent::RelayConnected).then_some(())).await;
    wait_for(&mut rb, "B 连上中转", |e| matches!(e, NodeEvent::RelayConnected).then_some(())).await;

    assert!(a.relay_secret().await.unwrap().is_some());
    assert!(b.relay_secret().await.unwrap().is_none());

    b.shutdown().await;
    drop(rb);
    let mut c = cfg_default("手机", Platform::Android);
    c.lan.enabled = false;
    let (_b, mut rb) = Node::open(store(&b_db), c).await.unwrap();
    wait_for(&mut rb, "B 经中转与 A 建立会话", |e| matches!(e, NodeEvent::PeerUp(_)).then_some(())).await;
    a.copy(text("经后来添加的中转")).await.unwrap();
    wait_apply(&mut rb, "经后来添加的中转").await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn pairing_falls_back_to_relay() {
    init_log();
    let (a, mut ra) =
        Node::create_group(store(&temp_db("fa")), cfg_default("台式机", Platform::Windows)).await.unwrap();
    let _relay = start_relay_for(&a).await;
    wait_for(&mut ra, "A 连上中转", |e| matches!(e, NodeEvent::RelayConnected).then_some(())).await;
    let uri = a.create_invite().await.unwrap();
    assert!(uri.contains("relay=") && uri.contains("&rk="));
    let mut c = cfg_default("手机", Platform::Android);
    c.lan.enabled = false;
    let b_store = store(&temp_db("fb"));
    let join = tokio::spawn(async move { Node::join(b_store, c, &uri, |_| {}).await });
    wait_for(&mut ra, "配对请求", |e| matches!(e, NodeEvent::PairRequest { .. }).then_some(())).await;
    let b_id = a.respond_pairing(true).await.unwrap();
    let (_b, mut rb) = join.await.unwrap().unwrap();
    wait_for(&mut ra, "A 与 B 经中转建立会话", |e| {
        matches!(e, NodeEvent::PeerUp(id) if *id == b_id).then_some(())
    })
    .await;
    a.copy(text("经中转配对后同步")).await.unwrap();
    wait_apply(&mut rb, "经中转配对后同步").await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn pair_and_sync_over_tcp_only() {
    let mut p = setup_pair_via(PathPref::TcpOnly).await;
    p.a.copy(text("经 TCP 兜底路径同步")).await.unwrap();
    wait_apply(&mut p.rb, "经 TCP 兜底路径同步").await;
    p.b.copy(text("反方向也没问题")).await.unwrap();
    wait_apply(&mut p.ra, "反方向也没问题").await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn lan_direct_keeps_syncing_without_relay() {
    let mut p = setup_pair().await;
    let (a_id, b_id) = (p.a.id(), p.b.id());

    wait_for(&mut p.ra, "A 建立局域网直连", |e| {
        matches!(e, NodeEvent::LanPath { peer, up: true } if *peer == b_id).then_some(())
    })
    .await;
    wait_for(&mut p.rb, "B 建立局域网直连", |e| {
        matches!(e, NodeEvent::LanPath { peer, up: true } if *peer == a_id).then_some(())
    })
    .await;

    p.a.set_relay_enabled(false).await;
    p.b.set_relay_enabled(false).await;
    wait_for(&mut p.ra, "A 断开中转", |e| matches!(e, NodeEvent::RelayDisconnected(_)).then_some(())).await;
    wait_for(&mut p.rb, "B 断开中转", |e| matches!(e, NodeEvent::RelayDisconnected(_)).then_some(())).await;
    assert!(p.a.online_peers().await.unwrap().contains(&b_id), "中转断开后仍应通过局域网在线");

    p.a.copy(text("仅经局域网传输")).await.unwrap();
    wait_apply(&mut p.rb, "仅经局域网传输").await;
    p.b.copy(text("局域网反方向")).await.unwrap();
    wait_apply(&mut p.ra, "局域网反方向").await;

    p.a.set_relay_enabled(true).await;
    wait_for(&mut p.ra, "A 重新连上中转", |e| matches!(e, NodeEvent::RelayConnected).then_some(())).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn lan_discovery_without_relay() {
    let p = setup_pair().await;
    let (a_id, b_id) = (p.a.id(), p.b.id());
    p.a.shutdown().await;
    p.b.shutdown().await;
    drop((p.ra, p.rb));

    let (pa, pb) = (free_udp_port(), free_udp_port());
    let (a, mut ra) =
        Node::open(store(&p.a_db), cfg_discovery("台式机", Platform::Windows, pa, Some(pb))).await.unwrap();
    let (b, mut rb) = Node::open(store(&p.b_db), cfg_discovery("手机", Platform::Android, pb, Some(pa))).await.unwrap();
    wait_for(&mut ra, "A 经信标建立局域网直连", |e| {
        matches!(e, NodeEvent::LanPath { peer, up: true } if *peer == b_id).then_some(())
    })
    .await;
    wait_for(&mut rb, "B 经信标建立局域网直连", |e| {
        matches!(e, NodeEvent::LanPath { peer, up: true } if *peer == a_id).then_some(())
    })
    .await;
    let st = a.status().await.unwrap();
    assert!(!st.relay_enabled && !st.relay_connected, "全程不应连接中转");

    a.copy(text("没有中转也能同步")).await.unwrap();
    wait_apply(&mut rb, "没有中转也能同步").await;
    b.copy(text("局域网发现反方向")).await.unwrap();
    wait_apply(&mut ra, "局域网发现反方向").await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn lan_discovery_one_way_beacon() {
    let p = setup_pair().await;
    let b_id = p.b.id();
    p.a.shutdown().await;
    p.b.shutdown().await;
    drop((p.ra, p.rb));

    let (pa, pb) = (free_udp_port(), free_udp_port());
    let (_b, _rb) = Node::open(store(&p.b_db), cfg_discovery("手机", Platform::Android, pb, None)).await.unwrap();
    let start = std::time::Instant::now();
    let (_a, mut ra) =
        Node::open(store(&p.a_db), cfg_discovery("台式机", Platform::Windows, pa, Some(pb))).await.unwrap();
    wait_for(&mut ra, "A 建立局域网直连", |e| {
        matches!(e, NodeEvent::LanPath { peer, up: true } if *peer == b_id).then_some(())
    })
    .await;
    assert!(start.elapsed() < Duration::from_secs(5), "单播回应应让双方立即直连，实际用时 {:?}", start.elapsed());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn lan_loss_falls_back_to_relay() {
    let mut p = setup_pair().await;
    let b_id = p.b.id();
    wait_for(&mut p.ra, "A 建立局域网直连", |e| {
        matches!(e, NodeEvent::LanPath { peer, up: true } if *peer == b_id).then_some(())
    })
    .await;

    p.b.shutdown().await;
    drop(p.rb);
    wait_for(&mut p.ra, "A 发现局域网断开", |e| matches!(e, NodeEvent::LanPath { up: false, .. }).then_some(())).await;
    let mut c = cfg_default("手机", Platform::Android);
    c.lan.enabled = false;
    let (_b, mut rb) = Node::open(store(&p.b_db), c).await.unwrap();
    wait_for(&mut rb, "B 经中转重新上线", |e| matches!(e, NodeEvent::PeerUp(_)).then_some(())).await;
    p.a.copy(text("回落到中转")).await.unwrap();
    wait_apply(&mut rb, "回落到中转").await;
}

fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("copycat-files-{}-{name}-{}", std::process::id(), rand_suffix()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn random_file(path: &std::path::Path, len: usize) -> Vec<u8> {
    let mut data = vec![0u8; len];
    copycat_crypto::random::fill(&mut data).unwrap();
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, &data).unwrap();
    data
}

fn outgoing(name: &str, path: PathBuf) -> OutgoingFile {
    OutgoingFile { name: name.into(), source: FileSource::Path(path) }
}

async fn wait_transfer(
    rx: &mut Receiver<NodeEvent>,
    what: &str,
    limit: Duration,
    f: impl Fn(&TransferInfo) -> bool,
) -> TransferInfo {
    tokio::time::timeout(limit, async {
        loop {
            match rx.recv().await {
                Some(NodeEvent::Transfer(t)) if f(&t) => return t,
                Some(NodeEvent::Transfer(t)) => {
                    assert!(!matches!(t.state, TransferState::Failed(_)), "{what}：传输失败 {:?}", t.state);
                }
                Some(_) => {}
                None => panic!("事件通道关闭，仍在等待: {what}"),
            }
        }
    })
    .await
    .unwrap_or_else(|_| panic!("等待超时: {what}"))
}

async fn accept_next(node: &Node, rx: &mut Receiver<NodeEvent>, dir: &std::path::Path) -> TransferInfo {
    let t = wait_transfer(rx, "收到传输请求", WAIT, |t| t.state == TransferState::Asking).await;
    node.accept_transfer(t.id, dir.to_path_buf()).await.unwrap();
    t
}

fn leftovers(dir: &std::path::Path) -> Vec<String> {
    std::fs::read_dir(dir)
        .unwrap()
        .filter_map(|e| e.ok()?.file_name().into_string().ok())
        .filter(|n| n.ends_with(".part"))
        .collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn send_folder_over_lan() {
    let mut p = setup_pair().await;
    let b_id = p.b.id();
    wait_for(&mut p.ra, "A 建立局域网直连", |e| {
        matches!(e, NodeEvent::LanPath { peer, up: true } if *peer == b_id).then_some(())
    })
    .await;

    let src = temp_dir("src");
    let big = random_file(&src.join("a.bin"), 3 * 1024 * 1024 + 5);
    let small = random_file(&src.join("b.txt"), 1000);
    std::fs::write(src.join("empty.txt"), b"").unwrap();
    let dst = temp_dir("dst");
    std::fs::write(dst.join("空.txt"), b"old").unwrap();

    let id =
        p.a.send_files(
            b_id,
            vec![
                outgoing("相册/a.bin", src.join("a.bin")),
                outgoing("相册/子目录/b.txt", src.join("b.txt")),
                outgoing("空.txt", src.join("empty.txt")),
                outgoing("con.txt", src.join("b.txt")),
            ],
        )
        .await
        .unwrap();
    let req = accept_next(&p.b, &mut p.rb, &dst).await;
    assert_eq!(req.id, id);
    assert_eq!(req.total, big.len() as u64 + 2 * small.len() as u64);
    assert!(req.files.iter().any(|(n, _)| n == "_con.txt"), "保留设备名应被改掉：{:?}", req.files);

    let a_done = wait_transfer(&mut p.ra, "A 发送完成", WAIT, |t| t.id == id && t.state == TransferState::Done).await;
    assert!(a_done.lan, "应经局域网传输");
    let b_done = wait_transfer(&mut p.rb, "B 接收完成", WAIT, |t| t.id == id && t.state == TransferState::Done).await;
    assert_eq!(b_done.done, b_done.total);

    assert_eq!(std::fs::read(dst.join("相册").join("a.bin")).unwrap(), big);
    assert_eq!(std::fs::read(dst.join("相册").join("子目录").join("b.txt")).unwrap(), small);
    assert_eq!(std::fs::read(dst.join("_con.txt")).unwrap(), small);
    assert_eq!(std::fs::read(dst.join("空 (1).txt")).unwrap(), b"", "同名文件不能被覆盖");
    assert_eq!(std::fs::read(dst.join("空.txt")).unwrap(), b"old");
    let mut saved = b_done.saved.clone();
    saved.sort();
    assert_eq!(saved, vec![dst.join("_con.txt"), dst.join("相册"), dst.join("空 (1).txt")]);
    assert!(leftovers(&dst).is_empty(), "不应残留临时文件");

    let id2 = p.a.send_files(b_id, vec![outgoing("相册/a.bin", src.join("a.bin"))]).await.unwrap();
    accept_next(&p.b, &mut p.rb, &dst).await;
    wait_transfer(&mut p.rb, "第二次接收完成", WAIT, |t| t.id == id2 && t.state == TransferState::Done).await;
    assert_eq!(std::fs::read(dst.join("相册 (1)").join("a.bin")).unwrap(), big);
    assert_eq!(p.a.transfers().await.unwrap().len(), 2);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn transfer_resumes_after_disconnect() {
    let p = setup_pair().await;
    let b_id = p.b.id();
    p.a.shutdown().await;
    p.b.shutdown().await;
    drop((p.ra, p.rb));
    let relay_only = |name: &str, platform| {
        let mut c = cfg_default(name, platform);
        c.lan.enabled = false;
        c
    };
    let (a, mut ra) = Node::open(store(&p.a_db), relay_only("台式机", Platform::Windows)).await.unwrap();
    let (b, mut rb) = Node::open(store(&p.b_db), relay_only("手机", Platform::Android)).await.unwrap();
    wait_for(&mut ra, "A 与 B 经中转建立会话", |e| {
        matches!(e, NodeEvent::PeerUp(id) if *id == b_id).then_some(())
    })
    .await;

    let src = temp_dir("src");
    let data = random_file(&src.join("video.mp4"), 16 * 1024 * 1024);
    let dst = temp_dir("dst");
    let id = a.send_files(b_id, vec![outgoing("video.mp4", src.join("video.mp4"))]).await.unwrap();
    accept_next(&b, &mut rb, &dst).await;
    let mid = wait_transfer(&mut rb, "B 收到一部分", WAIT, |t| t.id == id && t.done >= 2 * 1024 * 1024).await;
    assert!(mid.done < mid.total);
    assert!(!mid.lan);

    a.set_relay_enabled(false).await;
    wait_transfer(&mut ra, "A 转入等待", WAIT, |t| t.id == id && t.state == TransferState::Waiting).await;
    a.set_relay_enabled(true).await;

    let done = wait_transfer(&mut rb, "B 接收完成", Duration::from_secs(60), |t| {
        assert!(t.done >= mid.done, "断点之前已收到的部分不应重传");
        t.id == id && t.state == TransferState::Done
    })
    .await;
    assert_eq!(done.saved, vec![dst.join("video.mp4")]);
    assert_eq!(std::fs::read(dst.join("video.mp4")).unwrap(), data);
    wait_transfer(&mut ra, "A 发送完成", WAIT, |t| t.id == id && t.state == TransferState::Done).await;
    assert!(leftovers(&dst).is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn share_text_decline_and_cancel() {
    let mut p = setup_pair().await;
    let (a_id, b_id) = (p.a.id(), p.b.id());

    let link = "看看这个 https://example.com/a?b=1 不错";
    let id = p.b.share_text(a_id, link.into()).await.unwrap();
    let (from, text) = wait_for(&mut p.ra, "A 收到分享", |e| match e {
        NodeEvent::SharedText { from, text } => Some((*from, text.clone())),
        _ => None,
    })
    .await;
    assert_eq!((from, text.as_str()), (b_id, link));
    wait_transfer(&mut p.rb, "分享已送达", WAIT, |t| t.id == id && t.state == TransferState::Done).await;

    let src = temp_dir("src");
    random_file(&src.join("x.bin"), 4096);
    let id = p.a.send_files(b_id, vec![outgoing("x.bin", src.join("x.bin"))]).await.unwrap();
    let req = wait_transfer(&mut p.rb, "B 收到请求", WAIT, |t| t.state == TransferState::Asking).await;
    p.b.cancel_transfer(req.id).await.unwrap();
    let t = wait_transfer(&mut p.ra, "A 得知被拒绝", WAIT, |t| t.id == id && t.state.is_finished()).await;
    assert_eq!(t.state, TransferState::Failed("对方拒绝接收".into()));

    let id = p.a.send_files(b_id, vec![outgoing("x.bin", src.join("x.bin"))]).await.unwrap();
    wait_transfer(&mut p.rb, "B 收到第二个请求", WAIT, |t| t.id == id && t.state == TransferState::Asking).await;
    p.a.cancel_transfer(id).await.unwrap();
    let t = wait_transfer(&mut p.rb, "B 得知对方取消", WAIT, |t| t.id == id && t.state.is_finished()).await;
    assert_eq!(t.state, TransferState::Failed("对方取消了发送".into()));
    assert!(p.b.accept_transfer(id, src.clone()).await.is_err());

    assert!(p.a.send_files(a_id, vec![outgoing("x.bin", src.join("x.bin"))]).await.is_err());
    assert!(p.a.share_text(copycat_core::DeviceId([9; 16]), "x".into()).await.is_err());
}

fn random_bytes(len: usize) -> Vec<u8> {
    let mut data = vec![0u8; len];
    copycat_crypto::random::fill(&mut data).unwrap();
    data
}

async fn wait_item(rx: &mut Receiver<NodeEvent>, what: &str, id: [u8; 16]) -> ClipItem {
    wait_for(rx, what, |e| match e {
        NodeEvent::Apply(i) if i.id == id => Some(i.clone()),
        _ => None,
    })
    .await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn large_clipboard_content_is_fetched() {
    let mut p = setup_pair().await;
    let png = random_bytes(3 * 1024 * 1024 + 7);
    p.a.copy(vec![
        Rep::from_bytes("text/plain", "截图说明".as_bytes().to_vec()),
        Rep::from_bytes("image/png", png.clone()),
    ])
    .await
    .unwrap();
    let id = p.a.history().await.unwrap()[0].id;
    let got = wait_item(&mut p.rb, "B 写入完整的大图片", id).await;
    assert!(got.is_complete());
    assert_eq!(got.reps[1].inline.as_deref(), Some(&png[..]));
    assert_eq!(text_of(&got), "截图说明");

    let long = "长文本".repeat(400_000);
    p.a.copy(text(&long)).await.unwrap();
    let got = wait_apply(&mut p.rb, &long).await;
    assert!(got.is_complete());

    let opts = copycat_core::Options { fetch_images: false, ..Default::default() };
    p.b.set_options(opts.clone()).await;
    let png2 = random_bytes(2 * 1024 * 1024);
    p.a.copy(vec![Rep::from_bytes("image/png", png2.clone())]).await.unwrap();
    let id2 = p.a.history().await.unwrap()[0].id;
    let stub = tokio::time::timeout(WAIT, async {
        loop {
            if let Some(i) = p.b.history().await.unwrap().into_iter().find(|i| i.id == id2) {
                return i;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("B 应收到条目的元数据");
    assert!(!stub.is_complete());
    assert!(!p.b.fetch_item(id2).await.unwrap().is_complete(), "关闭图片拉取时按需拉取也不下载图片");

    p.b.set_options(copycat_core::Options { fetch_images: true, ..opts }).await;
    let full = p.b.fetch_item(id2).await.unwrap();
    assert_eq!(full.reps[0].inline.as_deref(), Some(&png2[..]));
    let stored = p.b.history().await.unwrap().into_iter().find(|i| i.id == id2).unwrap();
    assert!(stored.is_complete(), "拉取到的内容应写回历史");
    while let Ok(e) = p.rb.try_recv() {
        assert!(!matches!(e, NodeEvent::Apply(ref i) if i.id == id2), "按需拉取的旧条目不应写入剪贴板");
    }

    assert!(p.a.transfers().await.unwrap().is_empty(), "拉取不应出现在传输列表中");
    assert!(p.b.transfers().await.unwrap().is_empty());
    assert!(p.b.fetch_item([3; 16]).await.is_err());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn screenshot_needs_receiver_opt_in() {
    let mut p = setup_pair().await;
    let b_id = p.b.id();
    eprintln!("SHOT A={} B={}", p.a.id().short(), b_id.short());
    let shot = random_bytes(300_000);
    let id = p.a.send_screenshot(b_id, "截图.png".into(), shot.clone()).await.unwrap();
    let t = wait_transfer(&mut p.ra, "未开启时被拒绝", WAIT, |t| t.id == id && t.state.is_finished()).await;
    assert_eq!(t.state, TransferState::Failed("对方没有开启接收截图".into()));
    assert_eq!(t.kind, copycat_core::TransferKind::Screenshot);

    p.b.set_options(copycat_core::Options { accept_screenshots: true, ..Default::default() }).await;
    tokio::time::timeout(WAIT, async {
        loop {
            let st = p.a.status().await.unwrap();
            if st.peers.iter().any(|x| x.id == b_id && x.screenshots) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("A 应得知 B 开启了接收截图");

    let id = p.a.send_screenshot(b_id, "截图.png".into(), shot.clone()).await.unwrap();
    let dst = temp_dir("shots");
    let req = accept_next(&p.b, &mut p.rb, &dst).await;
    assert_eq!(req.kind, copycat_core::TransferKind::Screenshot);
    wait_transfer(&mut p.rb, "B 收到截图", WAIT, |t| t.id == id && t.state == TransferState::Done).await;
    assert_eq!(std::fs::read(dst.join("截图.png")).unwrap(), shot);
}

async fn wait_screenshot_cap(node: &Node, peer: copycat_core::DeviceId, what: &str) {
    tokio::time::timeout(WAIT, async {
        loop {
            let st = node.status().await.unwrap();
            if st.peers.iter().any(|x| x.id == peer && x.screenshots && (x.relay || x.lan)) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("{what}"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn screenshot_opt_in_survives_peer_crash() {
    let mut p = setup_pair().await;
    let b_id = p.b.id();
    p.b.set_options(copycat_core::Options { accept_screenshots: true, ..Default::default() }).await;
    p.a.shutdown().await;
    drop(p.ra);
    wait_for(&mut p.rb, "B 发现 A 离线", |e| matches!(e, NodeEvent::PeerDown(_)).then_some(())).await;

    let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap();
    let (a, mut ra) =
        rt.spawn(Node::open(store(&p.a_db), cfg_default("台式机", Platform::Windows))).await.unwrap().unwrap();
    wait_for(&mut ra, "A 与 B 建立局域网直连", |e| {
        matches!(e, NodeEvent::LanPath { peer, up: true } if *peer == b_id).then_some(())
    })
    .await;
    wait_screenshot_cap(&a, b_id, "A 应得知 B 开启了接收截图").await;
    rt.shutdown_background();
    drop((a, ra));

    let (a, _ra) = Node::open(store(&p.a_db), cfg_default("台式机", Platform::Windows)).await.unwrap();
    wait_screenshot_cap(&a, b_id, "A 重启后应重新得知 B 开启了接收截图").await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn large_clipboard_content_over_relay_only() {
    let p = setup_pair().await;
    let b_id = p.b.id();
    p.a.shutdown().await;
    p.b.shutdown().await;
    drop((p.ra, p.rb));
    let relay_only = |name: &str, platform| {
        let mut c = cfg_default(name, platform);
        c.lan.enabled = false;
        c
    };
    let (a, mut ra) = Node::open(store(&p.a_db), relay_only("台式机", Platform::Windows)).await.unwrap();
    let (_b, mut rb) = Node::open(store(&p.b_db), relay_only("手机", Platform::Android)).await.unwrap();
    wait_for(&mut ra, "A 与 B 经中转建立会话", |e| {
        matches!(e, NodeEvent::PeerUp(id) if *id == b_id).then_some(())
    })
    .await;
    let png = random_bytes(5 * 1024 * 1024);
    a.copy(vec![Rep::from_bytes("image/png", png.clone())]).await.unwrap();
    let id = a.history().await.unwrap()[0].id;
    let got = wait_item(&mut rb, "B 经中转拉取到图片", id).await;
    assert_eq!(got.reps[0].inline.as_deref(), Some(&png[..]));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn clear_history_everywhere() {
    let mut p = setup_pair().await;
    p.a.copy(text("误复制的密码")).await.unwrap();
    wait_apply(&mut p.rb, "误复制的密码").await;
    assert_eq!(p.a.forget_all_everywhere().await.unwrap(), 1);
    tokio::time::timeout(WAIT, async {
        while !p.b.history().await.unwrap().is_empty() {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("B 的历史应被清空");
    assert!(p.a.history().await.unwrap().is_empty());

    p.b.resumed(Duration::from_secs(60)).await;
    p.a.copy(text("新的内容")).await.unwrap();
    wait_apply(&mut p.rb, "新的内容").await;
    assert_eq!(p.b.history().await.unwrap().len(), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn lan_handshake_from_stranger_is_rejected() {
    use copycat_crypto::DeviceIdentity;
    use copycat_crypto::session::{Initiator, PathKind, session_prologue};
    use copycat_proto::peer::{PeerFrame, peer_frame::Kind};
    use copycat_transport::lan::LanEndpoint;

    let p = setup_pair().await;
    let port = p.a.status().await.unwrap().lan_port.expect("A 应开启局域网监听");
    let a_rec = p.b.members().await.unwrap().into_iter().find(|m| m.id == p.a.id()).unwrap();
    let group = store(&p.a_db).group().unwrap().unwrap();
    let addr: std::net::SocketAddr = format!("127.0.0.1:{port}").parse().unwrap();

    let attempt = |who: DeviceIdentity, group: [u8; 32]| {
        let a_dh = a_rec.dh_pub;
        async move {
            let ep = LanEndpoint::bind(0, true).unwrap();
            let mut c = ep.connect(addr, Duration::from_secs(3)).await.unwrap();
            let (_init, m1) = Initiator::start(&who, &a_dh, &session_prologue(&group, PathKind::Lan)).unwrap();
            c.write.send(&PeerFrame { session_id: 7, kind: Some(Kind::HandshakeInit(m1)) }).await.unwrap();

            if let Ok(Ok(Some(f))) = tokio::time::timeout(Duration::from_secs(3), c.read.recv::<PeerFrame>()).await {
                panic!("陌生设备不应得到回应：{:?}", f.kind.map(|k| std::mem::discriminant(&k)));
            }
        }
    };
    attempt(DeviceIdentity::generate().unwrap(), group).await;

    let b_identity = store(&p.b_db).load_identity().unwrap().unwrap();
    attempt(b_identity, [9; 32]).await;

    let mut rb = p.rb;
    p.a.copy(text("仍然正常")).await.unwrap();
    wait_apply(&mut rb, "仍然正常").await;
}
