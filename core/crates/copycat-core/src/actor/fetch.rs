use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use copycat_crypto::DeviceId;
use copycat_proto::app::{self, AppMessage, app_message::Body as A};
use copycat_sync::ClipItem;
use tokio::sync::oneshot;
use tracing::{debug, info, warn};

use super::Actor;
use super::files::{InFile, cancel_msg};
use crate::{CoreError, NodeEvent, Result, TransferId, TransferState, now_ms};

const ASK_TIMEOUT: Duration = Duration::from_secs(20);

const RETRY_EVERY: Duration = Duration::from_secs(15);

const FETCH_TTL: Duration = Duration::from_secs(10 * 60);

const MAX_FETCHES: usize = 16;

pub(super) struct Fetch {
    reps: Vec<u32>,

    apply: bool,
    tried: HashSet<DeviceId>,

    asking: Option<(DeviceId, Instant)>,
    transfer: Option<TransferId>,
    last_ask: Instant,
    started: Instant,
    waiters: Vec<oneshot::Sender<Result<ClipItem>>>,
}

#[derive(Default)]
pub(super) struct Fetches {
    map: HashMap<[u8; 16], Fetch>,
}

impl Actor {
    fn wanted_reps(&self, item: &ClipItem) -> Vec<u32> {
        let o = &self.cfg.options;
        item.reps
            .iter()
            .enumerate()
            .filter(|(_, r)| r.inline.is_none() && r.size <= o.fetch_max)
            .filter(|(_, r)| o.fetch_images || !r.mime.starts_with("image/"))
            .map(|(i, _)| i as u32)
            .collect()
    }

    pub(super) async fn apply_item(&mut self, item: ClipItem) {
        let want = self.wanted_reps(&item);
        if want.is_empty() {
            if item.reps.iter().any(|r| r.inline.is_some()) {
                self.emit(NodeEvent::Apply(item)).await;
            }
            return;
        }
        debug!(reps = want.len(), "条目内容未内联，先拉取再写入剪贴板");
        self.start_fetch(item.id, want, true, None).await;
    }

    pub(super) async fn fetch_item(&mut self, id: [u8; 16], reply: oneshot::Sender<Result<ClipItem>>) {
        let Some(item) = self.engine.item(&id).cloned() else {
            let _ = reply.send(Err(CoreError::InvalidState("这条记录已不存在")));
            return;
        };
        let want = self.wanted_reps(&item);
        if want.is_empty() {
            let _ = reply.send(Ok(item));
            return;
        }
        self.start_fetch(id, want, false, Some(reply)).await;
    }

    async fn start_fetch(
        &mut self,
        id: [u8; 16],
        reps: Vec<u32>,
        apply: bool,
        waiter: Option<oneshot::Sender<Result<ClipItem>>>,
    ) {
        if !self.fetches.map.contains_key(&id) && self.fetches.map.len() >= MAX_FETCHES {
            let oldest = self
                .fetches
                .map
                .iter()
                .filter(|(_, f)| f.waiters.is_empty() && f.transfer.is_none())
                .min_by_key(|(_, f)| f.started)
                .map(|(k, _)| *k);
            match oldest {
                Some(k) => self.fail_fetch(k, "同时拉取的内容太多").await,
                None => {
                    if let Some(w) = waiter {
                        let _ = w.send(Err(CoreError::InvalidState("同时下载的内容太多，请稍后再试")));
                    }
                    return;
                }
            }
        }
        let f = self.fetches.map.entry(id).or_insert_with(|| Fetch {
            reps,
            apply: false,
            tried: HashSet::new(),
            asking: None,
            transfer: None,
            last_ask: Instant::now(),
            started: Instant::now(),
            waiters: Vec::new(),
        });
        f.apply |= apply;
        f.waiters.extend(waiter);
        if f.asking.is_none() {
            self.fetch_next(id).await;
        }
    }

    async fn fetch_next(&mut self, id: [u8; 16]) {
        let Some(origin) = self.engine.item(&id).map(|i| i.origin) else {
            self.fail_fetch(id, "这条记录已不存在").await;
            return;
        };
        let mut cands = vec![origin];
        cands.extend(self.up_peers());
        let Some(f) = self.fetches.map.get(&id) else { return };
        let next = cands.into_iter().find(|p| *p != self.me && self.peer_up(p) && !f.tried.contains(p));
        let reps = f.reps.clone();
        let f = self.fetches.map.get_mut(&id).expect("上面已确认存在");
        f.asking = next.map(|p| (p, Instant::now()));
        f.transfer = None;
        let Some(peer) = next else { return };
        f.last_ask = Instant::now();
        let msg = AppMessage::new(A::ClipFetch(app::ClipFetch { item_id: id.to_vec(), rep_index: 0, offset: 0, reps }));
        self.send_app(peer, msg).await;
    }

    async fn fail_fetch(&mut self, id: [u8; 16], reason: &'static str) {
        let Some(f) = self.fetches.map.remove(&id) else { return };
        info!(reason, "未能拉取剪贴板内容");
        for w in f.waiters {
            let _ = w.send(Err(CoreError::InvalidState(reason)));
        }

        if f.apply
            && let Some(item) = self.engine.item(&id).cloned()
            && self.engine.is_current(&item)
            && item.reps.iter().any(|r| r.inline.is_some())
        {
            self.emit(NodeEvent::Apply(item)).await;
        }
    }

    async fn complete_fetch(&mut self, id: [u8; 16], parts: Vec<(usize, Vec<u8>)>) {
        let Some(f) = self.fetches.map.remove(&id) else { return };
        let Some(item) = self.engine.fill(&id, parts) else {
            warn!("拉取到的内容与条目不符，已丢弃");
            for w in f.waiters {
                let _ = w.send(Err(CoreError::InvalidState("下载的内容校验失败")));
            }
            return;
        };
        if let Err(e) = self.db().update_item(&item) {
            warn!(error = %e, "保存拉取到的内容失败");
        }
        for w in f.waiters {
            let _ = w.send(Ok(item.clone()));
        }
        self.emit(NodeEvent::HistoryChanged).await;
        if f.apply && self.engine.is_current(&item) {
            if let Some(fp) = item.fingerprint() {
                self.engine.remember_write(fp, now_ms());
            }
            self.emit(NodeEvent::Apply(item)).await;
        }
    }

    pub(super) async fn on_clip_fetch(&mut self, peer: DeviceId, req: app::ClipFetch) {
        let Ok(id) = <[u8; 16]>::try_from(req.item_id.as_slice()) else { return };
        if let Some(t) = self.clip_out(peer, id) {
            self.reoffer(t).await;
            return;
        }
        let mut reps: Vec<u32> = req.reps.clone();
        reps.sort_unstable();
        reps.dedup();
        let files = self.engine.item(&id).and_then(|item| {
            if reps.is_empty() || reps.len() > item.reps.len() {
                return None;
            }
            reps.iter()
                .map(|&r| {
                    let data = item.reps.get(r as usize)?.inline.clone()?;
                    Some(crate::OutgoingFile { name: r.to_string(), source: crate::FileSource::Memory(data) })
                })
                .collect::<Option<Vec<_>>>()
        });
        match files {
            Some(files) => {
                if let Err(e) = self.send_files(peer, files, super::files::Kind::Clip(id)).await {
                    warn!(error = %e, "发送剪贴板内容失败");
                }
            }
            None => {
                let no = app::ClipData { item_id: id.to_vec(), rep_index: 0, offset: 0, data: Vec::new(), last: true };
                self.send_app(peer, AppMessage::new(A::ClipData(no))).await;
            }
        }
    }

    pub(super) async fn on_clip_data(&mut self, peer: DeviceId, d: app::ClipData) {
        let Ok(id) = <[u8; 16]>::try_from(d.item_id.as_slice()) else { return };
        let Some(f) = self.fetches.map.get_mut(&id) else { return };
        if f.asking.is_some_and(|(p, _)| p == peer) && f.transfer.is_none() {
            f.tried.insert(peer);
            self.fetch_next(id).await;
        }
    }

    pub(super) async fn on_clip_offer(
        &mut self,
        peer: DeviceId,
        tid: TransferId,
        item_id: [u8; 16],
        mut files: Vec<InFile>,
        chunk: u32,
    ) {
        let expected = match (self.fetches.map.get(&item_id), self.engine.item(&item_id)) {
            (Some(f), Some(item)) if f.asking.is_some_and(|(p, _)| p == peer) && f.transfer.is_none() => {
                Some((f.reps.clone(), item.clone()))
            }
            _ => None,
        };
        let ok = expected.is_some_and(|(reps, item)| {
            files.len() == reps.len()
                && files.iter().zip(&reps).all(|(file, &r)| {
                    let rep = &item.reps[r as usize];
                    file.name == r.to_string() && file.size == rep.size && file.hash == rep.hash
                })
        });
        if !ok {
            self.reject(peer, tid, "不需要这些内容").await;
            return;
        }
        let tag: String = item_id[..8].iter().map(|b| format!("{b:02x}")).collect();
        for f in &mut files {
            f.name = format!("clip-{tag}-{}", f.name);
        }
        if let Err(e) = self.accept_clip(peer, tid, item_id, files, chunk).await {
            warn!(error = %e, "无法接收剪贴板内容");
            self.send_app(peer, cancel_msg(tid, "对方无法保存")).await;
            if let Some(f) = self.fetches.map.get_mut(&item_id) {
                f.tried.insert(peer);
            }
            self.fetch_next(item_id).await;
            return;
        }
        if let Some(f) = self.fetches.map.get_mut(&item_id) {
            f.transfer = Some(tid);
        }
    }

    pub(super) async fn clip_transfer_finished(&mut self, tid: TransferId) {
        let Some(t) = self.take_in(&tid) else { return };
        let super::files::Kind::Clip(item_id) = t.kind else { return };
        let paths: Vec<Option<PathBuf>> = t.files.iter().map(|f| f.path.clone()).collect();
        let ours = self.fetches.map.get(&item_id).is_some_and(|f| f.transfer == Some(tid));
        if t.state != TransferState::Done || !ours {
            remove_files(paths).await;
            if ours && let Some(f) = self.fetches.map.get_mut(&item_id) {
                f.tried.insert(t.peer);
                self.fetch_next(item_id).await;
            }
            return;
        }
        let reps = self.fetches.map[&item_id].reps.clone();
        let read = tokio::task::spawn_blocking(move || {
            let mut out = Vec::new();
            for (r, p) in reps.into_iter().zip(&paths) {
                let data = p.as_ref().and_then(|p| std::fs::read(p).ok());
                out.push(data.map(|d| (r as usize, d)));
            }
            for p in paths.iter().flatten() {
                let _ = std::fs::remove_file(p);
            }
            out.into_iter().collect::<Option<Vec<_>>>()
        })
        .await
        .ok()
        .flatten();
        match read {
            Some(parts) => self.complete_fetch(item_id, parts).await,
            None => self.fail_fetch(item_id, "读取下载的内容失败").await,
        }
    }

    pub(super) async fn fetch_peer_up(&mut self) {
        let idle: Vec<[u8; 16]> =
            self.fetches.map.iter().filter(|(_, f)| f.asking.is_none()).map(|(k, _)| *k).collect();
        for id in idle {
            self.fetch_next(id).await;
        }
    }

    pub(super) async fn fetch_tick(&mut self) {
        let mut expired = Vec::new();
        let mut stalled = Vec::new();
        let mut retry = Vec::new();
        for (id, f) in &self.fetches.map {
            if f.started.elapsed() > FETCH_TTL {
                expired.push(*id);
            } else if f.transfer.is_none() && f.asking.is_some_and(|(_, t)| t.elapsed() > ASK_TIMEOUT) {
                stalled.push(*id);
            } else if f.asking.is_none() && f.last_ask.elapsed() > RETRY_EVERY {
                retry.push(*id);
            }
        }
        for id in expired {
            self.fail_fetch(id, "没有在线设备能提供这条内容").await;
        }
        for id in stalled {
            if let Some(f) = self.fetches.map.get_mut(&id)
                && let Some((p, _)) = f.asking.take()
            {
                f.tried.insert(p);
            }
            self.fetch_next(id).await;
        }

        for id in retry {
            if let Some(f) = self.fetches.map.get_mut(&id) {
                f.tried.clear();
                f.last_ask = Instant::now();
            }
            self.fetch_next(id).await;
        }
    }
}

async fn remove_files(paths: Vec<Option<PathBuf>>) {
    let _ = tokio::task::spawn_blocking(move || {
        for p in paths.into_iter().flatten() {
            let _ = std::fs::remove_file(p);
        }
    })
    .await;
}
