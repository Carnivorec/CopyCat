pub mod dto;
pub mod settings;
pub mod shots;
pub mod transfer;

use std::collections::HashSet;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use copycat_core::{
    ClipItem, CoreError, DeviceId, DeviceRecord, Hlc, Node, NodeConfig, NodeEvent, Options, Platform, Rep, TransferId,
    TransferInfo, TransferKind, TransferState,
};
use copycat_store::Store;
use data_encoding::{BASE32_NOPAD, BASE64};
use serde::Serialize;
use tokio::sync::mpsc;
use tracing::{debug, error, info, warn};

pub use dto::{MIME_PNG, MIME_TEXT, Overview};
pub use settings::DEFAULT_HOTKEY;
pub use settings::Settings;
pub use transfer::{SendSummary, TransferDto};

pub const MAX_IMAGE: usize = 64 * 1024 * 1024;

const SCREENSHOT_DEDUP: Duration = Duration::from_secs(60);

const DUPLICATE_COPY: Duration = Duration::from_secs(2);

const SHOT_PRUNE_EVERY: Duration = Duration::from_secs(60 * 60);

pub type Res<T> = Result<T, String>;

pub trait Host: Send + Sync + 'static {
    fn platform(&self) -> Platform;

    fn default_settings(&self) -> Settings;

    fn master_key(&self, data_dir: &Path) -> Res<[u8; 32]>;

    fn write_clipboard(&self, item: ClipItem, remote: bool) -> impl Future<Output = Res<()>> + Send;

    fn emit(&self, ev: UiEvent);

    fn default_receive_dir(&self) -> PathBuf;

    fn ask_receive_dir(&self, t: TransferDto) -> impl Future<Output = Option<PathBuf>> + Send {
        let _ = t;
        async { None }
    }

    fn open_url(&self, url: &str);

    fn transfer_finished(&self, t: &TransferDto);

    fn writes_images(&self) -> bool {
        false
    }

    fn screenshot_name(&self) -> String {
        format!("截图_{}.png", now_ms())
    }

    fn files_removed(&self, paths: &[PathBuf]) {
        let _ = paths;
    }
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct PairRequest {
    pub name: String,
    pub platform: &'static str,
    pub sas: String,
}

#[derive(Serialize, Clone, Debug)]
#[serde(tag = "event", content = "data", rename_all = "kebab-case")]
pub enum UiEvent {
    Changed(&'static str),

    Arrived(String),
    PairRequest(PairRequest),

    Paired(String),

    Revoked,

    JoinSas(String),

    Transfer(Box<TransferDto>),

    SharedText(SharedText),
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct SharedText {
    pub device: String,
    pub text: String,

    pub link: Option<String>,

    pub opened: bool,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct RelaySetup {
    pub addr: String,

    pub key: String,

    pub compose: String,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct LeaveResult {
    pub peers: usize,

    pub relay: bool,
}

const COMPOSE_TEMPLATE: &str = include_str!("../../../../deploy/docker/compose.yml");

fn compose_for(addr: &str, key: &str) -> String {
    let port = addr.rsplit_once(':').map_or("8443", |(_, p)| p);
    COMPOSE_TEMPLATE
        .replace("${COPYCAT_PORT:-8443}", port)
        .replace("COPYCAT_RELAY_KEY: \"\"", &format!("COPYCAT_RELAY_KEY: \"{key}\""))
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Invite {
    pub uri: String,

    pub matrix: Vec<String>,
}

struct Inner<H> {
    host: H,
    data_dir: PathBuf,
    node: tokio::sync::Mutex<Option<Node>>,
    settings: Mutex<Settings>,
    paused: AtomicBool,

    members: Mutex<Vec<DeviceRecord>>,

    finished: Mutex<HashSet<TransferId>>,

    pause_gen: std::sync::atomic::AtomicU64,

    last_shot: Mutex<Option<([u8; 32], Instant)>>,

    last_copy: Mutex<Option<([u8; 32], Instant)>>,

    shot_pruner: AtomicBool,

    leaving: AtomicBool,
}

pub struct Service<H>(Arc<Inner<H>>);

impl<H> Clone for Service<H> {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}

fn parse_item_id(id: &str) -> Res<[u8; 16]> {
    hex::decode(id).ok().and_then(|v| v.try_into().ok()).ok_or_else(|| "条目 ID 无效".to_owned())
}

fn parse_transfer_id(id: &str) -> Res<TransferId> {
    hex::decode(id).ok().and_then(|v| v.try_into().ok()).ok_or_else(|| "传输 ID 无效".to_owned())
}

fn parse_device_id(id: &str) -> Res<DeviceId> {
    let raw = BASE32_NOPAD.decode(id.to_ascii_uppercase().as_bytes()).map_err(|_| "设备 ID 无效".to_owned())?;
    DeviceId::from_slice(&raw).map_err(|_| "设备 ID 无效".to_owned())
}

impl<H: Host> Service<H> {
    pub fn new(host: H, data_dir: PathBuf) -> std::io::Result<Self> {
        std::fs::create_dir_all(&data_dir)?;
        let settings = Settings::load(&data_dir, || host.default_settings());
        Ok(Self(Arc::new(Inner {
            host,
            data_dir,
            node: tokio::sync::Mutex::new(None),
            settings: Mutex::new(settings),
            paused: AtomicBool::new(false),
            members: Mutex::new(Vec::new()),
            finished: Mutex::new(HashSet::new()),
            pause_gen: std::sync::atomic::AtomicU64::new(0),
            last_shot: Mutex::new(None),
            last_copy: Mutex::new(None),
            shot_pruner: AtomicBool::new(false),
            leaving: AtomicBool::new(false),
        })))
    }

    pub fn host(&self) -> &H {
        &self.0.host
    }

    pub fn data_dir(&self) -> &Path {
        &self.0.data_dir
    }

    pub fn settings(&self) -> Settings {
        self.0.settings.lock().unwrap_or_else(|p| p.into_inner()).clone()
    }

    pub fn is_paused(&self) -> bool {
        self.0.paused.load(Ordering::Relaxed)
    }

    pub async fn node(&self) -> Res<Node> {
        self.0.node.lock().await.clone().ok_or_else(|| "还没有加入设备组".to_owned())
    }

    fn store_settings(&self, s: Settings) -> Res<()> {
        s.save(&self.0.data_dir).map_err(|e| format!("保存设置失败：{e}"))?;
        *self.0.settings.lock().unwrap_or_else(|p| p.into_inner()) = s;
        Ok(())
    }

    fn node_config(&self) -> NodeConfig {
        let s = self.settings();
        let mut c = NodeConfig::new(s.device_name.clone(), self.0.host.platform());
        c.lan.enabled = s.lan_enabled;
        c.relay_enabled = s.relay_enabled;
        if !s.mailbox {
            c.mailbox_ttl_secs = 0;
        }
        c.options = self.options(&s);
        if self.0.host.platform() == Platform::Android {
            c.tuning = copycat_core::Tuning::mobile();
        }
        c.cache_dir = self.0.data_dir.join("cache");
        c
    }

    fn options(&self, s: &Settings) -> Options {
        let android = self.0.host.platform() == Platform::Android;
        Options {
            fetch_images: s.sync_images && self.0.host.writes_images(),
            fetch_max: MAX_IMAGE as u64,
            accept_screenshots: android && s.screenshots,
            history_days: s.history_days,
        }
    }

    fn open_store(&self) -> Res<Store> {
        let key = self.0.host.master_key(&self.0.data_dir)?;
        Store::open(self.0.data_dir.join("copycat.db"), &key).map_err(|e| format!("打开本地数据失败：{e}"))
    }

    fn wipe_store(&self) {
        for f in ["copycat.db", "copycat.db-wal", "copycat.db-shm"] {
            let _ = std::fs::remove_file(self.0.data_dir.join(f));
        }
    }

    pub async fn leave_group(&self) -> Res<LeaveResult> {
        let mut slot = self.0.node.lock().await;
        let node = slot.as_ref().ok_or("本机还没有加入设备组")?;
        self.0.leaving.store(true, Ordering::Relaxed);
        let report = match node.leave_group().await {
            Ok(r) => r,
            Err(e) => {
                self.0.leaving.store(false, Ordering::Relaxed);
                return Err(format!("退出设备组失败：{e}"));
            }
        };
        if let Some(node) = slot.take() {
            node.shutdown().await;
        }
        drop(slot);
        self.0.members.lock().unwrap_or_else(|p| p.into_inner()).clear();
        self.0.finished.lock().unwrap_or_else(|p| p.into_inner()).clear();
        self.0.leaving.store(false, Ordering::Relaxed);
        for f in ["copycat.db", "copycat.db-wal", "copycat.db-shm"] {
            match std::fs::remove_file(self.0.data_dir.join(f)) {
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
                    self.0.host.emit(UiEvent::Changed("node"));
                    return Err(format!("已退出设备组，但删除本地数据 {f} 失败：{e}"));
                }
                _ => {}
            }
        }
        let _ = std::fs::remove_dir_all(self.0.data_dir.join("cache"));
        info!(peers = report.peers, relay = report.relay, "已退出设备组");
        self.0.host.emit(UiEvent::Changed("node"));
        Ok(LeaveResult { peers: report.peers, relay: report.relay })
    }

    pub async fn start(&self) -> Res<bool> {
        let store = self.open_store()?;
        if !matches!(store.load_identity(), Ok(Some(_))) {
            return Ok(false);
        }
        let (node, events) = Node::open(store, self.node_config()).await.map_err(|e| format!("启动节点失败：{e}"))?;
        self.attach(node, events).await;
        Ok(true)
    }

    async fn attach(&self, node: Node, events: mpsc::Receiver<NodeEvent>) {
        if !self.settings().relay_enabled {
            node.set_relay_enabled(false).await;
        }
        if let Ok(m) = node.members().await {
            *self.0.members.lock().unwrap_or_else(|p| p.into_inner()) = m;
        }
        *self.0.node.lock().await = Some(node);
        self.0.host.emit(UiEvent::Changed("node"));
        tokio::spawn(self.clone().pump(events));
        if self.0.host.platform() == Platform::Android && !self.0.shot_pruner.swap(true, Ordering::Relaxed) {
            let me = self.clone();
            tokio::spawn(async move {
                loop {
                    me.prune_screenshots().await;
                    tokio::time::sleep(SHOT_PRUNE_EVERY).await;
                }
            });
        }
    }

    async fn prune_screenshots(&self) {
        if self.0.host.platform() != Platform::Android {
            return;
        }
        let limits = shots::Limits::from_settings(&self.settings());
        if limits.is_unlimited() {
            return;
        }
        let dir = self.receive_dir().join("screenshot");
        let removed =
            tokio::task::spawn_blocking(move || shots::prune(&dir, &limits, std::time::SystemTime::now())).await;
        if let Ok(removed) = removed
            && !removed.is_empty()
        {
            info!(count = removed.len(), "自动清理了较旧的截图");
            self.0.host.files_removed(&removed);
        }
    }

    fn members(&self) -> Vec<DeviceRecord> {
        self.0.members.lock().unwrap_or_else(|p| p.into_inner()).clone()
    }

    fn refresh_members(&self) {
        let me = self.clone();
        tokio::spawn(async move {
            if let Ok(node) = me.node().await
                && let Ok(m) = node.members().await
            {
                *me.0.members.lock().unwrap_or_else(|p| p.into_inner()) = m;
            }
        });
    }

    pub fn receive_dir(&self) -> PathBuf {
        let dir = self.settings().receive_dir;
        if dir.trim().is_empty() { self.0.host.default_receive_dir() } else { PathBuf::from(dir.trim()) }
    }

    fn on_offer(&self, dto: TransferDto, id: TransferId) {
        let me = self.clone();
        tokio::spawn(async move {
            let Ok(node) = me.node().await else { return };
            let dir = if dto.kind == "screenshot" {
                Some(me.receive_dir().join("screenshot"))
            } else if me.settings().ask_receive_dir {
                me.0.host.ask_receive_dir(dto).await
            } else {
                Some(me.receive_dir())
            };
            let r = match dir {
                Some(dir) => node.accept_transfer(id, dir).await,
                None => node.cancel_transfer(id).await,
            };
            if let Err(e) = r {
                warn!(error = %e, "处理文件传输请求失败");
            }
        });
    }

    fn on_transfer(&self, t: TransferInfo) {
        let dto = transfer::transfer_dto(&t, &self.members());
        if t.state == TransferState::Asking && !t.outgoing {
            self.on_offer(dto.clone(), t.id);
        }
        if t.state == TransferState::Done && !t.outgoing && t.kind == TransferKind::Screenshot {
            let me = self.clone();
            tokio::spawn(async move { me.prune_screenshots().await });
        }

        if t.state.is_finished()
            && t.state != TransferState::Cancelled
            && (t.outgoing || t.text.is_none())
            && !(t.outgoing && t.kind == TransferKind::Screenshot)
            && self.0.finished.lock().unwrap_or_else(|p| p.into_inner()).insert(t.id)
        {
            self.0.host.transfer_finished(&dto);
        }
        self.0.host.emit(UiEvent::Transfer(Box::new(dto)));
    }

    async fn on_shared_text(&self, from: DeviceId, text: String) {
        let host = &self.0.host;
        let item = ClipItem {
            id: copycat_crypto::random::bytes::<16>().unwrap_or_default(),
            origin: from,
            seq: 0,
            hlc: Hlc { wall_ms: now_ms(), counter: 0, node: from },
            reps: vec![Rep::from_bytes(MIME_TEXT, text.as_bytes().to_vec())],
        };
        if let Err(e) = host.write_clipboard(item, true).await {
            warn!(error = %e, "分享的文本没能写入剪贴板");
        }
        let link = transfer::first_link(&text).map(str::to_owned);
        let opened = match &link {
            Some(l) if self.settings().open_shared_links => {
                host.open_url(l);
                true
            }
            _ => false,
        };
        let device =
            self.members().iter().find(|m| m.id == from).map_or_else(|| "其他设备".to_owned(), |m| m.name.clone());
        host.emit(UiEvent::SharedText(SharedText { device, text, link, opened }));
    }

    async fn pump(self, mut events: mpsc::Receiver<NodeEvent>) {
        let host = &self.0.host;
        while let Some(e) = events.recv().await {
            match e {
                NodeEvent::Apply(item) => {
                    let id = hex::encode(item.id);
                    let mimes: Vec<&str> =
                        item.reps.iter().filter(|r| r.inline.is_some()).map(|r| r.mime.as_str()).collect();
                    if self.is_paused() {
                        info!(item = &id[..8], from = %item.origin.short(), "收到远端内容，已暂停同步，不写入剪贴板");
                    } else {
                        info!(item = &id[..8], from = %item.origin.short(), ?mimes, "写入剪贴板");

                        *self.0.last_copy.lock().unwrap_or_else(|p| p.into_inner()) = None;
                        if let Err(e) = host.write_clipboard(item, true).await {
                            warn!(item = &id[..8], error = %e, "远端内容没能写入剪贴板");
                        }
                    }
                    host.emit(UiEvent::Arrived(id));
                }
                NodeEvent::PairRequest { name, platform, sas } => {
                    host.emit(UiEvent::PairRequest(PairRequest { name, platform: dto::platform_str(platform), sas }));
                }
                NodeEvent::Paired(id) => {
                    self.refresh_members();
                    host.emit(UiEvent::Paired(id.to_string()));
                }
                NodeEvent::Transfer(t) => self.on_transfer(t),
                NodeEvent::SharedText { from, text } => self.on_shared_text(from, text).await,
                NodeEvent::Revoked if self.0.leaving.load(Ordering::Relaxed) => {}
                NodeEvent::Revoked => host.emit(UiEvent::Revoked),
                NodeEvent::HistoryChanged => host.emit(UiEvent::Changed("history")),
                NodeEvent::PeerUp(_)
                | NodeEvent::PeerDown(_)
                | NodeEvent::LanPath { .. }
                | NodeEvent::RelayConnected
                | NodeEvent::RelayDisconnected(_)
                | NodeEvent::RelayChanged => host.emit(UiEvent::Changed("status")),
                NodeEvent::RosterChanged => {
                    self.refresh_members();
                    host.emit(UiEvent::Changed("status"));
                }
            }
        }
        self.recover_node().await;
    }

    fn recover_node(&self) -> std::pin::Pin<Box<dyn Future<Output = ()> + Send + '_>> {
        Box::pin(async move {
            let mut crashed = false;
            for _ in 0..20 {
                let slot = self.0.node.lock().await;
                match slot.as_ref() {
                    Some(n) if n.is_stopped() => {
                        crashed = true;
                        break;
                    }
                    Some(_) => {}
                    None => return,
                }
                drop(slot);
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            if !crashed {
                return;
            }
            {
                let mut slot = self.0.node.lock().await;
                if !slot.as_ref().is_some_and(Node::is_stopped) {
                    return;
                }
                *slot = None;
            }
            error!("同步节点意外停止，正在重新启动");
            self.0.host.emit(UiEvent::Changed("node"));
            for wait in [1u64, 5, 15, 60] {
                tokio::time::sleep(Duration::from_secs(wait)).await;
                if self.0.node.lock().await.is_some() {
                    return;
                }
                match self.start().await {
                    Ok(_) => {
                        info!("同步节点已重新启动");
                        return;
                    }
                    Err(e) => warn!(error = %e, "重新启动同步节点失败"),
                }
            }
            error!("同步节点多次重启失败，需要重新打开应用");
        })
    }

    pub async fn shutdown(&self) {
        if let Some(node) = self.0.node.lock().await.take() {
            node.shutdown().await;
        }
    }

    pub async fn create_group(&self, name: &str) -> Res<()> {
        let mut s = self.settings();
        s.device_name = name.trim().to_owned();
        self.store_settings(s)?;
        let store = self.open_store()?;
        match Node::create_group(store, self.node_config()).await {
            Ok((node, events)) => {
                self.attach(node, events).await;
                Ok(())
            }
            Err(e) => {
                self.wipe_store();
                Err(e.to_string())
            }
        }
    }

    pub async fn join_group(&self, uri: &str, name: &str) -> Res<()> {
        let uri = uri.trim();
        copycat_crypto::pairing::PairingInvite::parse_uri(uri).map_err(|_| "这不是有效的 CopyCat 配对码".to_owned())?;
        let mut s = self.settings();
        s.device_name = name.trim().to_owned();
        self.store_settings(s)?;

        let store = self.open_store()?;
        let me = self.clone();
        let res =
            Node::join(store, self.node_config(), uri, move |sas| me.0.host.emit(UiEvent::JoinSas(sas.to_owned())))
                .await;
        match res {
            Ok((node, events)) => {
                self.attach(node, events).await;
                Ok(())
            }
            Err(e) => {
                warn!(error = %e, "加入设备组失败");
                self.wipe_store();
                Err(match e {
                    CoreError::Crypto(_) => "配对码无效或已过期，请在已加入的设备上重新生成".to_owned(),
                    other => other.to_string(),
                })
            }
        }
    }

    pub async fn overview(&self) -> Res<Overview> {
        let settings = self.settings();
        let paused = self.is_paused();
        let Some(node) = self.0.node.lock().await.clone() else {
            return Ok(Overview {
                initialized: false,
                me: None,
                relay: None,
                devices: vec![],
                history: vec![],
                settings,
                paused,
                lan_port: None,
                transfers: vec![],
                receive_dir: self.receive_dir().display().to_string(),
            });
        };
        let (status, members, items, transfers) =
            tokio::try_join!(node.status(), node.members(), node.history(), node.transfers())
                .map_err(|e| e.to_string())?;
        let devices = dto::devices(&members, &status);
        Ok(Overview {
            initialized: true,
            me: devices.iter().find(|d| d.me).cloned(),
            relay: Some(dto::RelayDto {
                configured: status.relay_addr.is_some(),
                addr: status.relay_addr.clone().unwrap_or_default(),
                connected: status.relay_connected,
                enabled: status.relay_enabled,
            }),
            history: dto::history(&items, &members, status.me),
            transfers: transfers.iter().map(|t| transfer::transfer_dto(t, &members)).collect(),
            receive_dir: self.receive_dir().display().to_string(),
            devices,
            lan_port: status.lan_port,
            settings,
            paused,
        })
    }

    pub async fn create_invite(&self) -> Res<Invite> {
        let uri = self.node().await?.create_invite().await.map_err(|e| e.to_string())?;
        let code = qrcode::QrCode::new(uri.as_bytes()).map_err(|e| e.to_string())?;
        let w = code.width();
        let matrix = code
            .to_colors()
            .chunks(w)
            .map(|row| row.iter().map(|c| if *c == qrcode::Color::Dark { '1' } else { '0' }).collect())
            .collect();
        Ok(Invite { uri, matrix })
    }

    pub async fn respond_pairing(&self, accept: bool) -> Res<()> {
        let r = self.node().await?.respond_pairing(accept).await;
        match (accept, r) {
            (true, Err(e)) => Err(e.to_string()),
            _ => Ok(()),
        }
    }

    pub async fn remove_device(&self, id: &str) -> Res<()> {
        self.node().await?.remove_device(parse_device_id(id)?).await.map_err(|e| e.to_string())
    }

    async fn find_item(&self, id: &str) -> Res<ClipItem> {
        let id = parse_item_id(id)?;
        let node = self.node().await?;
        node.history()
            .await
            .map_err(|e| e.to_string())?
            .into_iter()
            .find(|i| i.id == id)
            .ok_or_else(|| "这条记录已不存在".to_owned())
    }

    pub async fn copy_item(&self, id: &str) -> Res<()> {
        let mut item = self.find_item(id).await?;
        if !item.is_complete() {
            item = self
                .node()
                .await?
                .fetch_item(item.id)
                .await
                .map_err(|e| format!("内容还没有下载到本机，暂时无法复制：{e}"))?;
        }
        if item.reps.iter().all(|r| r.inline.is_none()) {
            return Err("这条内容在本机不可用（同步图片已关闭）".into());
        }
        self.0.host.write_clipboard(item, false).await.map_err(|e| format!("没能写入剪贴板：{e}"))
    }

    pub async fn forget_item(&self, id: &str) -> Res<()> {
        self.node().await?.forget(parse_item_id(id)?).await.map_err(|e| e.to_string())
    }

    pub async fn clear_history(&self) -> Res<()> {
        self.node().await?.forget_all().await.map_err(|e| e.to_string())
    }

    pub async fn clear_history_everywhere(&self) -> Res<usize> {
        self.node().await?.forget_all_everywhere().await.map_err(|e| e.to_string())
    }

    pub async fn item_image(&self, id: &str) -> Res<Option<String>> {
        let item = self.find_item(id).await?;
        Ok(item
            .reps
            .iter()
            .find(|r| r.mime == MIME_PNG)
            .and_then(|r| r.inline.as_ref())
            .map(|b| format!("data:image/png;base64,{}", BASE64.encode(b))))
    }

    pub async fn item_text(&self, id: &str) -> Res<Option<String>> {
        let item = self.find_item(id).await?;
        Ok(item
            .reps
            .iter()
            .find(|r| r.mime == MIME_TEXT)
            .and_then(|r| r.inline.as_ref())
            .map(|b| String::from_utf8_lossy(b).into_owned()))
    }

    pub async fn setup_relay(&self, addr: &str) -> Res<RelaySetup> {
        let addr = addr.trim().to_owned();
        let key = self.node().await?.setup_relay(addr.clone()).await.map_err(|e| e.to_string())?;
        self.0.host.emit(UiEvent::Changed("status"));
        Ok(RelaySetup { compose: compose_for(&addr, &key), addr, key })
    }

    pub async fn relay_setup(&self) -> Res<Option<RelaySetup>> {
        let node = self.node().await?;
        let Some(key) = node.relay_secret().await.map_err(|e| e.to_string())? else { return Ok(None) };
        let addr = node.status().await.map_err(|e| e.to_string())?.relay_addr.unwrap_or_default();
        Ok(Some(RelaySetup { compose: compose_for(&addr, &key), addr, key }))
    }

    pub async fn set_relay_addr(&self, addr: &str) -> Res<()> {
        self.node().await?.set_relay_addr(addr.trim().to_owned()).await.map_err(|e| e.to_string())?;
        self.0.host.emit(UiEvent::Changed("status"));
        Ok(())
    }

    pub async fn remove_relay(&self) -> Res<()> {
        self.node().await?.remove_relay().await.map_err(|e| e.to_string())?;
        self.0.host.emit(UiEvent::Changed("status"));
        Ok(())
    }

    pub async fn set_relay_enabled(&self, enabled: bool) -> Res<()> {
        self.node().await?.set_relay_enabled(enabled).await;
        let mut s = self.settings();
        s.relay_enabled = enabled;
        self.store_settings(s)?;
        self.0.host.emit(UiEvent::Changed("status"));
        Ok(())
    }

    pub fn set_paused(&self, paused: bool) {
        self.0.pause_gen.fetch_add(1, Ordering::Relaxed);
        self.0.paused.store(paused, Ordering::Relaxed);
        self.0.host.emit(UiEvent::Changed("status"));
    }

    pub fn pause_for(&self, d: Duration) {
        self.set_paused(true);
        let gen_at = self.0.pause_gen.load(Ordering::Relaxed);
        let me = self.clone();
        tokio::spawn(async move {
            tokio::time::sleep(d).await;
            if me.0.pause_gen.load(Ordering::Relaxed) == gen_at {
                me.set_paused(false);
            }
        });
    }

    pub fn save_settings(&self, s: Settings) -> Res<()> {
        let old = self.settings();
        let before = self.options(&old);
        let after = self.options(&s);
        let shots_changed = shots::Limits::from_settings(&old) != shots::Limits::from_settings(&s);
        self.store_settings(s)?;
        if shots_changed {
            let me = self.clone();
            tokio::spawn(async move { me.prune_screenshots().await });
        }
        if before != after {
            let me = self.clone();
            tokio::spawn(async move {
                if let Ok(node) = me.node().await {
                    node.set_options(after).await;
                }
            });
        }
        self.0.host.emit(UiEvent::Changed("settings"));
        Ok(())
    }

    pub fn summarize(&self, paths: &[PathBuf]) -> Res<SendSummary> {
        let (files, total) = transfer::collect_files(paths)?;
        Ok(SendSummary { count: files.len(), total })
    }

    pub async fn send_paths(&self, device: &str, paths: &[PathBuf]) -> Res<String> {
        let peer = parse_device_id(device)?;
        let (files, _) = transfer::collect_files(paths)?;
        let id = self.node().await?.send_files(peer, files).await.map_err(|e| e.to_string())?;
        Ok(hex::encode(id))
    }

    pub async fn send_open_files(&self, device: &str, files: Vec<(String, std::fs::File)>) -> Res<String> {
        let peer = parse_device_id(device)?;
        let (files, _) = transfer::open_files(files)?;
        let id = self.node().await?.send_files(peer, files).await.map_err(|e| e.to_string())?;
        Ok(hex::encode(id))
    }

    pub async fn share_text(&self, device: &str, text: &str) -> Res<String> {
        let peer = parse_device_id(device)?;
        let id = self.node().await?.share_text(peer, text.to_owned()).await.map_err(|e| e.to_string())?;
        Ok(hex::encode(id))
    }

    pub async fn transfers(&self) -> Res<Vec<TransferDto>> {
        let list = self.node().await?.transfers().await.map_err(|e| e.to_string())?;
        let members = self.members();
        Ok(list.iter().map(|t| transfer::transfer_dto(t, &members)).collect())
    }

    pub async fn accept_transfer(&self, id: &str, dir: Option<PathBuf>) -> Res<()> {
        let dir = dir.unwrap_or_else(|| self.receive_dir());
        self.node().await?.accept_transfer(parse_transfer_id(id)?, dir).await.map_err(|e| e.to_string())
    }

    pub async fn cancel_transfer(&self, id: &str) -> Res<()> {
        self.node().await?.cancel_transfer(parse_transfer_id(id)?).await.map_err(|e| e.to_string())
    }

    pub async fn clear_transfers(&self) -> Res<()> {
        self.node().await?.clear_transfers().await.map_err(|e| e.to_string())?;
        self.0.host.emit(UiEvent::Changed("transfers"));
        Ok(())
    }

    pub async fn resumed(&self, slept: Duration) {
        if let Ok(node) = self.node().await {
            node.resumed(slept).await;
        }
    }

    pub async fn network_changed(&self) {
        if let Ok(node) = self.node().await {
            node.network_changed().await;
        }
    }

    pub async fn local_copy(&self, reps: Vec<(String, Vec<u8>)>, source: Option<&str>) -> bool {
        if self.is_paused() || reps.is_empty() {
            return false;
        }
        let settings = self.settings();
        if settings.is_excluded(source) {
            info!("来源程序在排除列表中，不同步");
            return false;
        }
        if settings.screenshots
            && let Some((_, png)) = reps.iter().find(|(m, d)| m == MIME_PNG && d.len() <= MAX_IMAGE)
        {
            if settings.is_screenshot_source(source) {
                self.send_screenshot(png.clone()).await;
            } else {
                info!(source = source.map(settings::file_name), "图片不是来自截图工具，不作为截图发送");
            }
        }
        let reps: Vec<Rep> = reps
            .into_iter()
            .filter(|(mime, data)| mime != MIME_PNG || (settings.sync_images && data.len() <= MAX_IMAGE))
            .map(|(mime, data)| Rep::from_bytes(mime, data))
            .collect();
        let Some(first) = reps.first() else {
            debug!("没有可以同步的内容（例如关闭了同步图片）");
            return false;
        };
        let Ok(node) = self.node().await else { return false };
        if node.is_echo(first.hash).await.unwrap_or(false) {
            debug!("刚写入的远端内容的回声，不再同步");
            return false;
        }

        if settings::is_remote_control_source(source) && node.is_current_content(first.hash).await.unwrap_or(false) {
            info!(source = source.map(settings::file_name), "远程控制软件同步来的内容与最新一条相同，不再同步");
            return false;
        }
        let all: Vec<u8> = reps.iter().flat_map(|r| r.hash).collect();
        let fingerprint = copycat_crypto::kdf::hash(&all);
        {
            let mut last = self.0.last_copy.lock().unwrap_or_else(|p| p.into_inner());
            if last.is_some_and(|(h, at)| h == fingerprint && at.elapsed() < DUPLICATE_COPY) {
                debug!("同一内容刚刚复制过（程序连续写了两遍剪贴板），不再同步");
                return false;
            }
            *last = Some((fingerprint, Instant::now()));
        }
        let size: u64 = reps.iter().map(|r| r.size).sum();
        let mimes: Vec<String> = reps.iter().map(|r| r.mime.clone()).collect();
        let peers = node.status().await.map(|s| s.peers.iter().filter(|p| p.relay || p.lan).count()).unwrap_or(0);
        match node.copy(reps).await {
            Ok(_) => {
                info!(size, ?mimes, online = peers, "本机复制，已交给同步引擎");
                true
            }
            Err(e) => {
                warn!(error = %e, "同步失败");
                false
            }
        }
    }
}

impl<H: Host> Service<H> {
    async fn send_screenshot(&self, png: Vec<u8>) {
        let hash = copycat_crypto::kdf::hash(&png);
        {
            let mut last = self.0.last_shot.lock().unwrap_or_else(|p| p.into_inner());
            if last.is_some_and(|(h, t)| h == hash && t.elapsed() < SCREENSHOT_DEDUP) {
                return;
            }
            *last = Some((hash, Instant::now()));
        }
        let Ok(node) = self.node().await else { return };
        let Ok(status) = node.status().await else { return };
        let name = self.0.host.screenshot_name();
        let targets: Vec<_> = status.peers.iter().filter(|p| p.screenshots && (p.relay || p.lan)).collect();
        if targets.is_empty() {
            info!("没有在线且开启了接收截图的设备，截图不发送");
        }
        for p in targets {
            match node.send_screenshot(p.id, name.clone(), png.clone()).await {
                Ok(_) => info!(peer = %p.id.short(), size = png.len(), "截图已发出"),
                Err(e) => warn!(error = %e, "发送截图失败"),
            }
        }
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TestHost;

    impl Host for TestHost {
        fn platform(&self) -> Platform {
            Platform::Android
        }
        fn default_settings(&self) -> Settings {
            Settings { device_name: "手机".into(), ..Settings::default() }
        }
        fn master_key(&self, _: &Path) -> Res<[u8; 32]> {
            Ok([7; 32])
        }
        async fn write_clipboard(&self, _: ClipItem, _: bool) -> Res<()> {
            Ok(())
        }
        fn emit(&self, _: UiEvent) {}
        fn default_receive_dir(&self) -> PathBuf {
            std::env::temp_dir()
        }
        fn open_url(&self, _: &str) {}
        fn transfer_finished(&self, _: &TransferDto) {}
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn leave_group_resets_to_onboarding() {
        let dir = tempfile::tempdir().unwrap();
        let svc = Service::new(TestHost, dir.path().to_path_buf()).unwrap();
        svc.create_group("手机").await.unwrap();
        assert!(svc.overview().await.unwrap().initialized);

        let r =
            tokio::time::timeout(Duration::from_secs(20), svc.leave_group()).await.expect("退出设备组卡住了").unwrap();
        assert_eq!((r.peers, r.relay), (0, false));
        assert!(!svc.overview().await.unwrap().initialized);
        assert!(!dir.path().join("copycat.db").exists());
        assert_eq!(svc.settings().device_name, "手机", "本机设置应保留");

        svc.create_group("手机").await.unwrap();
        assert!(svc.overview().await.unwrap().initialized);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn node_restarts_after_unexpected_stop() {
        let dir = tempfile::tempdir().unwrap();
        let svc = Service::new(TestHost, dir.path().to_path_buf()).unwrap();
        svc.create_group("电脑").await.unwrap();
        let old = svc.node().await.unwrap();
        old.shutdown().await;
        assert!(old.is_stopped());
        let restarted = tokio::time::timeout(Duration::from_secs(20), async {
            loop {
                if let Ok(n) = svc.node().await
                    && !n.is_stopped()
                {
                    return n;
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        })
        .await
        .expect("节点没有自动重新启动");
        assert_eq!(restarted.id(), old.id(), "应从本地数据恢复同一台设备");

        svc.shutdown().await;
        tokio::time::sleep(Duration::from_secs(3)).await;
        assert!(svc.node().await.is_err(), "主动关闭后不应自动重启");
    }

    #[test]
    fn compose_gets_port_and_key() {
        let c = compose_for("relay.example.com:9443", "ccr1-abc");
        assert!(c.contains("\"9443:8443/udp\"") && c.contains("\"9443:8443/tcp\""), "{c}");
        assert!(c.contains("COPYCAT_RELAY_KEY: \"ccr1-abc\""), "{c}");
        assert!(!c.contains("COPYCAT_PORT"), "端口应已填好：{c}");
    }
}
