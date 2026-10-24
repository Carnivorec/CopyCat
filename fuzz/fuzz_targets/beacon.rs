#![no_main]

use copycat_transport::discovery::Beacon;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if let Some(b) = Beacon::decode(data) {
        let again = Beacon::decode(&b.encode()).expect("编码后的信标应能解析");
        assert_eq!(again.encode(), b.encode());
    }
});
