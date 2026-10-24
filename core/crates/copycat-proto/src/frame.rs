use bytes::{Buf, BufMut, Bytes, BytesMut};
use thiserror::Error;

pub const MAX_CONTROL_FRAME: usize = 64 * 1024;

pub const MAX_DATA_FRAME: usize = 1024 * 1024 + 4096;

const HEADER_LEN: usize = 4;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum FrameError {
    #[error("帧长度 {len} 超过上限 {max}")]
    TooLarge { len: usize, max: usize },
    #[error("消息解码失败: {0}")]
    Decode(String),
}

pub fn encode(payload: &[u8], max: usize) -> Result<Bytes, FrameError> {
    if payload.len() > max {
        return Err(FrameError::TooLarge { len: payload.len(), max });
    }
    let mut out = BytesMut::with_capacity(HEADER_LEN + payload.len());
    out.put_u32(payload.len() as u32);
    out.put_slice(payload);
    Ok(out.freeze())
}

pub fn encode_message<M: prost::Message>(msg: &M, max: usize) -> Result<Bytes, FrameError> {
    let len = msg.encoded_len();
    if len > max {
        return Err(FrameError::TooLarge { len, max });
    }
    let mut out = BytesMut::with_capacity(HEADER_LEN + len);
    out.put_u32(len as u32);
    msg.encode(&mut out).expect("BytesMut 容量已预留，编码不会失败");
    Ok(out.freeze())
}

pub fn decode_message<M: prost::Message + Default>(payload: &[u8]) -> Result<M, FrameError> {
    M::decode(payload).map_err(|e| FrameError::Decode(e.to_string()))
}

#[derive(Debug)]
pub struct FrameDecoder {
    buf: BytesMut,
    max: usize,
}

impl FrameDecoder {
    pub fn new(max: usize) -> Self {
        Self { buf: BytesMut::new(), max }
    }

    pub fn push(&mut self, data: &[u8]) {
        self.buf.extend_from_slice(data);
    }

    pub fn next_frame(&mut self) -> Result<Option<Bytes>, FrameError> {
        if self.buf.len() < HEADER_LEN {
            return Ok(None);
        }
        let len = u32::from_be_bytes(self.buf[..HEADER_LEN].try_into().unwrap()) as usize;
        if len > self.max {
            return Err(FrameError::TooLarge { len, max: self.max });
        }
        if self.buf.len() < HEADER_LEN + len {
            self.buf.reserve(HEADER_LEN + len - self.buf.len());
            return Ok(None);
        }
        self.buf.advance(HEADER_LEN);
        Ok(Some(self.buf.split_to(len).freeze()))
    }

    pub fn pending(&self) -> usize {
        self.buf.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_split_arbitrarily() {
        let a = encode(b"hello", 1024).unwrap();
        let b = encode(b"", 1024).unwrap();
        let c = encode(&[7u8; 300], 1024).unwrap();
        let stream: Vec<u8> = [a, b, c].concat();

        for chunk in 1..stream.len() {
            let mut dec = FrameDecoder::new(1024);
            let mut out = Vec::new();
            for piece in stream.chunks(chunk) {
                dec.push(piece);
                while let Some(f) = dec.next_frame().unwrap() {
                    out.push(f);
                }
            }
            assert_eq!(out.len(), 3);
            assert_eq!(&out[0][..], b"hello");
            assert!(out[1].is_empty());
            assert_eq!(&out[2][..], &[7u8; 300][..]);
            assert_eq!(dec.pending(), 0);
        }
    }

    #[test]
    fn rejects_oversized_header_before_buffering() {
        let mut dec = FrameDecoder::new(16);
        dec.push(&(17u32).to_be_bytes());
        assert_eq!(dec.next_frame(), Err(FrameError::TooLarge { len: 17, max: 16 }));
    }

    #[test]
    fn encode_rejects_oversized() {
        assert!(encode(&[0u8; 17], 16).is_err());
    }
}
