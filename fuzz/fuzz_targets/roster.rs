#![no_main]

use std::sync::OnceLock;

use copycat_crypto::{DeviceIdentity, DeviceRecord, Platform, Roster};
use libfuzzer_sys::fuzz_target;

fn base() -> &'static (Roster, DeviceIdentity) {
    static K: OnceLock<(Roster, DeviceIdentity)> = OnceLock::new();
    K.get_or_init(|| {
        let id = DeviceIdentity::generate().unwrap();
        let rec = DeviceRecord::issue(&id, "台式机", Platform::Windows, 1).unwrap();
        (Roster::create(&id, &rec, 1).unwrap().0, id)
    })
}

fuzz_target!(|data: &[u8]| {
    let mut ops = Vec::new();
    let mut rest = data;
    while rest.len() >= 2 && ops.len() < 64 {
        let n = (usize::from(rest[0]) << 8 | usize::from(rest[1])).min(rest.len() - 2);
        ops.push(&rest[2..2 + n]);
        rest = &rest[2 + n..];
    }
    let (roster, owner) = base();
    let mut r = roster.clone();
    r.merge(ops.iter().copied());

    assert!(r.is_member(&owner.id()));
    assert_eq!(r.members().count(), 1, "未经签名的操作不应改变成员");
    let _ = Roster::from_ops(roster.group_id(), ops.iter().copied());
});
