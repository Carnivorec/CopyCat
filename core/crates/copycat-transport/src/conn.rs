use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::watch;

pub type BoxRead = Box<dyn AsyncRead + Send + Unpin>;
pub type BoxWrite = Box<dyn AsyncWrite + Send + Unpin>;

#[derive(Clone)]
pub enum Closer {
    Quic(quinn::Connection),

    Stream(Arc<watch::Sender<bool>>),
}

impl Closer {
    pub fn stream() -> Self {
        Closer::Stream(Arc::new(watch::channel(false).0))
    }

    pub fn close(&self) {
        match self {
            Closer::Quic(c) => c.close(0u32.into(), b"bye"),
            Closer::Stream(tx) => {
                tx.send_replace(true);
            }
        }
    }

    pub fn is_closed(&self) -> bool {
        match self {
            Closer::Quic(c) => c.close_reason().is_some(),
            Closer::Stream(tx) => *tx.borrow(),
        }
    }

    pub async fn closed(&self) {
        match self {
            Closer::Quic(c) => {
                c.closed().await;
            }
            Closer::Stream(tx) => {
                let mut rx = tx.subscribe();
                let _ = rx.wait_for(|v| *v).await;
            }
        }
    }

    pub fn rtt(&self) -> Duration {
        match self {
            Closer::Quic(c) => c.rtt(),
            Closer::Stream(_) => Duration::ZERO,
        }
    }

    pub fn is_quic(&self) -> bool {
        matches!(self, Closer::Quic(_))
    }
}

pub struct RawConn {
    pub read: BoxRead,
    pub write: BoxWrite,

    pub binding: [u8; 32],
    pub closer: Closer,
    pub peer: SocketAddr,
}
