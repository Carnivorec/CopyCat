#![no_main]

use std::sync::OnceLock;

use copycat_crypto::DeviceIdentity;
use copycat_crypto::session::{Initiator, PathKind, Responder, Session, session_prologue};
use libfuzzer_sys::fuzz_target;

struct Keys {
    a: DeviceIdentity,
    b: DeviceIdentity,
    prologue: Vec<u8>,
}

fn keys() -> &'static Keys {
    static K: OnceLock<Keys> = OnceLock::new();
    K.get_or_init(|| Keys {
        a: DeviceIdentity::generate().unwrap(),
        b: DeviceIdentity::generate().unwrap(),
        prologue: session_prologue(&[5; 32], PathKind::Relay),
    })
}

fn pair(k: &Keys) -> (Session, Session) {
    let (init, m1) = Initiator::start(&k.a, &k.b.dh_public(), &k.prologue).unwrap();
    let (rx, m2) = Responder::read(&k.b, &k.prologue, &m1).unwrap().accept().unwrap();
    (init.finish(&m2).unwrap(), rx)
}

fuzz_target!(|data: &[u8]| {
    let k = keys();

    if let Ok(r) = Responder::read(&k.b, &k.prologue, data) {
        let _ = r.accept();
    }

    let (mut tx, mut rx) = pair(k);
    assert!(rx.open(data).is_err(), "伪造的记录不应通过认证");

    let mut recs = tx.seal(data).unwrap();
    let first = &mut recs[0];
    let i = data.first().map_or(0, |b| usize::from(*b)) % first.len();
    first[i] ^= 0x01;
    let (_, mut rx2) = pair(k);
    assert!(rx2.open(&recs[0]).is_err(), "篡改的记录必须被拒绝");
});
