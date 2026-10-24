use copycat_proto::frame::{self, FrameDecoder};
use prost::Message;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use crate::{Result, TransportError};

pub struct FramedWrite<W> {
    inner: W,
    max: usize,
}

impl<W: AsyncWrite + Unpin> FramedWrite<W> {
    pub fn new(inner: W, max: usize) -> Self {
        Self { inner, max }
    }

    pub async fn send<M: Message>(&mut self, msg: &M) -> Result<()> {
        let f = frame::encode_message(msg, self.max)?;
        self.inner.write_all(&f).await?;
        Ok(())
    }

    pub async fn shutdown(&mut self) -> Result<()> {
        self.inner.shutdown().await?;
        Ok(())
    }
}

pub struct FramedRead<R> {
    inner: R,
    dec: FrameDecoder,
    buf: Box<[u8]>,
}

impl<R: AsyncRead + Unpin> FramedRead<R> {
    pub fn new(inner: R, max: usize) -> Self {
        Self { inner, dec: FrameDecoder::new(max), buf: vec![0u8; 16 * 1024].into_boxed_slice() }
    }

    pub async fn recv<M: Message + Default>(&mut self) -> Result<Option<M>> {
        loop {
            if let Some(f) = self.dec.next_frame()? {
                return M::decode(f).map(Some).map_err(|e| TransportError::Protocol(e.to_string()));
            }
            let n = self.inner.read(&mut self.buf).await?;
            if n == 0 {
                return if self.dec.pending() == 0 { Ok(None) } else { Err(TransportError::Closed) };
            }
            self.dec.push(&self.buf[..n]);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use copycat_proto::relay::Ping;

    #[tokio::test]
    async fn roundtrip_over_duplex() {
        let (a, b) = tokio::io::duplex(64);
        let mut w = FramedWrite::new(a, 1024);
        let mut r = FramedRead::new(b, 1024);
        let writer = tokio::spawn(async move {
            for i in 0..100u64 {
                w.send(&Ping { nonce: i }).await.unwrap();
            }
        });
        for i in 0..100u64 {
            let p: Ping = r.recv().await.unwrap().unwrap();
            assert_eq!(p.nonce, i);
        }
        writer.await.unwrap();
        assert!(r.recv::<Ping>().await.unwrap().is_none());
    }
}
