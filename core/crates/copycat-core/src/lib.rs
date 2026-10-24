pub(crate) mod actor;
mod join;
mod lan;
mod link;
mod supervisor;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use copycat_crypto::{CryptoError, DeviceIdentity, Roster};
use copycat_store::{Store, StoreError};
use copycat_transport::TransportError;
use thiserror::Error;
use tokio::sync::{mpsc, oneshot};

pub use copycat_crypto::{DeviceId, DeviceRecord, Platform};
pub use copycat_sync::{ClipItem, EngineConfig, Hlc, Rep};
pub use copycat_transport::lan::local_lan_ips;
pub use copycat_transport::relay_client::RelayAddr;
pub use copycat_transport::tls::{RelayKey, Tuning};

#[derive(Debug, Error)]
pub enum CoreError {
    #[error("存储错误: {0}")]
    Store(#[from] StoreError),
    #[error("密码学错误: {0}")]
    Crypto(#[from] CryptoError),
    #[error("传输错误: {0}")]
    Transport(#[from] TransportError),
    #[error("本机尚未加入设备组")]
    NotInitialized,
    #[error("本机已经加入了设备组")]
    AlreadyInitialized,
    #[error("配对失败: {0}")]
    Pairing(String),
    #[error("当前状态不允许该操作: {0}")]
    InvalidState(&'static str),
    #[error("文件操作失败: {0}")]
    Io(String),
    #[error("节点已停止")]
    Stopped,
}

pub type Result<T, E = CoreError> = core::result::Result<T, E>;

#[derive(Clone, Debug)]
pub struct NodeConfig {
    pub device_name: String,
    pub platform: Platform,
    pub tuning: Tuning,
    pub engine: EngineConfig,

    pub mailbox_ttl_secs: u32,
    pub lan: LanConfig,

    pub relay_enabled: bool,
    pub options: Options,

    pub cache_dir: PathBuf,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Options {
    pub fetch_images: bool,

    pub fetch_max: u64,

    pub accept_screenshots: bool,

    pub history_days: u32,
}

impl Default for Options {
    fn default() -> Self {
        Self { fetch_images: true, fetch_max: 64 * 1024 * 1024, accept_screenshots: false, history_days: 1 }
    }
}

impl Options {
    pub fn retention(&self) -> (usize, u64) {
        let days = self.history_days.clamp(1, 365);
        let per_origin = match days {
            1 => 50,
            2..=7 => 200,
            _ => 500,
        };
        (per_origin, u64::from(days) * 24 * 3600 * 1000)
    }
}

#[derive(Clone, Debug)]
pub struct LanConfig {
    pub enabled: bool,

    pub port: u16,

    pub loopback_for_tests: bool,

    pub discovery: bool,

    pub discovery_port: u16,

    pub discovery_targets: Vec<std::net::SocketAddr>,
}

impl Default for LanConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            port: copycat_transport::lan::DEFAULT_PORT,
            loopback_for_tests: false,
            discovery: true,
            discovery_port: copycat_transport::discovery::DISCOVERY_PORT,
            discovery_targets: Vec::new(),
        }
    }
}

impl NodeConfig {
    pub fn new(device_name: impl Into<String>, platform: Platform) -> Self {
        Self {
            device_name: device_name.into(),
            platform,
            tuning: Tuning::default(),
            engine: EngineConfig::default(),
            mailbox_ttl_secs: 24 * 3600,
            lan: LanConfig::default(),
            relay_enabled: true,
            options: Options::default(),
            cache_dir: std::env::temp_dir().join("copycat-cache"),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum NodeEvent {
    Apply(ClipItem),

    HistoryChanged,
    PeerUp(DeviceId),
    PeerDown(DeviceId),

    LanPath { peer: DeviceId, up: bool },
    RelayConnected,
    RelayDisconnected(String),

    RelayChanged,

    PairRequest { name: String, platform: Platform, sas: String },
    Paired(DeviceId),
    RosterChanged,

    Revoked,

    Transfer(TransferInfo),

    SharedText { from: DeviceId, text: String },
}

#[derive(Debug)]
pub struct OutgoingFile {
    pub name: String,
    pub source: FileSource,
}

#[derive(Debug)]
pub enum FileSource {
    Path(PathBuf),

    Open(std::fs::File),

    Memory(Vec<u8>),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TransferKind {
    Files,

    Screenshot,
}

pub type TransferId = [u8; 16];

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TransferState {
    Preparing,

    Waiting,

    Asking,
    Active,
    Done,
    Failed(String),
    Cancelled,
}

impl TransferState {
    pub fn is_finished(&self) -> bool {
        matches!(self, TransferState::Done | TransferState::Failed(_) | TransferState::Cancelled)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TransferInfo {
    pub id: TransferId,
    pub peer: DeviceId,
    pub outgoing: bool,
    pub kind: TransferKind,

    pub files: Vec<(String, u64)>,

    pub text: Option<String>,
    pub total: u64,
    pub done: u64,

    pub rate: u64,
    pub state: TransferState,

    pub lan: bool,

    pub dir: Option<PathBuf>,
    pub saved: Vec<PathBuf>,
    pub created_ms: u64,
    pub finished_ms: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PeerStatus {
    pub id: DeviceId,

    pub relay: bool,

    pub lan: bool,

    pub screenshots: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LeaveReport {
    pub peers: usize,

    pub relay: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NodeStatus {
    pub me: DeviceId,

    pub relay_addr: Option<String>,
    pub relay_connected: bool,
    pub relay_enabled: bool,
    pub lan_port: Option<u16>,
    pub peers: Vec<PeerStatus>,
}

pub(crate) enum Cmd {
    Copy(Vec<Rep>, oneshot::Sender<Result<()>>),
    IsEcho([u8; 32], oneshot::Sender<bool>),
    IsCurrent([u8; 32], oneshot::Sender<bool>),
    CreateInvite(oneshot::Sender<Result<String>>),
    RespondPairing(bool, oneshot::Sender<Result<DeviceId>>),
    Remove(DeviceId, oneshot::Sender<Result<()>>),
    Leave(oneshot::Sender<Result<LeaveReport>>),
    Members(oneshot::Sender<Vec<DeviceRecord>>),
    History(oneshot::Sender<Vec<ClipItem>>),
    OnlinePeers(oneshot::Sender<Vec<DeviceId>>),
    Status(oneshot::Sender<NodeStatus>),
    Forget([u8; 16], oneshot::Sender<()>),
    ForgetAll(oneshot::Sender<()>),

    ForgetEverywhere(Option<[u8; 16]>, oneshot::Sender<usize>),
    NetworkChanged,
    Resumed(std::time::Duration),
    SetRelayEnabled(bool),
    Relay(RelayCmd, oneshot::Sender<Result<Option<String>>>),
    SendFiles(DeviceId, Vec<OutgoingFile>, TransferKind, oneshot::Sender<Result<TransferId>>),
    ShareText(DeviceId, String, oneshot::Sender<Result<TransferId>>),
    AcceptTransfer(TransferId, PathBuf, oneshot::Sender<Result<()>>),
    CancelTransfer(TransferId, oneshot::Sender<Result<()>>),
    Transfers(oneshot::Sender<Vec<TransferInfo>>),
    ClearTransfers(oneshot::Sender<()>),
    SetOptions(Options),
    FetchItem([u8; 16], oneshot::Sender<Result<ClipItem>>),
    Shutdown(oneshot::Sender<()>),
}

pub(crate) enum RelayCmd {
    Generate(String),

    Address(String),

    Remove,

    Secret,
}

#[derive(Clone)]
pub struct Node {
    tx: mpsc::Sender<Cmd>,
    id: DeviceId,
}

pub(crate) fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

const SETTING_RELAY_ADDR: &str = "relay_addr";
const SETTING_RELAY_KEY: &str = "relay_key";
const SETTING_RELAY_VERSION: &str = "relay_version";
const SETTING_RELAY_AUTHOR: &str = "relay_author";

const SETTING_RELAY_SECRET: &str = "relay_secret";

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct RelayConf {
    pub relay: Option<RelayAddr>,

    pub version: u64,
    pub author: Option<DeviceId>,
}

impl RelayConf {
    pub fn order(&self) -> (u64, [u8; 16]) {
        (self.version, self.author.map(|a| a.0).unwrap_or_default())
    }
}

pub(crate) fn save_relay(store: &Store, c: &RelayConf) -> Result<()> {
    let (addr, key) = match &c.relay {
        Some(r) => (r.addr.as_bytes(), r.key.to_vec()),
        None => (&b""[..], Vec::new()),
    };
    store.set_setting(SETTING_RELAY_ADDR, addr)?;
    store.set_setting(SETTING_RELAY_KEY, &key)?;
    store.set_setting(SETTING_RELAY_VERSION, &c.version.to_be_bytes())?;
    store.set_setting(SETTING_RELAY_AUTHOR, c.author.as_ref().map_or(&[][..], |a| &a.0[..]))?;
    Ok(())
}

pub(crate) fn load_relay(store: &Store) -> Result<RelayConf> {
    let addr = store.setting(SETTING_RELAY_ADDR)?.and_then(|v| String::from_utf8(v).ok()).unwrap_or_default();
    let key = store.setting(SETTING_RELAY_KEY)?.and_then(|v| <[u8; 32]>::try_from(v.as_slice()).ok());
    let version = store
        .setting(SETTING_RELAY_VERSION)?
        .and_then(|v| <[u8; 8]>::try_from(v.as_slice()).ok())
        .map_or(0, u64::from_be_bytes);
    let author = store.setting(SETTING_RELAY_AUTHOR)?.and_then(|v| DeviceId::from_slice(&v).ok());
    let relay = match key {
        Some(key) if !addr.is_empty() => Some(RelayAddr { addr, key }),
        _ => None,
    };
    Ok(RelayConf { relay, version, author })
}

pub(crate) fn save_relay_secret(store: &Store, secret: &str) -> Result<()> {
    Ok(store.set_setting(SETTING_RELAY_SECRET, secret.as_bytes())?)
}

pub(crate) fn load_relay_secret(store: &Store) -> Result<Option<RelayKey>> {
    let Some(v) = store.setting(SETTING_RELAY_SECRET)? else { return Ok(None) };
    Ok(std::str::from_utf8(&v).ok().and_then(|s| RelayKey::parse(s).ok()))
}

impl Node {
    pub async fn create_group(mut store: Store, cfg: NodeConfig) -> Result<(Node, mpsc::Receiver<NodeEvent>)> {
        if store.load_identity()?.is_some() {
            return Err(CoreError::AlreadyInitialized);
        }
        let identity = DeviceIdentity::generate()?;
        let record = DeviceRecord::issue(&identity, &cfg.device_name, cfg.platform, now_ms())?;
        let (roster, _genesis) = Roster::create(&identity, &record, now_ms())?;
        store.save_identity(&identity)?;
        store.set_group(roster.group_id())?;
        store.add_roster_ops(&roster.ops_raw())?;
        save_relay(&store, &RelayConf::default())?;
        Ok(actor::spawn(store, cfg, Arc::new(identity), roster, Vec::new()))
    }

    pub async fn open(store: Store, cfg: NodeConfig) -> Result<(Node, mpsc::Receiver<NodeEvent>)> {
        let identity = store.load_identity()?.ok_or(CoreError::NotInitialized)?;
        let group = store.group()?.ok_or(CoreError::NotInitialized)?;
        let ops = store.roster_ops()?;
        let roster = Roster::from_ops(&group, ops.iter().map(Vec::as_slice))?;
        Ok(actor::spawn(store, cfg, Arc::new(identity), roster, Vec::new()))
    }

    pub async fn join(
        store: Store,
        cfg: NodeConfig,
        invite_uri: &str,
        on_sas: impl FnOnce(&str) + Send,
    ) -> Result<(Node, mpsc::Receiver<NodeEvent>)> {
        join::join(store, cfg, invite_uri, on_sas).await
    }

    pub fn id(&self) -> DeviceId {
        self.id
    }

    pub fn is_stopped(&self) -> bool {
        self.tx.is_closed()
    }

    async fn call<T>(&self, make: impl FnOnce(oneshot::Sender<T>) -> Cmd) -> Result<T> {
        let (tx, rx) = oneshot::channel();
        self.tx.send(make(tx)).await.map_err(|_| CoreError::Stopped)?;
        rx.await.map_err(|_| CoreError::Stopped)
    }

    pub async fn copy(&self, reps: Vec<Rep>) -> Result<()> {
        self.call(|tx| Cmd::Copy(reps, tx)).await?
    }

    pub async fn is_echo(&self, fingerprint: [u8; 32]) -> Result<bool> {
        self.call(|tx| Cmd::IsEcho(fingerprint, tx)).await
    }

    pub async fn is_current_content(&self, fingerprint: [u8; 32]) -> Result<bool> {
        self.call(|tx| Cmd::IsCurrent(fingerprint, tx)).await
    }

    pub async fn create_invite(&self) -> Result<String> {
        self.call(Cmd::CreateInvite).await?
    }

    pub async fn respond_pairing(&self, accept: bool) -> Result<DeviceId> {
        self.call(|tx| Cmd::RespondPairing(accept, tx)).await?
    }

    pub async fn remove_device(&self, id: DeviceId) -> Result<()> {
        self.call(|tx| Cmd::Remove(id, tx)).await?
    }

    pub async fn leave_group(&self) -> Result<LeaveReport> {
        self.call(Cmd::Leave).await?
    }

    pub async fn members(&self) -> Result<Vec<DeviceRecord>> {
        self.call(Cmd::Members).await
    }

    pub async fn history(&self) -> Result<Vec<ClipItem>> {
        self.call(Cmd::History).await
    }

    pub async fn online_peers(&self) -> Result<Vec<DeviceId>> {
        self.call(Cmd::OnlinePeers).await
    }

    pub async fn status(&self) -> Result<NodeStatus> {
        self.call(Cmd::Status).await
    }

    pub async fn forget(&self, id: [u8; 16]) -> Result<()> {
        self.call(|tx| Cmd::Forget(id, tx)).await
    }

    pub async fn forget_all(&self) -> Result<()> {
        self.call(Cmd::ForgetAll).await
    }

    pub async fn forget_all_everywhere(&self) -> Result<usize> {
        self.forget_all().await?;
        self.call(|tx| Cmd::ForgetEverywhere(None, tx)).await
    }

    pub async fn network_changed(&self) {
        let _ = self.tx.send(Cmd::NetworkChanged).await;
    }

    pub async fn resumed(&self, slept: std::time::Duration) {
        let _ = self.tx.send(Cmd::Resumed(slept)).await;
    }

    pub async fn set_relay_enabled(&self, enabled: bool) {
        let _ = self.tx.send(Cmd::SetRelayEnabled(enabled)).await;
    }

    async fn relay_cmd(&self, c: RelayCmd) -> Result<Option<String>> {
        self.call(|tx| Cmd::Relay(c, tx)).await?
    }

    pub async fn setup_relay(&self, addr: String) -> Result<String> {
        self.relay_cmd(RelayCmd::Generate(addr)).await?.ok_or(CoreError::InvalidState("未能生成中转密钥"))
    }

    pub async fn set_relay_addr(&self, addr: String) -> Result<()> {
        self.relay_cmd(RelayCmd::Address(addr)).await.map(|_| ())
    }

    pub async fn remove_relay(&self) -> Result<()> {
        self.relay_cmd(RelayCmd::Remove).await.map(|_| ())
    }

    pub async fn relay_secret(&self) -> Result<Option<String>> {
        self.relay_cmd(RelayCmd::Secret).await
    }

    pub async fn send_files(&self, peer: DeviceId, files: Vec<OutgoingFile>) -> Result<TransferId> {
        self.call(|tx| Cmd::SendFiles(peer, files, TransferKind::Files, tx)).await?
    }

    pub async fn send_screenshot(&self, peer: DeviceId, name: String, png: Vec<u8>) -> Result<TransferId> {
        let files = vec![OutgoingFile { name, source: FileSource::Memory(png) }];
        self.call(|tx| Cmd::SendFiles(peer, files, TransferKind::Screenshot, tx)).await?
    }

    pub async fn set_options(&self, options: Options) {
        let _ = self.tx.send(Cmd::SetOptions(options)).await;
    }

    pub async fn fetch_item(&self, id: [u8; 16]) -> Result<ClipItem> {
        self.call(|tx| Cmd::FetchItem(id, tx)).await?
    }

    pub async fn share_text(&self, peer: DeviceId, text: String) -> Result<TransferId> {
        self.call(|tx| Cmd::ShareText(peer, text, tx)).await?
    }

    pub async fn accept_transfer(&self, id: TransferId, dir: PathBuf) -> Result<()> {
        self.call(|tx| Cmd::AcceptTransfer(id, dir, tx)).await?
    }

    pub async fn cancel_transfer(&self, id: TransferId) -> Result<()> {
        self.call(|tx| Cmd::CancelTransfer(id, tx)).await?
    }

    pub async fn transfers(&self) -> Result<Vec<TransferInfo>> {
        self.call(Cmd::Transfers).await
    }

    pub async fn clear_transfers(&self) -> Result<()> {
        self.call(Cmd::ClearTransfers).await
    }

    pub async fn shutdown(&self) {
        let _ = self.call(Cmd::Shutdown).await;
    }
}

#[doc(hidden)]
pub mod fuzzing {

    pub fn sanitize_name(name: &str) -> Option<String> {
        crate::actor::files::sanitize(name)
    }
}
