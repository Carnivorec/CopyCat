#![no_main]

use copycat_crypto::DeviceId;
use copycat_proto::Message;
use copycat_proto::app::{AppMessage, SyncState, app_message::Body};
use copycat_sync::{Effect, Engine, EngineConfig, Rep};
use libfuzzer_sys::fuzz_target;

const MAX_MESSAGE: usize = 2 * 1024 * 1024;

fn check(fx: Vec<Effect>) {
    for e in fx {
        if let Effect::Send { msg, .. } = e {
            assert!(msg.encoded_len() <= MAX_MESSAGE, "引擎发出的消息超过会话层上限");
        }
    }
}

fuzz_target!(|data: &[u8]| {
    let Ok(msg) = AppMessage::decode(data) else { return };
    let (me, peer, other) = (DeviceId([1; 16]), DeviceId([2; 16]), DeviceId([3; 16]));
    let mut e = Engine::new(me, EngineConfig::default());
    e.set_members([me, peer, other]);
    e.local_copy(vec![Rep::from_bytes("text/plain", b"x".to_vec())], 1);
    e.local_copy(vec![Rep::from_bytes("image/png", vec![7; 900 * 1024])], 2);
    e.peer_connected(peer);
    check(e.on_message(peer, AppMessage::new(Body::SyncState(SyncState { marks: vec![], reply: true })), 3));
    check(e.on_message(peer, msg.clone(), 4));
    if let Some(Body::ClipBatch(b)) = msg.body {
        check(e.on_offline_batch(b, 5));
    }
    e.peer_connected(other);
    check(e.on_message(other, AppMessage::new(Body::SyncState(SyncState { marks: vec![], reply: true })), 6));
    check(e.prune(u64::MAX / 2));
    check(e.resync(peer));
    let _ = e.snapshot();
});
