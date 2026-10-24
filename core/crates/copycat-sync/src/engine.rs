use std::collections::{BTreeMap, BTreeSet, VecDeque};

use copycat_crypto::DeviceId;
use copycat_proto::Message;
use copycat_proto::app::{self, AppMessage, app_message::Body};

use crate::hlc::{Hlc, HlcClock};
use crate::item::{ClipItem, Rep};

const MAX_EXTRA: usize = 1024;

const MAX_ORIGINS: usize = 256;

const SYNC_RETRY_MS: u64 = 10_000;

pub const WIRE_INLINE: usize = 768 * 1024;

const WIRE_BATCH: usize = 1536 * 1024;

#[derive(Clone, Debug)]
pub struct EngineConfig {
    pub retention_per_origin: usize,

    pub retention_ms: u64,

    pub max_batch: usize,

    pub echo_window_ms: u64,
}

impl Default for EngineConfig {
    fn default() -> Self {
        Self { retention_per_origin: 50, retention_ms: 24 * 60 * 60 * 1000, max_batch: 16, echo_window_ms: 2000 }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Mark {
    pub contiguous: u64,
    pub extra: BTreeSet<u64>,
}

impl Mark {
    pub fn has(&self, seq: u64) -> bool {
        seq <= self.contiguous || self.extra.contains(&seq)
    }

    fn insert(&mut self, seq: u64) -> bool {
        if self.has(seq) {
            return false;
        }
        self.extra.insert(seq);
        self.normalize();
        true
    }

    fn advance(&mut self, upto: u64) {
        if upto > self.contiguous {
            self.contiguous = upto;
            self.normalize();
        }
    }

    fn normalize(&mut self) {
        self.extra.retain(|&s| s > self.contiguous);
        while self.extra.remove(&(self.contiguous + 1)) {
            self.contiguous += 1;
        }
    }

    fn to_proto(&self, origin: &DeviceId) -> app::Watermark {
        app::Watermark {
            origin: origin.0.to_vec(),
            contiguous: self.contiguous,
            extra: self.extra.iter().copied().collect(),
        }
    }

    fn from_proto(w: &app::Watermark) -> Option<(DeviceId, Self)> {
        let origin = DeviceId::from_slice(&w.origin).ok()?;
        if w.extra.len() > MAX_EXTRA {
            return None;
        }
        let mut m = Mark { contiguous: w.contiguous, extra: w.extra.iter().copied().collect() };
        m.normalize();
        Some((origin, m))
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Effect {
    Send { to: DeviceId, msg: AppMessage },

    Apply(ClipItem),

    Store(ClipItem),

    Forget(Vec<(DeviceId, u64)>),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Snapshot {
    pub next_seq: u64,
    pub clock: (u64, u32),
    pub marks: BTreeMap<DeviceId, Mark>,
    pub applied: Option<Hlc>,
}

#[derive(Debug, Default)]
struct Peer {
    known: BTreeMap<DeviceId, Mark>,

    ready: bool,

    asked_ms: u64,
}

pub struct Engine {
    me: DeviceId,
    cfg: EngineConfig,
    clock: HlcClock,
    next_seq: u64,
    items: BTreeMap<(DeviceId, u64), ClipItem>,
    marks: BTreeMap<DeviceId, Mark>,
    peers: BTreeMap<DeviceId, Peer>,
    applied: Option<Hlc>,

    applied_id: Option<[u8; 16]>,
    members: Option<BTreeSet<DeviceId>>,
    recent_writes: VecDeque<([u8; 32], u64)>,
}

impl Engine {
    pub fn new(me: DeviceId, cfg: EngineConfig) -> Self {
        Self {
            me,
            cfg,
            clock: HlcClock::new(me),
            next_seq: 1,
            items: BTreeMap::new(),
            marks: BTreeMap::new(),
            peers: BTreeMap::new(),
            applied: None,
            applied_id: None,
            members: None,
            recent_writes: VecDeque::new(),
        }
    }

    pub fn restore(me: DeviceId, cfg: EngineConfig, snap: Snapshot, items: Vec<ClipItem>) -> Self {
        let mut e = Self::new(me, cfg);
        e.next_seq = snap.next_seq.max(1);
        e.clock = HlcClock::restore(me, snap.clock.0, snap.clock.1);
        e.marks = snap.marks;
        e.applied = snap.applied;
        e.applied_id = snap.applied.and_then(|a| items.iter().find(|it| it.hlc == a)).map(|it| it.id);
        for it in items {
            e.marks.entry(it.origin).or_default().insert(it.seq);
            if it.origin == me {
                e.next_seq = e.next_seq.max(it.seq + 1);
            }
            e.items.insert((it.origin, it.seq), it);
        }
        e
    }

    pub fn cap_future(&mut self, now_ms: u64) {
        self.clock.cap(now_ms);
        for it in self.items.values_mut() {
            it.hlc = it.hlc.capped(now_ms);
        }
        if let Some(a) = self.applied {
            let capped = a.capped(now_ms);
            self.applied = Some(capped);

            if (capped.wall_ms, capped.counter) > self.clock.last() {
                self.clock = HlcClock::restore(self.me, capped.wall_ms, capped.counter);
            }
        }
    }

    pub fn snapshot(&self) -> Snapshot {
        Snapshot { next_seq: self.next_seq, clock: self.clock.last(), marks: self.marks.clone(), applied: self.applied }
    }

    pub fn me(&self) -> DeviceId {
        self.me
    }

    pub fn set_retention(&mut self, per_origin: usize, ms: u64) {
        self.cfg.retention_per_origin = per_origin.max(1);
        self.cfg.retention_ms = ms;
    }

    pub fn set_members(&mut self, members: impl IntoIterator<Item = DeviceId>) {
        let mut set: BTreeSet<DeviceId> = members.into_iter().collect();
        set.insert(self.me);
        self.peers.retain(|id, _| set.contains(id));
        self.members = Some(set);
    }

    pub fn batch_for(item: &ClipItem) -> AppMessage {
        AppMessage::new(Body::ClipBatch(app::ClipBatch { items: vec![item.to_proto()], advances: vec![] }))
    }

    pub fn history(&self, now_ms: u64) -> Vec<&ClipItem> {
        let mut v: Vec<&ClipItem> = self.items.values().collect();
        v.sort_by_key(|i| std::cmp::Reverse(i.hlc.capped(now_ms)));
        v
    }

    pub fn applied(&self) -> Option<Hlc> {
        self.applied
    }

    pub fn item(&self, id: &[u8; 16]) -> Option<&ClipItem> {
        self.items.values().find(|it| &it.id == id)
    }

    pub fn is_current(&self, item: &ClipItem) -> bool {
        match self.applied_id {
            Some(id) => id == item.id,
            None => self.applied == Some(item.hlc),
        }
    }

    pub fn is_current_content(&self, fingerprint: &[u8; 32]) -> bool {
        self.items.values().any(|it| self.is_current(it) && it.fingerprint().as_ref() == Some(fingerprint))
    }

    pub fn fill(&mut self, id: &[u8; 16], parts: Vec<(usize, Vec<u8>)>) -> Option<ClipItem> {
        let item = self.items.values_mut().find(|it| &it.id == id)?;
        let mut next = item.clone();
        for (rep, data) in parts {
            if !next.fill(rep, data) {
                return None;
            }
        }
        *item = next.clone();
        Some(next)
    }

    pub fn mark(&self, origin: &DeviceId) -> Option<&Mark> {
        self.marks.get(origin)
    }

    pub fn is_echo(&mut self, fingerprint: &[u8; 32], now_ms: u64) -> bool {
        let window = self.cfg.echo_window_ms;
        self.recent_writes.retain(|(_, t)| now_ms.saturating_sub(*t) <= window);
        if let Some(pos) = self.recent_writes.iter().position(|(f, _)| f == fingerprint) {
            self.recent_writes.remove(pos);
            true
        } else {
            false
        }
    }

    pub fn remember_write(&mut self, fingerprint: [u8; 32], now_ms: u64) {
        self.recent_writes.push_back((fingerprint, now_ms));
    }

    pub fn local_copy(&mut self, reps: Vec<Rep>, now_ms: u64) -> Vec<Effect> {
        let hlc = self.clock.now(now_ms);
        let item = ClipItem { id: *uuid::Uuid::now_v7().as_bytes(), origin: self.me, seq: self.next_seq, hlc, reps };
        self.next_seq += 1;
        self.marks.entry(self.me).or_default().insert(item.seq);
        self.items.insert((self.me, item.seq), item.clone());

        self.applied = Some(self.applied.map_or(hlc, |a| a.capped(now_ms).max(hlc)));
        self.applied_id = Some(item.id);
        let mut fx = vec![Effect::Store(item)];
        fx.extend(self.push_all());
        fx
    }

    pub fn peer_connected(&mut self, peer: DeviceId) -> Vec<Effect> {
        if peer == self.me || !self.allowed(&peer) {
            return Vec::new();
        }
        self.peers.insert(peer, Peer::default());
        vec![Effect::Send { to: peer, msg: self.sync_state(true) }]
    }

    pub fn resync(&self, peer: DeviceId) -> Vec<Effect> {
        if !self.peers.contains_key(&peer) {
            return Vec::new();
        }
        vec![Effect::Send { to: peer, msg: self.sync_state(true) }]
    }

    pub fn tick(&mut self, now_ms: u64) -> Vec<Effect> {
        let mut stalled = Vec::new();
        for (id, p) in &mut self.peers {
            if p.ready {
                continue;
            }
            if p.asked_ms == 0 {
                p.asked_ms = now_ms.max(1);
            } else if now_ms.saturating_sub(p.asked_ms) >= SYNC_RETRY_MS {
                p.asked_ms = now_ms.max(1);
                stalled.push(*id);
            }
        }
        stalled.into_iter().map(|to| Effect::Send { to, msg: self.sync_state(true) }).collect()
    }

    pub fn waiting_peers(&self) -> impl Iterator<Item = &DeviceId> {
        self.peers.iter().filter(|(_, p)| !p.ready).map(|(id, _)| id)
    }

    pub fn peer_disconnected(&mut self, peer: &DeviceId) {
        self.peers.remove(peer);
    }

    pub fn connected_peers(&self) -> impl Iterator<Item = &DeviceId> {
        self.peers.keys()
    }

    pub fn on_message(&mut self, from: DeviceId, msg: AppMessage, now_ms: u64) -> Vec<Effect> {
        if !self.peers.contains_key(&from) {
            return Vec::new();
        }
        match msg.body {
            Some(Body::SyncState(s)) => self.on_sync_state(from, s),
            Some(Body::ClipBatch(b)) => self.on_batch(Some(from), b, now_ms),
            _ => Vec::new(),
        }
    }

    pub fn on_offline_batch(&mut self, batch: app::ClipBatch, now_ms: u64) -> Vec<Effect> {
        self.on_batch(None, batch, now_ms)
    }

    pub fn forget(&mut self, id: &[u8; 16]) -> Vec<Effect> {
        let key = self.items.iter().find(|(_, it)| &it.id == id).map(|(k, _)| *k);
        match key {
            Some(k) => {
                self.items.remove(&k);
                vec![Effect::Forget(vec![k])]
            }
            None => Vec::new(),
        }
    }

    pub fn forget_all(&mut self) -> Vec<Effect> {
        let keys: Vec<(DeviceId, u64)> = self.items.keys().copied().collect();
        self.items.clear();
        if keys.is_empty() { Vec::new() } else { vec![Effect::Forget(keys)] }
    }

    pub fn prune(&mut self, now_ms: u64) -> Vec<Effect> {
        let mut forget = Vec::new();
        let origins: BTreeSet<DeviceId> = self.items.keys().map(|(o, _)| *o).collect();
        for origin in origins {
            let seqs: Vec<u64> = self.items.range((origin, 0)..=(origin, u64::MAX)).map(|(k, _)| k.1).collect();
            let over = seqs.len().saturating_sub(self.cfg.retention_per_origin);
            for (i, seq) in seqs.iter().enumerate() {
                let old = self.items[&(origin, *seq)].hlc.wall_ms.saturating_add(self.cfg.retention_ms) < now_ms;
                if i < over || old {
                    self.items.remove(&(origin, *seq));
                    forget.push((origin, *seq));
                }
            }
        }
        if forget.is_empty() { Vec::new() } else { vec![Effect::Forget(forget)] }
    }

    fn allowed(&self, origin: &DeviceId) -> bool {
        self.members.as_ref().is_none_or(|m| m.contains(origin))
    }

    fn sync_state(&self, reply: bool) -> AppMessage {
        AppMessage::new(Body::SyncState(app::SyncState {
            marks: self.marks.iter().map(|(o, m)| m.to_proto(o)).collect(),
            reply,
        }))
    }

    fn on_sync_state(&mut self, from: DeviceId, s: app::SyncState) -> Vec<Effect> {
        let mut known = BTreeMap::new();
        for w in s.marks.iter().take(MAX_ORIGINS) {
            if let Some((origin, m)) = Mark::from_proto(w) {
                known.insert(origin, m);
            }
        }
        let Some(p) = self.peers.get_mut(&from) else { return Vec::new() };
        p.known = known;
        p.ready = true;
        let mut fx = Vec::new();
        if s.reply {
            fx.push(Effect::Send { to: from, msg: self.sync_state(false) });
        }
        fx.extend(self.push_missing(from));
        fx
    }

    fn on_batch(&mut self, from: Option<DeviceId>, b: app::ClipBatch, now_ms: u64) -> Vec<Effect> {
        let mut fx = Vec::new();
        let mut best: Option<ClipItem> = None;
        for p in b.items {
            let Ok(mut item) = ClipItem::from_proto(p) else { continue };

            item.hlc = item.hlc.capped(now_ms);
            if !self.allowed(&item.origin) {
                continue;
            }
            if let Some(peer) = from.and_then(|f| self.peers.get_mut(&f)) {
                peer.known.entry(item.origin).or_default().insert(item.seq);
            }
            let mark = self.marks.entry(item.origin).or_default();
            let sparse = item.seq > mark.contiguous + 1;
            if mark.has(item.seq) || (sparse && mark.extra.len() >= MAX_EXTRA) {
                continue;
            }
            mark.insert(item.seq);
            self.clock.observe(&item.hlc, now_ms);
            self.items.insert((item.origin, item.seq), item.clone());

            if self.applied.is_none_or(|a| item.hlc > a.capped(now_ms))
                && best.as_ref().is_none_or(|b| item.hlc > b.hlc)
            {
                best = Some(item.clone());
            }
            fx.push(Effect::Store(item));
        }
        for adv in b.advances.iter().take(MAX_ORIGINS) {
            let Ok(origin) = DeviceId::from_slice(&adv.origin) else { continue };
            if !self.allowed(&origin) || origin == self.me {
                continue;
            }
            self.marks.entry(origin).or_default().advance(adv.upto);
            if let Some(peer) = from.and_then(|f| self.peers.get_mut(&f)) {
                peer.known.entry(origin).or_default().advance(adv.upto);
            }
        }
        if let Some(item) = best {
            self.applied = Some(item.hlc);
            self.applied_id = Some(item.id);
            if let Some(fp) = item.fingerprint() {
                self.recent_writes.push_back((fp, now_ms));
            }
            fx.push(Effect::Apply(item));
        }
        if !fx.is_empty() {
            fx.extend(self.push_all());
        }
        fx
    }

    fn push_all(&mut self) -> Vec<Effect> {
        let peers: Vec<DeviceId> = self.peers.keys().copied().collect();
        peers.into_iter().flat_map(|p| self.push_missing(p)).collect()
    }

    fn push_missing(&mut self, to: DeviceId) -> Vec<Effect> {
        let Some(peer) = self.peers.get(&to) else { return Vec::new() };
        if !peer.ready {
            return Vec::new();
        }
        let mut items = Vec::new();
        let mut advances = Vec::new();
        for (origin, mine) in &self.marks {
            let theirs = peer.known.get(origin).cloned().unwrap_or_default();
            let retained: Vec<&ClipItem> =
                self.items.range((*origin, 0)..=(*origin, u64::MAX)).map(|(_, v)| v).collect();

            let floor = retained.first().map_or(mine.contiguous + 1, |it| it.seq);
            let upto = floor.saturating_sub(1).min(mine.contiguous);
            if upto > theirs.contiguous {
                advances.push((*origin, upto));
            }
            for it in retained {
                if !theirs.has(it.seq) {
                    items.push(it.clone());
                }
            }
        }
        if items.is_empty() && advances.is_empty() {
            return Vec::new();
        }

        let peer = self.peers.get_mut(&to).expect("上面已确认存在");
        for (origin, upto) in &advances {
            peer.known.entry(*origin).or_default().advance(*upto);
        }
        for it in &items {
            peer.known.entry(it.origin).or_default().insert(it.seq);
        }

        let mut advances: Vec<app::Advance> =
            advances.into_iter().map(|(o, upto)| app::Advance { origin: o.0.to_vec(), upto }).collect();

        let mut batches: Vec<Vec<app::ClipItem>> = vec![Vec::new()];
        let mut bytes = 0;
        for it in &items {
            let p = it.to_wire(WIRE_INLINE);
            let n = p.encoded_len();
            let cur = batches.last().expect("至少有一批");
            if !cur.is_empty() && (cur.len() >= self.cfg.max_batch.max(1) || bytes + n > WIRE_BATCH) {
                batches.push(Vec::new());
                bytes = 0;
            }
            bytes += n;
            batches.last_mut().expect("至少有一批").push(p);
        }
        batches.into_iter().map(|b| batch(to, b, std::mem::take(&mut advances))).collect()
    }
}

fn batch(to: DeviceId, items: Vec<app::ClipItem>, advances: Vec<app::Advance>) -> Effect {
    Effect::Send { to, msg: AppMessage::new(Body::ClipBatch(app::ClipBatch { items, advances })) }
}

#[cfg(test)]
mod tests;
