use std::collections::{HashMap, HashSet, VecDeque};
use std::fs::{File, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use copycat_crypto::{DeviceId, random};
use copycat_proto::app::{self, AppMessage, app_message::Body as A};
use tokio::sync::mpsc;
use tracing::{info, warn};

use super::Actor;
use crate::{
    CoreError, FileSource, NodeEvent, OutgoingFile, Result, TransferId, TransferInfo, TransferKind, TransferState,
    now_ms,
};

const CHUNK: u32 = 256 * 1024;
const MIN_CHUNK: u32 = 16 * 1024;
const MAX_CHUNK: u32 = 4 * 1024 * 1024;

const MAX_CHUNKS: u64 = 1 << 22;

const WINDOW_LAN: u64 = 8 * 1024 * 1024;
const WINDOW_RELAY: u64 = 1024 * 1024;

const RELAY_RATE: f64 = 6.0 * 1024.0 * 1024.0;
const RELAY_BURST: f64 = 1024.0 * 1024.0;

const STALL: Duration = Duration::from_secs(15);

const GIVE_UP: Duration = Duration::from_secs(30 * 60);

const ASK_TTL: Duration = Duration::from_secs(10 * 60);
const PROGRESS_EVERY: Duration = Duration::from_millis(250);
const MAX_FILES: usize = 10_000;
const MAX_FILE_SIZE: u64 = 1 << 40;
pub(crate) const MAX_TEXT: usize = 256 * 1024;
const MAX_NAME: usize = 1024;
const MAX_DEPTH: usize = 16;
const MAX_COMPONENT: usize = 200;

const KEEP_FINISHED: usize = 100;

const MAX_ACTIVE_IN: usize = 8;

const KEEP_REJECTED: usize = 128;
const HASH_BLOCK: usize = 1024 * 1024;

pub(crate) enum FileIn {
    Hashed { id: TransferId, res: std::result::Result<Vec<OutFile>, String> },
    Read { id: TransferId, key: (u32, u64), res: io::Result<Vec<u8>> },
    Written { id: TransferId, key: (u32, u64), res: std::result::Result<(), String> },

    Saved { id: TransferId, file: u32, res: std::result::Result<PathBuf, String> },
}

pub(crate) struct Files {
    io: mpsc::UnboundedSender<FileIn>,
    outs: HashMap<TransferId, Out>,
    ins: HashMap<TransferId, In>,
    rejected: VecDeque<(TransferId, &'static str)>,
    relay_tokens: f64,
    relay_at: Instant,

    pub(super) wake: Option<Instant>,
}

impl Files {
    pub(super) fn new(io: mpsc::UnboundedSender<FileIn>) -> Self {
        Self {
            io,
            outs: HashMap::new(),
            ins: HashMap::new(),
            rejected: VecDeque::new(),
            relay_tokens: RELAY_BURST,
            relay_at: Instant::now(),
            wake: None,
        }
    }
}

struct Progress {
    total: u64,
    done: u64,
    rate: f64,
    sampled_at: Instant,
    sampled_done: u64,
    created_ms: u64,
    finished_ms: Option<u64>,
}

impl Progress {
    fn new(total: u64) -> Self {
        Self {
            total,
            done: 0,
            rate: 0.0,
            sampled_at: Instant::now(),
            sampled_done: 0,
            created_ms: now_ms(),
            finished_ms: None,
        }
    }

    fn due(&mut self) -> bool {
        let dt = self.sampled_at.elapsed();
        if dt < PROGRESS_EVERY {
            return false;
        }
        let inst = self.done.saturating_sub(self.sampled_done) as f64 / dt.as_secs_f64();
        self.rate = if self.rate == 0.0 { inst } else { 0.6 * self.rate + 0.4 * inst };
        self.sampled_at = Instant::now();
        self.sampled_done = self.done;
        true
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Kind {
    Files,
    Screenshot,

    Clip([u8; 16]),
}

impl Kind {
    fn public(self) -> TransferKind {
        match self {
            Kind::Screenshot => TransferKind::Screenshot,
            Kind::Files | Kind::Clip(_) => TransferKind::Files,
        }
    }

    pub(super) fn from_public(k: TransferKind) -> Self {
        match k {
            TransferKind::Files => Kind::Files,
            TransferKind::Screenshot => Kind::Screenshot,
        }
    }

    pub(super) fn is_clip(self) -> bool {
        matches!(self, Kind::Clip(_))
    }
}

#[derive(Clone)]
enum Src {
    File(Arc<File>),
    Mem(Arc<Vec<u8>>),
}

impl Src {
    fn read_at(&self, buf: &mut [u8], off: u64) -> io::Result<()> {
        match self {
            Src::File(f) => read_exact_at(f, buf, off),
            Src::Mem(m) => {
                let start = usize::try_from(off).map_err(|_| io::Error::from(io::ErrorKind::UnexpectedEof))?;
                let part = m.get(start..start + buf.len()).ok_or(io::ErrorKind::UnexpectedEof)?;
                buf.copy_from_slice(part);
                Ok(())
            }
        }
    }
}

pub(crate) struct OutFile {
    size: u64,
    hash: [u8; 32],

    src: Option<Src>,
}

struct Out {
    peer: DeviceId,
    kind: Kind,
    names: Vec<String>,
    files: Vec<OutFile>,
    text: Option<String>,
    state: TransferState,
    todo: VecDeque<(u32, u64)>,
    acked: Vec<Vec<bool>>,
    inflight: HashMap<(u32, u64), u64>,
    last_offer: Instant,

    last_progress: Instant,
    cancel: Arc<AtomicBool>,
    p: Progress,
}

impl Out {
    fn new(peer: DeviceId, kind: Kind, names: Vec<String>, text: Option<String>, state: TransferState) -> Self {
        let total = text.as_ref().map_or(0, |t| t.len() as u64);
        Self {
            peer,
            kind,
            names,
            files: Vec::new(),
            text,
            state,
            todo: VecDeque::new(),
            acked: Vec::new(),
            inflight: HashMap::new(),
            last_offer: Instant::now(),
            last_progress: Instant::now(),
            cancel: Arc::new(AtomicBool::new(false)),
            p: Progress::new(total),
        }
    }

    fn offer(&self, id: TransferId) -> AppMessage {
        let files = self
            .names
            .iter()
            .zip(&self.files)
            .map(|(name, f)| app::FileEntry { name: name.clone(), size: f.size, hash: f.hash.to_vec() })
            .collect();
        let (kind, clip_item) = match self.kind {
            Kind::Files => (app::FILE_KIND_FILES, Vec::new()),
            Kind::Screenshot => (app::FILE_KIND_SCREENSHOT, Vec::new()),
            Kind::Clip(item) => (app::FILE_KIND_CLIP, item.to_vec()),
        };
        AppMessage::new(A::FileOffer(app::FileOffer {
            transfer_id: id.to_vec(),
            files,
            chunk_size: CHUNK,
            text: self.text.clone(),
            kind,
            clip_item,
        }))
    }

    fn finish(&mut self, state: TransferState) {
        self.state = state;
        self.cancel.store(true, Ordering::Relaxed);
        self.p.finished_ms = Some(now_ms());
        self.todo.clear();
        self.inflight.clear();
        for f in &mut self.files {
            f.src = None;
        }
    }
}

pub(super) struct InFile {
    pub(super) name: String,
    pub(super) size: u64,
    pub(super) hash: [u8; 32],
    have: Vec<bool>,
    saving: bool,
    saved: bool,

    pub(super) path: Option<PathBuf>,
}

pub(super) struct In {
    pub(super) peer: DeviceId,
    pub(super) kind: Kind,
    pub(super) files: Vec<InFile>,
    text: Option<String>,
    chunk: u32,
    pub(super) state: TransferState,
    dir: Option<PathBuf>,
    writer: Option<mpsc::UnboundedSender<Job>>,
    saved: Vec<PathBuf>,
    created: Instant,
    last_rx: Instant,
    p: Progress,
}

impl In {
    fn accept_msg(&self, id: TransferId) -> AppMessage {
        AppMessage::new(A::FileAccept(app::FileAccept {
            transfer_id: id.to_vec(),
            have: self.files.iter().map(|f| bitmap(&f.have)).collect(),
        }))
    }

    fn finish(&mut self, state: TransferState) {
        self.state = state;
        self.p.finished_ms = Some(now_ms());

        self.writer = None;
        for f in &mut self.files {
            f.have = Vec::new();
        }
    }
}

fn chunk_count(size: u64, chunk: u32) -> u64 {
    size.div_ceil(u64::from(chunk))
}

fn chunk_len(size: u64, chunk: u32, idx: u64) -> u64 {
    (size - idx * u64::from(chunk)).min(u64::from(chunk))
}

fn bitmap(have: &[bool]) -> Vec<u8> {
    let mut out = vec![0u8; have.len().div_ceil(8)];
    for (i, h) in have.iter().enumerate() {
        if *h {
            out[i / 8] |= 1 << (i % 8);
        }
    }
    out
}

fn unbitmap(bytes: &[u8], n: usize) -> Vec<bool> {
    (0..n).map(|i| bytes.get(i / 8).is_some_and(|b| b & (1 << (i % 8)) != 0)).collect()
}

fn tid(raw: &[u8]) -> Option<TransferId> {
    raw.try_into().ok()
}

pub(super) fn cancel_msg(id: TransferId, reason: &str) -> AppMessage {
    AppMessage::new(A::FileCancel(app::FileCancel { transfer_id: id.to_vec(), reason: reason.to_owned() }))
}

fn done_msg(id: TransferId) -> AppMessage {
    AppMessage::new(A::FileDone(app::FileDone { transfer_id: id.to_vec() }))
}

const RESERVED: [&str; 22] = [
    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8", "COM9", "LPT1", "LPT2",
    "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
];

pub(crate) fn sanitize(name: &str) -> Option<String> {
    if name.len() > MAX_NAME {
        return None;
    }
    let mut out = Vec::new();
    for comp in name.split(['/', '\\']) {
        if comp.is_empty() || comp == "." {
            continue;
        }
        if comp == ".." {
            return None;
        }
        let mut c: String =
            comp.chars().map(|ch| if ch.is_control() || "<>:\"|?*".contains(ch) { '_' } else { ch }).collect();

        clip_component(&mut c);
        if c.is_empty() {
            c.push('_');
        }
        let stem = c.split('.').next().unwrap_or_default().trim_end().to_ascii_uppercase();
        if RESERVED.contains(&stem.as_str()) {
            c.insert(0, '_');
            clip_component(&mut c);
        }
        out.push(c);
    }
    if out.is_empty() || out.len() > MAX_DEPTH {
        return None;
    }
    Some(out.join("/"))
}

fn clip_component(c: &mut String) {
    if c.len() > MAX_COMPONENT {
        let mut cut = MAX_COMPONENT;
        while !c.is_char_boundary(cut) {
            cut -= 1;
        }
        c.truncate(cut);
    }
    let keep = c.trim_end_matches([' ', '.']).len();
    c.truncate(keep);
}

fn validate_offer(o: &app::FileOffer) -> std::result::Result<Vec<InFile>, &'static str> {
    if !(MIN_CHUNK..=MAX_CHUNK).contains(&o.chunk_size) {
        return Err("分块大小不合法");
    }
    if o.files.is_empty() || o.files.len() > MAX_FILES {
        return Err("文件数量不合法");
    }
    o.files
        .iter()
        .map(|f| {
            let hash: [u8; 32] = f.hash.as_slice().try_into().map_err(|_| "文件哈希不合法")?;
            if f.size > MAX_FILE_SIZE || chunk_count(f.size, o.chunk_size) > MAX_CHUNKS {
                return Err("文件过大");
            }
            let name = sanitize(&f.name).ok_or("文件名不合法")?;
            let n = chunk_count(f.size, o.chunk_size) as usize;
            Ok(InFile { name, size: f.size, hash, have: vec![false; n], saving: false, saved: false, path: None })
        })
        .collect()
}

impl Actor {
    fn check_target(&self, peer: DeviceId) -> Result<()> {
        if self.revoked {
            return Err(CoreError::InvalidState("本机已被移出设备组"));
        }
        if peer == self.me || !self.roster.is_member(&peer) {
            return Err(CoreError::InvalidState("目标设备不在设备组中"));
        }
        Ok(())
    }

    pub(super) async fn send_files(
        &mut self,
        peer: DeviceId,
        files: Vec<OutgoingFile>,
        kind: Kind,
    ) -> Result<TransferId> {
        self.check_target(peer)?;
        if files.is_empty() || files.len() > MAX_FILES {
            return Err(CoreError::InvalidState("文件数量不合适"));
        }
        if files.iter().any(|f| sanitize(&f.name).is_none()) {
            return Err(CoreError::InvalidState("文件名不合法"));
        }
        let id = random::bytes::<16>()?;
        let out = Out::new(peer, kind, files.iter().map(|f| f.name.clone()).collect(), None, TransferState::Preparing);
        let (cancel, tx) = (out.cancel.clone(), self.files.io.clone());
        tokio::task::spawn_blocking(move || {
            let res = open_and_hash(files, &cancel);
            let _ = tx.send(FileIn::Hashed { id, res });
        });
        self.files.outs.insert(id, out);
        self.emit_transfer(id, true).await;
        Ok(id)
    }

    pub(super) async fn share_text(&mut self, peer: DeviceId, text: String) -> Result<TransferId> {
        self.check_target(peer)?;
        if text.is_empty() || text.len() > MAX_TEXT {
            return Err(CoreError::InvalidState("文本为空或过长"));
        }
        let id = random::bytes::<16>()?;
        self.files.outs.insert(id, Out::new(peer, Kind::Files, Vec::new(), Some(text), TransferState::Waiting));
        self.offer(id).await;
        self.emit_transfer(id, true).await;
        Ok(id)
    }

    pub(super) async fn accept_transfer(&mut self, id: TransferId, dir: PathBuf) -> Result<()> {
        let i = self.files.ins.get_mut(&id).ok_or(CoreError::InvalidState("传输不存在"))?;
        if i.state != TransferState::Asking {
            return Err(CoreError::InvalidState("该传输已在进行或已结束"));
        }
        std::fs::create_dir_all(&dir).map_err(|e| CoreError::Io(format!("无法创建保存目录：{e}")))?;
        let writer = spawn_writer(id, dir.clone(), i.files.len(), self.files.io.clone());

        for (n, f) in i.files.iter_mut().enumerate() {
            if f.size == 0 {
                f.saving = true;
                let _ = writer.send(Job::Save { file: n as u32, name: f.name.clone(), size: 0, hash: f.hash });
            }
        }
        i.writer = Some(writer);
        i.dir = Some(dir);
        i.state = TransferState::Active;
        i.last_rx = Instant::now();
        let (peer, msg) = (i.peer, i.accept_msg(id));
        self.send_app(peer, msg).await;
        self.emit_transfer(id, true).await;
        Ok(())
    }

    pub(super) async fn accept_clip(
        &mut self,
        peer: DeviceId,
        id: TransferId,
        item: [u8; 16],
        files: Vec<InFile>,
        chunk: u32,
    ) -> Result<()> {
        self.files.ins.insert(id, new_in(peer, Kind::Clip(item), files, None, chunk, TransferState::Asking));
        let dir = self.cfg.cache_dir.clone();
        let r = self.accept_transfer(id, dir).await;
        if r.is_err() {
            self.files.ins.remove(&id);
        }
        r
    }

    pub(super) fn take_in(&mut self, id: &TransferId) -> Option<In> {
        self.files.ins.remove(id)
    }

    pub(super) fn clip_out(&self, peer: DeviceId, item: [u8; 16]) -> Option<TransferId> {
        self.files
            .outs
            .iter()
            .find(|(_, o)| o.peer == peer && o.kind == Kind::Clip(item) && !o.state.is_finished())
            .map(|(id, _)| *id)
    }

    pub(super) async fn reoffer(&mut self, id: TransferId) {
        self.offer(id).await;
    }

    pub(super) async fn cancel_transfer(&mut self, id: TransferId) -> Result<()> {
        if let Some(o) = self.files.outs.get_mut(&id) {
            if !o.state.is_finished() {
                o.finish(TransferState::Cancelled);
                let peer = o.peer;
                self.send_app(peer, cancel_msg(id, "对方取消了发送")).await;
                self.emit_transfer(id, true).await;
            }
            return Ok(());
        }
        if let Some(i) = self.files.ins.get_mut(&id) {
            if !i.state.is_finished() {
                let reason =
                    if i.state == TransferState::Asking { "对方拒绝接收" } else { "对方取消了接收" };
                i.finish(TransferState::Cancelled);
                let peer = i.peer;
                self.send_app(peer, cancel_msg(id, reason)).await;
                self.emit_transfer(id, true).await;
            }
            return Ok(());
        }
        Err(CoreError::InvalidState("传输不存在"))
    }

    pub(super) fn transfers(&self) -> Vec<TransferInfo> {
        let visible = self
            .files
            .outs
            .iter()
            .filter(|(_, o)| !o.kind.is_clip())
            .map(|(id, _)| id)
            .chain(self.files.ins.iter().filter(|(_, i)| !i.kind.is_clip()).map(|(id, _)| id));
        let mut v: Vec<TransferInfo> = visible.filter_map(|id| self.transfer_info(id)).collect();
        v.sort_by_key(|t| (t.state.is_finished(), std::cmp::Reverse(t.created_ms)));
        v
    }

    pub(super) fn clear_transfers(&mut self) {
        self.files.outs.retain(|_, o| !o.state.is_finished());
        self.files.ins.retain(|_, i| !i.state.is_finished());
    }

    fn transfer_info(&self, id: &TransferId) -> Option<TransferInfo> {
        if let Some(o) = self.files.outs.get(id) {
            return Some(TransferInfo {
                id: *id,
                peer: o.peer,
                outgoing: true,
                kind: o.kind.public(),
                files: o
                    .names
                    .iter()
                    .enumerate()
                    .map(|(n, name)| (name.clone(), o.files.get(n).map_or(0, |f| f.size)))
                    .collect(),
                text: o.text.clone(),
                total: o.p.total,
                done: o.p.done,
                rate: if o.state == TransferState::Active { o.p.rate as u64 } else { 0 },
                state: o.state.clone(),
                lan: self.lan_up(&o.peer),
                dir: None,
                saved: Vec::new(),
                created_ms: o.p.created_ms,
                finished_ms: o.p.finished_ms,
            });
        }
        let i = self.files.ins.get(id)?;
        Some(TransferInfo {
            id: *id,
            peer: i.peer,
            outgoing: false,
            kind: i.kind.public(),
            files: i.files.iter().map(|f| (f.name.clone(), f.size)).collect(),
            text: i.text.clone(),
            total: i.p.total,
            done: i.p.done,
            rate: if i.state == TransferState::Active { i.p.rate as u64 } else { 0 },
            state: i.state.clone(),
            lan: self.lan_up(&i.peer),
            dir: i.dir.clone(),
            saved: i.saved.clone(),
            created_ms: i.p.created_ms,
            finished_ms: i.p.finished_ms,
        })
    }

    async fn emit_transfer(&mut self, id: TransferId, force: bool) {
        let (clip, finished) = match (self.files.outs.get(&id), self.files.ins.get(&id)) {
            (Some(o), _) => (o.kind.is_clip(), o.state.is_finished()),
            (None, Some(i)) => (i.kind.is_clip(), i.state.is_finished()),
            (None, None) => return,
        };
        if clip {
            if finished {
                if self.files.ins.contains_key(&id) {
                    Box::pin(self.clip_transfer_finished(id)).await;
                }
                self.prune_transfers();
            }
            return;
        }
        let p = match (self.files.outs.get_mut(&id), self.files.ins.get_mut(&id)) {
            (Some(o), _) => &mut o.p,
            (None, Some(i)) => &mut i.p,
            (None, None) => return,
        };
        if !p.due() && !force {
            return;
        }
        if let Some(info) = self.transfer_info(&id) {
            self.emit(NodeEvent::Transfer(info)).await;
        }
    }

    async fn offer(&mut self, id: TransferId) {
        let Some(o) = self.files.outs.get_mut(&id) else { return };
        if o.state.is_finished() || o.state == TransferState::Preparing {
            return;
        }
        o.inflight.clear();
        o.todo.clear();
        o.last_offer = Instant::now();
        let (peer, msg) = (o.peer, o.offer(id));
        if self.peer_up(&peer) {
            self.send_app(peer, msg).await;
        }
    }

    pub(super) async fn files_pump(&mut self) {
        self.files.wake = None;
        let now = Instant::now();
        let refill = now.duration_since(self.files.relay_at).as_secs_f64() * RELAY_RATE;
        self.files.relay_tokens = (self.files.relay_tokens + refill).min(RELAY_BURST);
        self.files.relay_at = now;

        let mut ids: Vec<(u64, TransferId)> = self
            .files
            .outs
            .iter()
            .filter(|(_, o)| o.state == TransferState::Active)
            .map(|(id, o)| (o.p.created_ms, *id))
            .collect();
        ids.sort();
        for (_, id) in ids {
            let peer = self.files.outs[&id].peer;
            if !self.peer_up(&peer) {
                continue;
            }
            let lan = self.lan_up(&peer);
            let window = if lan { WINDOW_LAN } else { WINDOW_RELAY };
            let mut inflight: u64 =
                self.files.outs.values().filter(|o| o.peer == peer).flat_map(|o| o.inflight.values()).sum();
            let f = &mut self.files;
            let o = f.outs.get_mut(&id).expect("上面已确认存在");
            while inflight < window {
                let Some(key) = o.todo.pop_front() else { break };
                if o.acked[key.0 as usize][key.1 as usize] || o.inflight.contains_key(&key) {
                    continue;
                }
                let file = &o.files[key.0 as usize];
                let len = chunk_len(file.size, CHUNK, key.1);
                if !lan {
                    if f.relay_tokens < len as f64 {
                        o.todo.push_front(key);
                        let wait = Duration::from_secs_f64((len as f64 - f.relay_tokens) / RELAY_RATE);
                        f.wake = Some(f.wake.map_or(now + wait, |w| w.min(now + wait)));
                        break;
                    }
                    f.relay_tokens -= len as f64;
                }
                let Some(src) = file.src.clone() else { break };
                o.inflight.insert(key, len);
                inflight += len;
                let tx = f.io.clone();
                tokio::task::spawn_blocking(move || {
                    let mut buf = vec![0u8; len as usize];
                    let res = src.read_at(&mut buf, key.1 * u64::from(CHUNK)).map(|()| buf);
                    let _ = tx.send(FileIn::Read { id, key, res });
                });
            }
        }
    }

    pub(super) async fn on_file_msg(&mut self, peer: DeviceId, body: A) {
        match body {
            A::FileOffer(o) => self.on_offer(peer, o).await,
            A::FileAccept(a) => self.on_accept(peer, a).await,
            A::FileChunk(c) => self.on_chunk(peer, c).await,
            A::FileChunkAck(a) => self.on_ack(peer, a).await,
            A::FileCancel(c) => self.on_cancel(peer, c).await,
            A::FileDone(d) => self.on_done(peer, d).await,
            _ => {}
        }
    }

    async fn on_offer(&mut self, peer: DeviceId, mut o: app::FileOffer) {
        let Some(id) = tid(&o.transfer_id) else { return };
        if let Some(i) = self.files.ins.get(&id) {
            if i.peer != peer {
                return;
            }

            let reply = match &i.state {
                TransferState::Active => Some(i.accept_msg(id)),
                TransferState::Done => Some(done_msg(id)),
                TransferState::Failed(_) | TransferState::Cancelled => Some(cancel_msg(id, "对方已取消接收")),
                _ => None,
            };
            if let Some(m) = reply {
                self.send_app(peer, m).await;
            }
            return;
        }
        if let Some((_, reason)) = self.files.rejected.iter().find(|(r, _)| *r == id) {
            let reason = *reason;
            self.send_app(peer, cancel_msg(id, reason)).await;
            return;
        }
        if let Some(text) = o.text.take() {
            if !o.files.is_empty() || text.is_empty() || text.len() > MAX_TEXT {
                return;
            }
            let mut i = new_in(peer, Kind::Files, Vec::new(), Some(text.clone()), CHUNK, TransferState::Done);
            i.p.done = i.p.total;
            i.p.finished_ms = Some(now_ms());
            self.files.ins.insert(id, i);
            info!(peer = %peer.short(), "收到分享的文本");
            self.emit(NodeEvent::SharedText { from: peer, text }).await;
            self.send_app(peer, done_msg(id)).await;
            self.emit_transfer(id, true).await;
            self.prune_transfers();
            return;
        }
        let files = match validate_offer(&o) {
            Ok(f) => f,
            Err(reason) => {
                warn!(peer = %peer.short(), reason, "拒绝不合法的文件传输请求");
                self.reject(peer, id, reason).await;
                return;
            }
        };
        let active = self.files.ins.values().filter(|i| i.peer == peer && !i.state.is_finished()).count();
        if active >= MAX_ACTIVE_IN {
            self.reject(peer, id, "对方同时进行的传输太多，请稍后再发").await;
            return;
        }
        let kind = match o.kind {
            app::FILE_KIND_SCREENSHOT => Kind::Screenshot,
            app::FILE_KIND_CLIP => {
                let Ok(item) = <[u8; 16]>::try_from(o.clip_item.as_slice()) else { return };
                return self.on_clip_offer(peer, id, item, files, o.chunk_size).await;
            }
            _ => Kind::Files,
        };
        if kind == Kind::Screenshot && !self.cfg.options.accept_screenshots {
            self.reject(peer, id, "对方没有开启接收截图").await;
            return;
        }
        info!(peer = %peer.short(), files = files.len(), "收到文件传输请求");
        self.files.ins.insert(id, new_in(peer, kind, files, None, o.chunk_size, TransferState::Asking));
        self.emit_transfer(id, true).await;
        self.prune_transfers();
    }

    pub(super) async fn reject(&mut self, peer: DeviceId, id: TransferId, reason: &'static str) {
        self.files.rejected.push_back((id, reason));
        if self.files.rejected.len() > KEEP_REJECTED {
            self.files.rejected.pop_front();
        }
        self.send_app(peer, cancel_msg(id, reason)).await;
    }

    async fn on_accept(&mut self, peer: DeviceId, a: app::FileAccept) {
        let Some(id) = tid(&a.transfer_id) else { return };
        let Some(o) = self.files.outs.get_mut(&id).filter(|o| o.peer == peer) else {
            self.send_app(peer, cancel_msg(id, "对方已取消发送")).await;
            return;
        };
        if o.state.is_finished() {
            self.send_app(peer, cancel_msg(id, "对方已取消发送")).await;
            return;
        }
        if o.state == TransferState::Preparing || o.text.is_some() {
            return;
        }

        o.inflight.clear();
        o.todo.clear();
        o.p.done = 0;
        for (n, f) in o.files.iter().enumerate() {
            let count = chunk_count(f.size, CHUNK) as usize;
            let have = a.have.get(n).map_or_else(|| vec![false; count], |b| unbitmap(b, count));
            for (c, h) in have.iter().enumerate() {
                if *h {
                    o.p.done += chunk_len(f.size, CHUNK, c as u64);
                } else {
                    o.todo.push_back((n as u32, c as u64));
                }
            }
            o.acked[n] = have;
        }
        o.state = TransferState::Active;
        o.last_progress = Instant::now();
        self.emit_transfer(id, true).await;
        self.files_pump().await;
    }

    async fn on_chunk(&mut self, peer: DeviceId, c: app::FileChunk) {
        let Some(id) = tid(&c.transfer_id) else { return };
        let Some(i) = self.files.ins.get_mut(&id) else { return };
        if i.peer != peer || i.state != TransferState::Active {
            return;
        }
        let Some(f) = i.files.get(c.file_index as usize) else { return };
        if c.chunk_index >= chunk_count(f.size, i.chunk)
            || c.data.len() as u64 != chunk_len(f.size, i.chunk, c.chunk_index)
        {
            warn!(peer = %peer.short(), "收到不合法的文件分块，已忽略");
            return;
        }
        i.last_rx = Instant::now();
        if f.have[c.chunk_index as usize] {
            let ack = AppMessage::new(A::FileChunkAck(app::FileChunkAck {
                transfer_id: id.to_vec(),
                file_index: c.file_index,
                chunk_indices: vec![c.chunk_index],
            }));
            self.send_app(peer, ack).await;
            return;
        }
        let offset = c.chunk_index * u64::from(i.chunk);
        if let Some(w) = &i.writer {
            let _ = w.send(Job::Write { file: c.file_index, chunk: c.chunk_index, offset, data: c.data });
        }
    }

    async fn on_ack(&mut self, peer: DeviceId, a: app::FileChunkAck) {
        let Some(id) = tid(&a.transfer_id) else { return };
        let Some(o) = self.files.outs.get_mut(&id) else { return };
        if o.peer != peer || o.state != TransferState::Active {
            return;
        }
        let n = a.file_index as usize;
        let Some(f) = o.files.get(n) else { return };
        let size = f.size;
        for c in a.chunk_indices {
            let Some(acked) = o.acked[n].get_mut(c as usize) else { continue };
            o.inflight.remove(&(a.file_index, c));
            if !*acked {
                *acked = true;
                o.p.done += chunk_len(size, CHUNK, c);
            }
        }
        o.last_progress = Instant::now();
        self.emit_transfer(id, false).await;
        self.files_pump().await;
    }

    async fn on_cancel(&mut self, peer: DeviceId, c: app::FileCancel) {
        let Some(id) = tid(&c.transfer_id) else { return };
        let reason =
            if c.reason.is_empty() { "对方取消了传输".to_owned() } else { c.reason.chars().take(200).collect() };
        if let Some(o) = self.files.outs.get_mut(&id).filter(|o| o.peer == peer && !o.state.is_finished()) {
            info!(peer = %peer.short(), %reason, "文件发送被对方终止");
            o.finish(TransferState::Failed(reason));
        } else if let Some(i) = self.files.ins.get_mut(&id).filter(|i| i.peer == peer && !i.state.is_finished()) {
            info!(peer = %peer.short(), %reason, "文件接收被对方终止");
            i.finish(TransferState::Failed(reason));
        } else {
            return;
        }
        self.emit_transfer(id, true).await;
    }

    async fn on_done(&mut self, peer: DeviceId, d: app::FileDone) {
        let Some(id) = tid(&d.transfer_id) else { return };
        let Some(o) = self.files.outs.get_mut(&id).filter(|o| o.peer == peer && !o.state.is_finished()) else {
            return;
        };
        o.p.done = o.p.total;
        o.finish(TransferState::Done);
        info!(peer = %peer.short(), "文件已送达");
        self.emit_transfer(id, true).await;
        self.prune_transfers();
    }

    pub(super) async fn on_file_io(&mut self, ev: FileIn) {
        match ev {
            FileIn::Hashed { id, res } => {
                let Some(o) = self.files.outs.get_mut(&id).filter(|o| o.state == TransferState::Preparing) else {
                    return;
                };
                match res {
                    Ok(files) => {
                        o.p.total = files.iter().map(|f| f.size).sum();
                        o.acked = files.iter().map(|f| vec![false; chunk_count(f.size, CHUNK) as usize]).collect();
                        o.files = files;
                        o.state = TransferState::Waiting;
                        o.last_progress = Instant::now();
                        self.offer(id).await;
                    }
                    Err(e) => {
                        warn!(error = %e, "准备发送的文件失败");
                        o.finish(TransferState::Failed(e));
                    }
                }
                self.emit_transfer(id, true).await;
            }
            FileIn::Read { id, key, res } => {
                let Some(o) = self.files.outs.get_mut(&id) else { return };
                if o.state != TransferState::Active || !o.inflight.contains_key(&key) {
                    return;
                }
                let peer = o.peer;
                match res {
                    Ok(data) => {
                        let msg = AppMessage::new(A::FileChunk(app::FileChunk {
                            transfer_id: id.to_vec(),
                            file_index: key.0,
                            chunk_index: key.1,
                            data,
                            hash: Vec::new(),
                        }));
                        self.send_app(peer, msg).await;
                    }
                    Err(e) => {
                        warn!(error = %e, "读取待发送的文件失败");
                        o.finish(TransferState::Failed(format!("读取文件失败：{e}")));
                        self.send_app(peer, cancel_msg(id, "对方读取文件失败")).await;
                        self.emit_transfer(id, true).await;
                    }
                }
            }
            FileIn::Written { id, key, res } => {
                let Some(i) = self.files.ins.get_mut(&id).filter(|i| i.state == TransferState::Active) else {
                    return;
                };
                let peer = i.peer;
                if let Err(e) = res {
                    warn!(error = %e, "写入接收的文件失败");
                    i.finish(TransferState::Failed(format!("写入文件失败：{e}")));
                    self.send_app(peer, cancel_msg(id, "对方保存文件失败")).await;
                    self.emit_transfer(id, true).await;
                    return;
                }
                let chunk = i.chunk;
                let f = &mut i.files[key.0 as usize];
                if !f.have[key.1 as usize] {
                    f.have[key.1 as usize] = true;
                    i.p.done += chunk_len(f.size, chunk, key.1);
                }
                if !f.saving && f.have.iter().all(|h| *h) {
                    f.saving = true;
                    let job = Job::Save { file: key.0, name: f.name.clone(), size: f.size, hash: f.hash };
                    if let Some(w) = &i.writer {
                        let _ = w.send(job);
                    }
                }
                let ack = AppMessage::new(A::FileChunkAck(app::FileChunkAck {
                    transfer_id: id.to_vec(),
                    file_index: key.0,
                    chunk_indices: vec![key.1],
                }));
                self.send_app(peer, ack).await;
                self.emit_transfer(id, false).await;
            }
            FileIn::Saved { id, file, res } => {
                let Some(i) = self.files.ins.get_mut(&id).filter(|i| i.state == TransferState::Active) else {
                    return;
                };
                let peer = i.peer;
                match res {
                    Ok(top) => {
                        i.files[file as usize].saved = true;
                        i.files[file as usize].path = Some(top.clone());
                        if !i.saved.contains(&top) {
                            i.saved.push(top);
                        }
                        if !i.files.iter().all(|f| f.saved) {
                            return;
                        }
                        i.p.done = i.p.total;
                        i.finish(TransferState::Done);
                        info!(peer = %peer.short(), files = i.files.len(), "文件已全部收到");
                        self.send_app(peer, done_msg(id)).await;
                        self.prune_transfers();
                    }
                    Err(e) => {
                        warn!(error = %e, "保存接收的文件失败");
                        i.finish(TransferState::Failed(e));
                        self.send_app(peer, cancel_msg(id, "对方校验或保存文件失败")).await;
                    }
                }
                self.emit_transfer(id, true).await;
            }
        }
    }

    pub(super) async fn files_path_changed(&mut self, peer: DeviceId) {
        let up = self.peer_up(&peer);
        let ids: Vec<TransferId> = self
            .files
            .outs
            .iter()
            .filter(|(_, o)| o.peer == peer && matches!(o.state, TransferState::Waiting | TransferState::Active))
            .map(|(id, _)| *id)
            .collect();
        for id in ids {
            if up {
                self.offer(id).await;
            } else if let Some(o) = self.files.outs.get_mut(&id) {
                o.state = TransferState::Waiting;
                o.inflight.clear();
                o.todo.clear();
            }
            self.emit_transfer(id, true).await;
        }
        let ins: Vec<TransferId> = self
            .files
            .ins
            .iter()
            .filter(|(_, i)| i.peer == peer && !i.state.is_finished())
            .map(|(id, _)| *id)
            .collect();
        for id in ins {
            self.emit_transfer(id, true).await;
        }
    }

    pub(super) async fn files_tick(&mut self) {
        let mut reoffer = Vec::new();
        let mut give_up = Vec::new();
        for (id, o) in &self.files.outs {
            match o.state {
                TransferState::Waiting if o.last_progress.elapsed() > GIVE_UP => give_up.push(*id),
                TransferState::Waiting if o.last_offer.elapsed() > STALL => reoffer.push(*id),

                TransferState::Active if o.last_progress.elapsed() > STALL && o.last_offer.elapsed() > STALL => {
                    reoffer.push(*id);
                }
                _ => {}
            }
        }
        for id in reoffer {
            if let Some(o) = self.files.outs.get(&id)
                && self.peer_up(&o.peer)
            {
                self.offer(id).await;
            }
        }
        for id in give_up {
            if let Some(o) = self.files.outs.get_mut(&id) {
                o.finish(TransferState::Failed("对方一直不在线或没有回应".into()));
                let peer = o.peer;
                self.send_app(peer, cancel_msg(id, "发送方已放弃")).await;
                self.emit_transfer(id, true).await;
            }
        }

        let mut expired = Vec::new();
        for (id, i) in &self.files.ins {
            match i.state {
                TransferState::Asking if i.created.elapsed() > ASK_TTL => expired.push((*id, "对方没有接收（超时）")),
                TransferState::Active if i.last_rx.elapsed() > GIVE_UP => expired.push((*id, "传输中断太久")),
                _ => {}
            }
        }
        for (id, reason) in expired {
            if let Some(i) = self.files.ins.get_mut(&id) {
                i.finish(TransferState::Failed(reason.into()));
                let peer = i.peer;
                self.send_app(peer, cancel_msg(id, reason)).await;
                self.emit_transfer(id, true).await;
            }
        }

        let gone: Vec<TransferId> = self
            .files
            .outs
            .iter()
            .filter(|(_, o)| !o.state.is_finished() && !self.roster.is_member(&o.peer))
            .map(|(id, _)| *id)
            .chain(
                self.files
                    .ins
                    .iter()
                    .filter(|(_, i)| !i.state.is_finished() && !self.roster.is_member(&i.peer))
                    .map(|(id, _)| *id),
            )
            .collect();
        for id in gone {
            if let Some(o) = self.files.outs.get_mut(&id) {
                o.finish(TransferState::Failed("对方已被移出设备组".into()));
            }
            if let Some(i) = self.files.ins.get_mut(&id) {
                i.finish(TransferState::Failed("对方已被移出设备组".into()));
            }
            self.emit_transfer(id, true).await;
        }
        if self.files.outs.values().any(|o| o.state == TransferState::Active) {
            self.files_pump().await;
        }
    }

    pub(super) fn prune_transfers(&mut self) {
        self.files.outs.retain(|_, o| !(o.kind.is_clip() && o.state.is_finished()));
        let mut finished: Vec<(u64, TransferId)> = self
            .files
            .outs
            .iter()
            .filter(|(_, o)| o.state.is_finished())
            .map(|(id, o)| (o.p.finished_ms.unwrap_or(0), *id))
            .chain(
                self.files
                    .ins
                    .iter()
                    .filter(|(_, i)| i.state.is_finished())
                    .map(|(id, i)| (i.p.finished_ms.unwrap_or(0), *id)),
            )
            .collect();
        if finished.len() <= KEEP_FINISHED {
            return;
        }
        finished.sort();
        for (_, id) in &finished[..finished.len() - KEEP_FINISHED] {
            self.files.outs.remove(id);
            self.files.ins.remove(id);
        }
    }
}

pub(super) fn new_in(
    peer: DeviceId,
    kind: Kind,
    files: Vec<InFile>,
    text: Option<String>,
    chunk: u32,
    state: TransferState,
) -> In {
    let total = files.iter().map(|f| f.size).sum::<u64>() + text.as_ref().map_or(0, |t| t.len() as u64);
    In {
        peer,
        kind,
        files,
        text,
        chunk,
        state,
        dir: None,
        writer: None,
        saved: Vec::new(),
        created: Instant::now(),
        last_rx: Instant::now(),
        p: Progress::new(total),
    }
}

fn open_and_hash(files: Vec<OutgoingFile>, cancel: &AtomicBool) -> std::result::Result<Vec<OutFile>, String> {
    files
        .into_iter()
        .map(|f| {
            let file = match f.source {
                FileSource::Path(p) => File::open(&p).map_err(|e| format!("无法打开 {}：{e}", f.name))?,
                FileSource::Open(file) => file,
                FileSource::Memory(data) => {
                    let hash = *blake3::hash(&data).as_bytes();
                    return Ok(OutFile { size: data.len() as u64, hash, src: Some(Src::Mem(Arc::new(data))) });
                }
            };
            let meta = file.metadata().map_err(|e| format!("无法读取 {}：{e}", f.name))?;
            if !meta.is_file() {
                return Err(format!("{} 不是普通文件", f.name));
            }
            let size = meta.len();
            let hash = hash_file(&file, size, Some(cancel)).map_err(|e| format!("读取 {} 失败：{e}", f.name))?;
            Ok(OutFile { size, hash, src: Some(Src::File(Arc::new(file))) })
        })
        .collect()
}

fn hash_file(f: &File, size: u64, cancel: Option<&AtomicBool>) -> io::Result<[u8; 32]> {
    let mut h = blake3::Hasher::new();
    let mut buf = vec![0u8; HASH_BLOCK];
    let mut off = 0u64;
    while off < size {
        if cancel.is_some_and(|c| c.load(Ordering::Relaxed)) {
            return Err(io::Error::other("已取消"));
        }
        let n = usize::try_from(size - off).unwrap_or(HASH_BLOCK).min(HASH_BLOCK);
        read_exact_at(f, &mut buf[..n], off)?;
        h.update(&buf[..n]);
        off += n as u64;
    }
    Ok(*h.finalize().as_bytes())
}

#[cfg(unix)]
fn read_exact_at(f: &File, buf: &mut [u8], off: u64) -> io::Result<()> {
    std::os::unix::fs::FileExt::read_exact_at(f, buf, off)
}

#[cfg(unix)]
fn write_all_at(f: &File, buf: &[u8], off: u64) -> io::Result<()> {
    std::os::unix::fs::FileExt::write_all_at(f, buf, off)
}

#[cfg(windows)]
fn read_exact_at(f: &File, mut buf: &mut [u8], mut off: u64) -> io::Result<()> {
    use std::os::windows::fs::FileExt;
    while !buf.is_empty() {
        match f.seek_read(buf, off)? {
            0 => return Err(io::ErrorKind::UnexpectedEof.into()),
            n => {
                buf = &mut buf[n..];
                off += n as u64;
            }
        }
    }
    Ok(())
}

#[cfg(windows)]
fn write_all_at(f: &File, mut buf: &[u8], mut off: u64) -> io::Result<()> {
    use std::os::windows::fs::FileExt;
    while !buf.is_empty() {
        match f.seek_write(buf, off)? {
            0 => return Err(io::ErrorKind::WriteZero.into()),
            n => {
                buf = &buf[n..];
                off += n as u64;
            }
        }
    }
    Ok(())
}

enum Job {
    Write { file: u32, chunk: u64, offset: u64, data: Vec<u8> },

    Save { file: u32, name: String, size: u64, hash: [u8; 32] },
}

fn spawn_writer(
    id: TransferId,
    dir: PathBuf,
    files: usize,
    io: mpsc::UnboundedSender<FileIn>,
) -> mpsc::UnboundedSender<Job> {
    let (tx, mut rx) = mpsc::unbounded_channel();
    tokio::task::spawn_blocking(move || {
        let mut w = Writer { id, dir, open: HashMap::new(), tops: HashMap::new(), saved: HashSet::new() };
        while let Some(job) = rx.blocking_recv() {
            let ev = match job {
                Job::Write { file, chunk, offset, data } => {
                    let res = w.write(file, offset, &data).map_err(|e| e.to_string());
                    FileIn::Written { id, key: (file, chunk), res }
                }
                Job::Save { file, name, size, hash } => {
                    FileIn::Saved { id, file, res: w.save(file, &name, size, &hash) }
                }
            };
            if io.send(ev).is_err() {
                break;
            }
        }
        w.open.clear();
        for n in 0..files as u32 {
            if !w.saved.contains(&n) {
                let _ = std::fs::remove_file(w.part(n));
            }
        }
    });
    tx
}

struct Writer {
    id: TransferId,
    dir: PathBuf,
    open: HashMap<u32, File>,

    tops: HashMap<String, String>,
    saved: HashSet<u32>,
}

impl Writer {
    fn part(&self, file: u32) -> PathBuf {
        let tag: String = self.id[..6].iter().map(|b| format!("{b:02x}")).collect();
        self.dir.join(format!(".copycat-{tag}-{file}.part"))
    }

    fn handle(&mut self, file: u32) -> io::Result<&File> {
        if !self.open.contains_key(&file) {
            let f = OpenOptions::new().read(true).write(true).create(true).truncate(false).open(self.part(file))?;
            self.open.insert(file, f);
        }
        Ok(&self.open[&file])
    }

    fn write(&mut self, file: u32, offset: u64, data: &[u8]) -> io::Result<()> {
        if self.saved.contains(&file) {
            return Ok(());
        }
        write_all_at(self.handle(file)?, data, offset)
    }

    fn save(&mut self, file: u32, name: &str, size: u64, hash: &[u8; 32]) -> std::result::Result<PathBuf, String> {
        let f = self.handle(file).map_err(|e| format!("无法打开临时文件：{e}"))?;
        let len = f.metadata().map_err(|e| e.to_string())?.len();
        if len != size {
            return Err("文件大小与发送方不一致".into());
        }
        let actual = hash_file(f, size, None).map_err(|e| format!("校验时读取失败：{e}"))?;
        if &actual != hash {
            return Err("文件校验失败（内容与发送方不一致）".into());
        }

        self.open.remove(&file);
        let (dest, top) = self.destination(name);
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("无法创建文件夹：{e}"))?;
        }
        std::fs::rename(self.part(file), &dest).map_err(|e| format!("保存文件失败：{e}"))?;
        self.saved.insert(file);
        Ok(top)
    }

    fn destination(&mut self, name: &str) -> (PathBuf, PathBuf) {
        let parts: Vec<&str> = name.split('/').collect();
        let (last, dirs) = parts.split_last().expect("清洗后的路径非空");
        if dirs.is_empty() {
            let n = unique_name(&self.dir, last, false);
            return (self.dir.join(&n), self.dir.join(&n));
        }
        let dir = &self.dir;
        let top = self.tops.entry(dirs[0].to_owned()).or_insert_with(|| unique_name(dir, dirs[0], true)).clone();
        let mut parent = self.dir.join(&top);
        parent.extend(&dirs[1..]);
        let n = unique_name(&parent, last, false);
        (parent.join(n), self.dir.join(top))
    }
}

fn unique_name(dir: &Path, name: &str, is_dir: bool) -> String {
    if !dir.join(name).exists() {
        return name.to_owned();
    }
    let (stem, ext) = match name.rfind('.') {
        Some(i) if i > 0 && !is_dir => name.split_at(i),
        _ => (name, ""),
    };
    (1..).map(|n| format!("{stem} ({n}){ext}")).find(|c| !dir.join(c).exists()).expect("总能找到未占用的名字")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_rejects_traversal_and_cleans_names() {
        assert_eq!(sanitize("a/b/c.txt").as_deref(), Some("a/b/c.txt"));
        assert_eq!(sanitize(r"a\b.txt").as_deref(), Some("a/b.txt"));
        assert_eq!(sanitize("/abs/./x").as_deref(), Some("abs/x"));
        assert_eq!(sanitize("../x"), None);
        assert_eq!(sanitize("a/../../x"), None);
        assert_eq!(sanitize(""), None);
        assert_eq!(sanitize("///"), None);
        assert_eq!(sanitize("..."), Some("_".into()));
        assert_eq!(sanitize(".. ").as_deref(), Some("_"));
        assert_eq!(sanitize("a:b*c?.txt").as_deref(), Some("a_b_c_.txt"));
        assert_eq!(sanitize("con.txt").as_deref(), Some("_con.txt"));
        assert_eq!(sanitize("Nul").as_deref(), Some("_Nul"));
        assert_eq!(sanitize("console.txt").as_deref(), Some("console.txt"));
        assert_eq!(sanitize("报告 .").as_deref(), Some("报告"));
        assert_eq!(sanitize("x\u{0}y").as_deref(), Some("x_y"));
        assert_eq!(sanitize(&"a/".repeat(17)), None);
        let long = "名".repeat(100);
        assert!(sanitize(&long).unwrap().len() <= MAX_COMPONENT);

        let dotted = format!("{}{}J", "a".repeat(190), ".".repeat(20));
        let clean = sanitize(&dotted).unwrap();
        assert!(clean.len() <= MAX_COMPONENT && !clean.ends_with('.'), "{clean}");

        let reserved = format!("con.{}", "x".repeat(300));
        let clean = sanitize(&reserved).unwrap();
        assert!(clean.len() <= MAX_COMPONENT && clean.starts_with("_con."));
        assert_eq!(sanitize(&clean).as_deref(), Some(clean.as_str()), "清洗应当幂等");
    }

    #[test]
    fn bitmap_roundtrip() {
        let have: Vec<bool> = (0..19).map(|i| i % 3 == 0).collect();
        assert_eq!(unbitmap(&bitmap(&have), have.len()), have);
        assert_eq!(unbitmap(&[], 5), vec![false; 5]);
    }

    #[test]
    fn chunk_math() {
        assert_eq!(chunk_count(0, CHUNK), 0);
        assert_eq!(chunk_count(1, CHUNK), 1);
        assert_eq!(chunk_count(u64::from(CHUNK), CHUNK), 1);
        assert_eq!(chunk_count(u64::from(CHUNK) + 1, CHUNK), 2);
        assert_eq!(chunk_len(u64::from(CHUNK) + 1, CHUNK, 1), 1);
    }

    #[test]
    fn unique_names_do_not_overwrite() {
        let dir = std::env::temp_dir().join(format!("copycat-uniq-{}", random::u64().unwrap()));
        std::fs::create_dir_all(dir.join("相册")).unwrap();
        std::fs::write(dir.join("a.txt"), b"x").unwrap();
        assert_eq!(unique_name(&dir, "a.txt", false), "a (1).txt");
        assert_eq!(unique_name(&dir, "b.txt", false), "b.txt");
        assert_eq!(unique_name(&dir, "相册", true), "相册 (1)");
        assert_eq!(unique_name(&dir, ".bashrc", false), ".bashrc");
        std::fs::remove_file(dir.join("a.txt")).unwrap();
        std::fs::remove_dir(dir.join("相册")).unwrap();
        std::fs::remove_dir(&dir).unwrap();
    }
}
