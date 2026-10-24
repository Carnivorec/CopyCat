use std::collections::{BTreeMap, BTreeSet, VecDeque};

use proptest::prelude::*;

use super::*;

fn dev(b: u8) -> DeviceId {
    DeviceId([b; 16])
}

fn text(s: &str) -> Vec<Rep> {
    vec![Rep::from_bytes("text/plain", s.as_bytes().to_vec())]
}

fn text_of(it: &ClipItem) -> String {
    String::from_utf8(it.reps[0].inline.clone().unwrap()).unwrap()
}

struct Net {
    engines: BTreeMap<DeviceId, Engine>,
    links: BTreeMap<(DeviceId, DeviceId), VecDeque<AppMessage>>,
    applied: BTreeMap<DeviceId, Vec<ClipItem>>,
    now: u64,
}

impl Net {
    fn new(ids: &[DeviceId], cfg: EngineConfig) -> Self {
        let mut engines = BTreeMap::new();
        for id in ids {
            let mut e = Engine::new(*id, cfg.clone());
            e.set_members(ids.iter().copied());
            engines.insert(*id, e);
        }
        Self { engines, links: BTreeMap::new(), applied: BTreeMap::new(), now: 1_000 }
    }

    fn is_up(&self, a: DeviceId, b: DeviceId) -> bool {
        self.links.contains_key(&(a, b))
    }

    fn run(&mut self, at: DeviceId, fx: Vec<Effect>) {
        for e in fx {
            match e {
                Effect::Send { to, msg } => {
                    if let Some(q) = self.links.get_mut(&(at, to)) {
                        q.push_back(msg);
                    }
                }
                Effect::Apply(item) => self.applied.entry(at).or_default().push(item),
                Effect::Store(_) | Effect::Forget(_) => {}
            }
        }
    }

    fn connect(&mut self, a: DeviceId, b: DeviceId) {
        if self.is_up(a, b) {
            return;
        }
        self.links.insert((a, b), VecDeque::new());
        self.links.insert((b, a), VecDeque::new());
        let fa = self.engines.get_mut(&a).unwrap().peer_connected(b);
        self.run(a, fa);
        let fb = self.engines.get_mut(&b).unwrap().peer_connected(a);
        self.run(b, fb);
    }

    fn disconnect(&mut self, a: DeviceId, b: DeviceId) {
        self.links.remove(&(a, b));
        self.links.remove(&(b, a));
        self.engines.get_mut(&a).unwrap().peer_disconnected(&b);
        self.engines.get_mut(&b).unwrap().peer_disconnected(&a);
    }

    fn deliver_one(&mut self, from: DeviceId, to: DeviceId) -> bool {
        let Some(msg) = self.links.get_mut(&(from, to)).and_then(VecDeque::pop_front) else { return false };
        self.now += 1;
        let fx = self.engines.get_mut(&to).unwrap().on_message(from, msg, self.now);
        self.run(to, fx);
        true
    }

    fn settle(&mut self) {
        loop {
            let keys: Vec<_> = self.links.keys().copied().collect();
            let mut progressed = false;
            for (f, t) in keys {
                while self.deliver_one(f, t) {
                    progressed = true;
                }
            }
            if !progressed {
                break;
            }
        }
    }

    fn copy(&mut self, at: DeviceId, s: &str) -> ClipItem {
        self.now += 1;
        let fx = self.engines.get_mut(&at).unwrap().local_copy(text(s), self.now);
        let item = fx.iter().find_map(|e| if let Effect::Store(i) = e { Some(i.clone()) } else { None }).unwrap();
        self.run(at, fx);
        item
    }

    fn keys(&self, id: DeviceId) -> BTreeSet<(DeviceId, u64)> {
        self.engines[&id].history(u64::MAX / 2).iter().map(|i| (i.origin, i.seq)).collect()
    }
}

#[test]
fn two_devices_sync_both_ways() {
    let (a, b) = (dev(1), dev(2));
    let mut net = Net::new(&[a, b], EngineConfig::default());
    net.connect(a, b);
    net.settle();
    net.copy(a, "来自 A");
    net.settle();
    assert_eq!(text_of(net.applied[&b].last().unwrap()), "来自 A");
    net.copy(b, "来自 B");
    net.settle();
    assert_eq!(text_of(net.applied[&a].last().unwrap()), "来自 B");
    assert_eq!(net.keys(a), net.keys(b));
}

#[test]
fn one_sided_reset_recovers() {
    let (a, b) = (dev(1), dev(2));
    let mut net = Net::new(&[a, b], EngineConfig::default());
    net.connect(a, b);
    net.settle();
    let ea = net.engines.get_mut(&a).unwrap();
    ea.peer_disconnected(&b);
    let fx = ea.peer_connected(b);
    net.run(a, fx);
    net.settle();
    net.copy(a, "之后");
    net.settle();
    assert_eq!(text_of(net.applied[&b].last().unwrap()), "之后");
}

#[test]
fn lost_sync_request_is_retried() {
    let (a, b) = (dev(1), dev(2));
    let mut net = Net::new(&[a, b], EngineConfig::default());
    net.connect(a, b);
    net.settle();

    let ea = net.engines.get_mut(&a).unwrap();
    ea.peer_disconnected(&b);
    let _lost = ea.peer_connected(b);
    net.copy(a, "之后");
    net.settle();
    assert!(net.applied.get(&b).is_none_or(Vec::is_empty));
    let now = net.now;
    let ea = net.engines.get_mut(&a).unwrap();
    assert_eq!(ea.waiting_peers().count(), 1);
    assert!(ea.tick(now).is_empty());
    assert!(ea.tick(now + 5_000).is_empty());
    let fx = ea.tick(now + 10_000);
    assert_eq!(fx.len(), 1);
    net.run(a, fx);
    net.settle();
    assert_eq!(text_of(net.applied[&b].last().unwrap()), "之后");
    assert_eq!(net.engines[&a].waiting_peers().count(), 0);

    assert!(net.engines.get_mut(&a).unwrap().tick(now + 60_000).is_empty());
}

#[test]
fn offline_items_delivered_after_reconnect() {
    let (a, b) = (dev(1), dev(2));
    let mut net = Net::new(&[a, b], EngineConfig::default());
    net.copy(a, "1");
    net.copy(a, "2");
    net.copy(b, "x");
    net.connect(a, b);
    net.settle();
    assert_eq!(net.keys(a).len(), 3);
    assert_eq!(net.keys(a), net.keys(b));

    assert_eq!(net.engines[&a].applied(), net.engines[&b].applied());
}

#[test]
fn in_flight_messages_lost_on_disconnect_are_resent() {
    let (a, b) = (dev(1), dev(2));
    let mut net = Net::new(&[a, b], EngineConfig::default());
    net.connect(a, b);
    net.settle();
    net.copy(a, "会丢失的消息");

    net.disconnect(a, b);
    assert!(net.keys(b).is_empty());
    net.connect(a, b);
    net.settle();
    assert_eq!(text_of(net.applied[&b].last().unwrap()), "会丢失的消息");
}

#[test]
fn relayed_through_middle_device() {
    let (a, b, c) = (dev(1), dev(2), dev(3));
    let mut net = Net::new(&[a, b, c], EngineConfig::default());
    net.connect(a, b);
    net.connect(b, c);
    net.settle();
    net.copy(a, "经 B 转发");
    net.settle();
    assert_eq!(text_of(net.applied[&c].last().unwrap()), "经 B 转发");
}

#[test]
fn echo_is_suppressed_once() {
    let (a, b) = (dev(1), dev(2));
    let mut net = Net::new(&[a, b], EngineConfig::default());
    net.connect(a, b);
    net.settle();
    let item = net.copy(a, "hello");
    net.settle();
    let fp = item.fingerprint().unwrap();
    let eb = net.engines.get_mut(&b).unwrap();
    assert!(eb.is_echo(&fp, net.now), "刚写入的远端内容应被识别为回声");
    assert!(!eb.is_echo(&fp, net.now), "回声只抑制一次，之后主人再复制同一内容应正常同步");
}

#[test]
fn echo_window_expires() {
    let (a, b) = (dev(1), dev(2));
    let mut net = Net::new(&[a, b], EngineConfig::default());
    net.connect(a, b);
    net.settle();
    let item = net.copy(a, "hello");
    net.settle();
    let later = net.now + 10_000;
    assert!(!net.engines.get_mut(&b).unwrap().is_echo(&item.fingerprint().unwrap(), later));
}

#[test]
fn current_content_matches_latest_item_only() {
    let (a, b) = (dev(1), dev(2));
    let mut net = Net::new(&[a, b], EngineConfig::default());
    net.connect(a, b);
    net.settle();
    let first = net.copy(a, "hello").fingerprint().unwrap();
    net.settle();
    let eb = net.engines.get_mut(&b).unwrap();

    assert!(eb.is_echo(&first, net.now));
    assert!(eb.is_current_content(&first));
    let second = net.copy(b, "world").fingerprint().unwrap();
    let eb = &net.engines[&b];
    assert!(!eb.is_current_content(&first), "已有更新的内容，旧内容不再算当前内容");
    assert!(eb.is_current_content(&second));
}

#[test]
fn older_item_does_not_overwrite_clipboard() {
    let (a, b) = (dev(1), dev(2));
    let mut net = Net::new(&[a, b], EngineConfig::default());
    net.copy(a, "旧的");
    net.now += 100;
    net.copy(b, "新的");
    net.connect(a, b);
    net.settle();

    assert!(net.applied.get(&b).is_none_or(|v| v.is_empty()));
    assert_eq!(text_of(net.applied[&a].last().unwrap()), "新的");
}

#[test]
fn non_member_origin_is_dropped() {
    let (a, b, x) = (dev(1), dev(2), dev(9));
    let mut eb = Engine::new(b, EngineConfig::default());
    eb.set_members([a, b]);
    eb.peer_connected(a);
    let forged =
        ClipItem { id: [0; 16], origin: x, seq: 1, hlc: Hlc { wall_ms: 5, counter: 0, node: x }, reps: text("forged") };
    let msg = AppMessage::new(Body::ClipBatch(app::ClipBatch { items: vec![forged.to_proto()], advances: vec![] }));
    assert!(eb.on_message(a, msg, 10).is_empty());
    assert!(eb.history(0).is_empty());
}

#[test]
fn replayed_offline_batch_is_ignored() {
    let (a, b) = (dev(1), dev(2));
    let mut ea = Engine::new(a, EngineConfig::default());
    let fx = ea.local_copy(text("一次"), 10);
    let Effect::Store(item) = &fx[0] else { panic!() };
    let batch = app::ClipBatch { items: vec![item.to_proto()], advances: vec![] };
    let mut eb = Engine::new(b, EngineConfig::default());
    eb.set_members([a, b]);
    let first = eb.on_offline_batch(batch.clone(), 20);
    assert!(first.iter().any(|e| matches!(e, Effect::Apply(_))));
    assert!(eb.on_offline_batch(batch, 30).is_empty(), "重放的批次必须没有任何副作用");

    let fx = ea.local_copy(text("两次"), 40);
    let Effect::Store(newer) = &fx[0] else { panic!() };
    let applied = eb.on_offline_batch(app::ClipBatch { items: vec![newer.to_proto()], advances: vec![] }, 50);
    assert!(applied.iter().any(|e| matches!(e, Effect::Apply(i) if text_of(i) == "两次")));
}

#[test]
fn messages_from_unconnected_peer_ignored() {
    let (a, b) = (dev(1), dev(2));
    let mut eb = Engine::new(b, EngineConfig::default());
    let msg = AppMessage::new(Body::SyncState(app::SyncState { marks: vec![], reply: false }));
    assert!(eb.on_message(a, msg, 1).is_empty());
}

#[test]
fn pruned_history_is_skipped_with_advance() {
    let (a, b) = (dev(1), dev(2));
    let cfg = EngineConfig { retention_per_origin: 5, ..EngineConfig::default() };
    let mut net = Net::new(&[a, b], cfg);
    for i in 0..20 {
        net.copy(a, &i.to_string());
    }
    let now = net.now;
    let fx = net.engines.get_mut(&a).unwrap().prune(now);
    assert!(matches!(&fx[..], [Effect::Forget(v)] if v.len() == 15));
    net.connect(a, b);
    net.settle();
    assert_eq!(net.keys(b).len(), 5);
    assert_eq!(net.engines[&b].mark(&a).unwrap().contiguous, 20, "过期序号应通过 Advance 跳过");
    assert_eq!(text_of(net.applied[&b].last().unwrap()), "19");
}

#[test]
fn forgotten_item_is_not_resynced() {
    let (a, b) = (dev(1), dev(2));
    let mut net = Net::new(&[a, b], EngineConfig::default());
    net.connect(a, b);
    net.settle();
    let it = net.copy(a, "要删掉的");
    net.settle();
    let fx = net.engines.get_mut(&b).unwrap().forget(&it.id);
    assert!(matches!(&fx[..], [Effect::Forget(v)] if v.len() == 1));
    assert!(net.keys(b).is_empty());

    net.disconnect(a, b);
    net.connect(a, b);
    net.settle();
    assert!(net.keys(b).is_empty());
    assert_eq!(net.keys(a).len(), 1);
    assert!(net.engines.get_mut(&a).unwrap().forget(&[9; 16]).is_empty());
    assert_eq!(net.engines.get_mut(&a).unwrap().forget_all().len(), 1);
    assert!(net.keys(a).is_empty());
}

#[test]
fn restore_continues_sequence() {
    let a = dev(1);
    let mut e = Engine::new(a, EngineConfig::default());
    e.local_copy(text("1"), 10);
    e.local_copy(text("2"), 11);
    let snap = e.snapshot();
    let items: Vec<ClipItem> = e.history(0).into_iter().cloned().collect();
    let mut r = Engine::restore(a, EngineConfig::default(), snap, items);
    let fx = r.local_copy(text("3"), 5);
    let Effect::Store(it) = &fx[0] else { panic!() };
    assert_eq!(it.seq, 3);
    assert!(Some(it.hlc) > e.applied());
}

#[test]
fn sparse_sequence_flood_is_bounded() {
    let (a, b) = (dev(1), dev(2));
    let mut eb = Engine::new(b, EngineConfig::default());
    eb.peer_connected(a);
    let items: Vec<app::ClipItem> = (0..(MAX_EXTRA as u64 + 50))
        .map(|i| {
            ClipItem {
                id: [0; 16],
                origin: a,
                seq: 1000 + i * 2,
                hlc: Hlc { wall_ms: 1, counter: 0, node: a },
                reps: text("x"),
            }
            .to_proto()
        })
        .collect();
    eb.on_message(a, AppMessage::new(Body::ClipBatch(app::ClipBatch { items, advances: vec![] })), 1);
    assert!(eb.mark(&a).unwrap().extra.len() <= MAX_EXTRA);
}

#[test]
fn large_backlog_is_split_and_large_reps_are_not_inlined() {
    let (a, b) = (dev(1), dev(2));
    let mut ea = Engine::new(a, EngineConfig::default());
    ea.set_members([a, b]);

    for i in 0..6u8 {
        ea.local_copy(vec![Rep::from_bytes("image/png", vec![i; 600 * 1024])], 10 + u64::from(i));
    }
    let big = vec![9u8; 3 * 1024 * 1024];
    ea.local_copy(
        vec![Rep::from_bytes("text/plain", b"caption".to_vec()), Rep::from_bytes("image/png", big.clone())],
        20,
    );
    ea.peer_connected(b);
    let fx = ea.on_message(b, AppMessage::new(Body::SyncState(app::SyncState { marks: vec![], reply: false })), 30);
    let batches: Vec<app::ClipBatch> = fx
        .into_iter()
        .filter_map(|e| match e {
            Effect::Send { msg: AppMessage { body: Some(Body::ClipBatch(b)) }, .. } => Some(b),
            _ => None,
        })
        .collect();
    assert!(batches.len() >= 3, "必须拆成多批");
    for b in &batches {
        assert!(AppMessage::new(Body::ClipBatch(b.clone())).encoded_len() <= WIRE_BATCH + 64 * 1024);
    }
    let all: Vec<&app::ClipItem> = batches.iter().flat_map(|b| &b.items).collect();
    assert_eq!(all.len(), 7);
    let last = all.iter().find(|i| i.reps.len() == 2).unwrap();
    assert!(last.reps[0].inline.is_some() && last.reps[1].inline.is_none());

    let mut eb = Engine::new(b, EngineConfig::default());
    eb.set_members([a, b]);
    eb.peer_connected(a);
    let mut applied = None;
    for bt in batches {
        for e in eb.on_message(a, AppMessage::new(Body::ClipBatch(bt)), 40) {
            if let Effect::Apply(it) = e {
                applied = Some(it);
            }
        }
    }
    let applied = applied.unwrap();
    assert!(!applied.is_complete() && eb.is_current(&applied));
    assert!(eb.fill(&applied.id, vec![(1, vec![0u8; 3])]).is_none());
    let full = eb.fill(&applied.id, vec![(1, big)]).unwrap();
    assert!(full.is_complete());
    assert_eq!(eb.item(&applied.id), Some(&full));
    assert_eq!(Some(&full), ea.item(&applied.id));
}

#[derive(Clone, Debug)]
enum Op {
    Copy(usize),
    Toggle(usize, usize),
    Deliver(usize, usize),
}

fn op() -> impl Strategy<Value = Op> {
    prop_oneof![
        (0usize..3).prop_map(Op::Copy),
        (0usize..3, 0usize..3).prop_map(|(a, b)| Op::Toggle(a, b)),
        (0usize..3, 0usize..3).prop_map(|(a, b)| Op::Deliver(a, b)),
        (0usize..3, 0usize..3).prop_map(|(a, b)| Op::Deliver(a, b)),
    ]
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    #[test]
    fn converges_under_churn(ops in prop::collection::vec(op(), 1..120)) {
        let ids = [dev(1), dev(2), dev(3)];
        let mut net = Net::new(&ids, EngineConfig::default());
        let mut n = 0;
        for o in ops {
            match o {
                Op::Copy(i) => { n += 1; net.copy(ids[i], &format!("c{n}")); }
                Op::Toggle(a, b) if a != b => {
                    if net.is_up(ids[a], ids[b]) { net.disconnect(ids[a], ids[b]) } else { net.connect(ids[a], ids[b]) }
                }
                Op::Deliver(a, b) if a != b => { net.deliver_one(ids[a], ids[b]); }
                _ => {}
            }
        }
        for i in 0..3 {
            for j in (i + 1)..3 {
                net.connect(ids[i], ids[j]);
            }
        }
        net.settle();

        let k0 = net.keys(ids[0]);
        prop_assert_eq!(k0.len(), n);
        for id in &ids[1..] {
            prop_assert_eq!(&net.keys(*id), &k0);
        }
        let max_hlc = net.engines[&ids[0]].history(u64::MAX / 2).first().map(|i| i.hlc);
        for id in &ids {
            prop_assert_eq!(net.engines[id].applied(), max_hlc);

            if let Some(v) = net.applied.get(id) {
                prop_assert!(v.windows(2).all(|w| w[0].hlc < w[1].hlc));
            }
        }
    }
}

const HOUR: u64 = 60 * 60 * 1000;

fn remote_item(origin: DeviceId, seq: u64, wall_ms: u64, s: &str) -> ClipItem {
    let mut id = [0; 16];
    id[0] = origin.0[0];
    id[1] = seq as u8;
    ClipItem { id, origin, seq, hlc: Hlc { wall_ms, counter: 0, node: origin }, reps: text(s) }
}

fn offline(items: &[ClipItem]) -> app::ClipBatch {
    app::ClipBatch { items: items.iter().map(ClipItem::to_proto).collect(), advances: vec![] }
}

fn applies(fx: &[Effect]) -> bool {
    fx.iter().any(|e| matches!(e, Effect::Apply(_)))
}

#[test]
fn future_timestamp_does_not_suppress_later_items() {
    let (a, b, c) = (dev(1), dev(2), dev(3));
    let now = 1_000_000_000;
    let mut eb = Engine::new(b, EngineConfig::default());
    eb.set_members([a, b, c]);
    let poison = remote_item(a, 1, now + 24 * HOUR, "未来");
    assert!(applies(&eb.on_offline_batch(offline(std::slice::from_ref(&poison)), now)));
    assert!(eb.applied().unwrap().wall_ms <= now + HOUR, "保存的比较键必须截断");

    let later = now + HOUR + 1000;
    let normal = remote_item(c, 1, later, "正常");
    let fx = eb.on_offline_batch(offline(std::slice::from_ref(&normal)), later);
    assert!(applies(&fx), "正常时间的新内容被未来时间戳压制");
    assert!(eb.is_current(&normal) && !eb.is_current(&poison));

    assert_eq!(eb.history(later)[0].id, normal.id);

    let fx = eb.local_copy(text("本机"), later + 10);
    let Effect::Store(mine) = &fx[0] else { panic!() };
    assert!(eb.is_current(mine));
}

#[test]
fn poisoned_snapshot_is_repaired_on_restore() {
    let (a, b, c) = (dev(1), dev(2), dev(3));
    let now = 1_000_000_000;
    let poison = remote_item(a, 1, now + 24 * HOUR, "未来");
    let snap = Snapshot {
        next_seq: 1,
        clock: (now + 24 * HOUR, 0),
        marks: [(a, Mark { contiguous: 1, extra: BTreeSet::new() })].into_iter().collect(),
        applied: Some(poison.hlc),
    };
    let mut eb = Engine::restore(b, EngineConfig::default(), snap, vec![poison.clone()]);
    eb.set_members([a, b, c]);
    assert!(eb.is_current(&poison), "恢复后仍能认出已应用的条目");
    eb.cap_future(now);
    assert!(eb.is_current(&poison));
    assert!(eb.snapshot().clock.0 <= now + HOUR);

    let later = now + HOUR + 1000;
    let normal = remote_item(c, 1, later, "正常");
    assert!(applies(&eb.on_offline_batch(offline(&[normal]), later)));

    let fx = eb.local_copy(text("本机"), later);
    let Effect::Store(mine) = &fx[0] else { panic!() };
    assert!(mine.hlc.wall_ms <= later + HOUR);
    assert!(eb.is_current(mine));
}

#[test]
fn local_clock_set_back_recovers() {
    let (a, b) = (dev(1), dev(2));
    let now = 1_000_000_000;
    let mut ea = Engine::new(a, EngineConfig::default());
    ea.set_members([a, b]);
    ea.local_copy(text("时间错了"), now + 24 * HOUR);
    let fx = ea.local_copy(text("时间改回"), now);
    let Effect::Store(fixed) = &fx[0] else { panic!() };
    assert!(fixed.hlc.wall_ms <= now + HOUR);
    let later = now + 2 * HOUR;
    let fx = ea.on_offline_batch(offline(&[remote_item(b, 1, later, "对方")]), later);
    assert!(applies(&fx), "本机时钟改回后仍收不到其他设备的新内容");
}
