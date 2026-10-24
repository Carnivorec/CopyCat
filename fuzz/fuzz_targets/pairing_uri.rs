#![no_main]

use copycat_crypto::pairing::PairingInvite;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if let Ok(s) = std::str::from_utf8(data)
        && let Ok(inv) = PairingInvite::parse_uri(s)
    {

        let again = PairingInvite::parse_uri(&inv.to_uri()).expect("重新编码的链接应能解析");
        assert_eq!(again.to_uri(), inv.to_uri());
    }
});
