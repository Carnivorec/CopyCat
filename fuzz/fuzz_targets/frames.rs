#![no_main]

use copycat_proto::app::AppMessage;
use copycat_proto::frame::{FrameDecoder, MAX_CONTROL_FRAME, decode_message};
use copycat_proto::peer::PeerFrame;
use copycat_proto::relay::{ClientMessage, ServerMessage};
use libfuzzer_sys::fuzz_target;

fn decode_all(b: &[u8]) {
    let _ = decode_message::<PeerFrame>(b);
    let _ = decode_message::<ClientMessage>(b);
    let _ = decode_message::<ServerMessage>(b);
    let _ = decode_message::<AppMessage>(b);
}

fuzz_target!(|data: &[u8]| {
    decode_all(data);
    let split = data.first().map_or(0, |b| usize::from(*b)) % (data.len() + 1);
    let mut d = FrameDecoder::new(MAX_CONTROL_FRAME);
    for part in [&data[..split], &data[split..]] {
        d.push(part);
        loop {
            match d.next_frame() {
                Ok(Some(f)) => {
                    assert!(f.len() <= MAX_CONTROL_FRAME);
                    decode_all(&f);
                }
                Ok(None) => {

                    assert!(d.pending() <= MAX_CONTROL_FRAME + 4);
                    break;
                }
                Err(_) => return,
            }
        }
    }
});
