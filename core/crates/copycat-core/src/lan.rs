use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use copycat_crypto::{DeviceId, kdf};
use copycat_proto::peer::PeerFrame;
use copycat_transport::discovery::{Beacon, Discovery};
use copycat_transport::lan::{LanConn, LanEndpoint};
use tokio::sync::mpsc;

const DIAL_TIMEOUT: Duration = Duration::from_millis(1500);

const CONN_QUEUE: usize = 1024;

const WRITE_STALL: Duration = Duration::from_secs(30);

pub(crate) enum LanIn {
    Conn { conn_id: u64, tx: mpsc::Sender<PeerFrame>, dialed: Option<DeviceId> },
    Frame { conn_id: u64, frame: PeerFrame },
    Closed { conn_id: u64 },
    DialFailed { peer: DeviceId },
    Beacon { beacon: Beacon, from: SocketAddr },
}

const BUCKET_SECS: u64 = 600;

pub(crate) fn bucket(now_ms: u64) -> u32 {
    u32::try_from(now_ms / 1000 / BUCKET_SECS).unwrap_or(u32::MAX)
}

pub(crate) fn discovery_key(group: &[u8; 32]) -> [u8; 32] {
    kdf::derive_key(kdf::purpose::DISCOVERY, group)
}

pub(crate) fn beacon_tag(key: &[u8; 32], bucket: u32, id: &DeviceId) -> [u8; 16] {
    let mut msg = Vec::with_capacity(4 + 32);
    msg.extend_from_slice(&bucket.to_be_bytes());
    msg.extend_from_slice(id.as_bytes());
    let mac = kdf::keyed_mac(key, &msg);
    let mut tag = [0u8; 16];
    tag.copy_from_slice(&mac[..16]);
    tag
}

pub(crate) fn spawn_discovery_loop(d: Arc<Discovery>, out: mpsc::Sender<LanIn>) {
    tokio::spawn(async move {
        let mut buf = [0u8; 256];
        loop {
            let r = tokio::select! {
                r = d.recv(&mut buf) => r,

                () = out.closed() => break,
            };
            match r {
                Ok((n, from)) => {
                    let Some(beacon) = Beacon::decode(&buf[..n]) else { continue };
                    if out.send(LanIn::Beacon { beacon, from }).await.is_err() {
                        break;
                    }
                }

                Err(_) if !out.is_closed() => tokio::time::sleep(Duration::from_millis(200)).await,
                Err(_) => break,
            }
        }
    });
}

pub(crate) fn spawn_accept_loop(ep: Arc<LanEndpoint>, ids: Arc<AtomicU64>, out: mpsc::Sender<LanIn>) {
    tokio::spawn(async move {
        while let Some(res) = ep.accept().await {
            let Ok(conn) = res else { continue };
            let id = ids.fetch_add(1, Ordering::Relaxed);
            tokio::spawn(run_conn(id, conn, None, out.clone()));
            if out.is_closed() {
                break;
            }
        }
    });
}

pub(crate) fn spawn_dial(
    ep: Arc<LanEndpoint>,
    peer: DeviceId,
    addrs: Vec<SocketAddr>,
    conn_id: u64,
    out: mpsc::Sender<LanIn>,
) {
    tokio::spawn(async move {
        for addr in addrs {
            if let Ok(conn) = ep.connect(addr, DIAL_TIMEOUT).await {
                run_conn(conn_id, conn, Some(peer), out).await;
                return;
            }
        }
        let _ = out.send(LanIn::DialFailed { peer }).await;
    });
}

async fn run_conn(conn_id: u64, c: LanConn, dialed: Option<DeviceId>, out: mpsc::Sender<LanIn>) {
    let (tx, mut rx) = mpsc::channel::<PeerFrame>(CONN_QUEUE);
    if out.send(LanIn::Conn { conn_id, tx, dialed }).await.is_err() {
        return;
    }
    let LanConn { mut read, mut write, conn } = c;
    let wconn = conn.clone();
    tokio::spawn(async move {
        while let Some(f) = rx.recv().await {
            if !matches!(tokio::time::timeout(WRITE_STALL, write.send(&f)).await, Ok(Ok(()))) {
                break;
            }
        }

        wconn.close(0u32.into(), b"");
    });
    while let Ok(Some(frame)) = read.recv::<PeerFrame>().await {
        if out.send(LanIn::Frame { conn_id, frame }).await.is_err() {
            break;
        }
    }
    conn.close(0u32.into(), b"");
    let _ = out.send(LanIn::Closed { conn_id }).await;
}
