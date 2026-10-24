#![no_main]

use std::sync::OnceLock;

use copycat_crypto::DeviceIdentity;
use copycat_crypto::mailbox::{self, MailboxContext};
use libfuzzer_sys::fuzz_target;

fn ids() -> &'static (DeviceIdentity, DeviceIdentity) {
    static K: OnceLock<(DeviceIdentity, DeviceIdentity)> = OnceLock::new();
    K.get_or_init(|| (DeviceIdentity::generate().unwrap(), DeviceIdentity::generate().unwrap()))
}

fuzz_target!(|data: &[u8]| {
    let (sender, recipient) = ids();
    let ctx = MailboxContext { group_id: &[1; 32], sender: sender.id(), recipient: recipient.id(), msg_id: b"m" };
    assert!(mailbox::open(recipient, &sender.dh_public(), &ctx, data).is_err(), "随机数据不应解密成功");

    if data.len() <= mailbox::MAX_PLAINTEXT {
        let sealed = mailbox::seal(sender, &recipient.dh_public(), &ctx, data).unwrap();
        let cut = data.first().map_or(0, |b| usize::from(*b)) % sealed.len();
        assert!(mailbox::open(recipient, &sender.dh_public(), &ctx, &sealed[..cut]).is_err());
        assert_eq!(mailbox::open(recipient, &sender.dh_public(), &ctx, &sealed).unwrap(), data);
    }
});
