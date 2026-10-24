use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use copycat_crypto::{DeviceId, DeviceIdentity, random};
use copycat_transport::relay_client::{self, AuthMode, RelayAddr, RelayEvent, RelayHandle};
use copycat_transport::tls::{Tuning, client_endpoint};
use tokio::sync::{Notify, mpsc};
use tracing::{debug, info};

const BACKOFF_MIN: Duration = Duration::from_secs(1);
const BACKOFF_MAX: Duration = Duration::from_secs(60);

pub(crate) enum RelayIn {
    Connected(RelayHandle, Vec<DeviceId>),
    Event(RelayEvent),
    Disconnected(String),
}

pub(crate) struct Supervisor {
    pub relay: RelayAddr,
    pub tuning: Tuning,
    pub identity: Arc<DeviceIdentity>,

    pub roster: Arc<Mutex<Vec<Vec<u8>>>>,

    pub generation: u64,
    pub out: mpsc::Sender<(u64, RelayIn)>,
    pub kick: Arc<Notify>,

    pub stop: Arc<AtomicBool>,
}

fn jitter(d: Duration) -> Duration {
    let ms = d.as_millis() as u64;
    let spread = (ms * 3 / 10).max(1);
    let r = random::u64().unwrap_or(0) % (2 * spread);
    Duration::from_millis(ms.saturating_sub(spread) + r)
}

impl Supervisor {
    async fn send(&self, m: RelayIn) -> bool {
        self.out.send((self.generation, m)).await.is_ok()
    }

    pub async fn run(self) {
        let endpoint = match client_endpoint(self.relay.key, &self.tuning) {
            Ok(ep) => ep,
            Err(e) => {
                let _ = self.send(RelayIn::Disconnected(e.to_string())).await;
                return;
            }
        };
        let mut backoff = BACKOFF_MIN;
        loop {
            if self.stop.load(Ordering::Relaxed) {
                return;
            }
            let roster_ops = self.roster.lock().unwrap_or_else(|p| p.into_inner()).clone();
            let auth = AuthMode::Device { identity: self.identity.clone(), roster_ops };
            match relay_client::connect(&endpoint, &self.relay, &self.tuning, auth).await {
                Ok((handle, mut rx, sess)) => {
                    info!("已连接中转服务器");
                    backoff = BACKOFF_MIN;
                    if self.stop.load(Ordering::Relaxed)
                        || !self.send(RelayIn::Connected(handle.clone(), sess.online)).await
                    {
                        handle.close();
                        return;
                    }
                    let reason = loop {
                        tokio::select! {
                            ev = rx.recv() => match ev {
                                Some(RelayEvent::Closed(r)) => break r,
                                Some(ev) => {
                                    if !self.send(RelayIn::Event(ev)).await {
                                        handle.close();
                                        return;
                                    }
                                }
                                None => break "连接结束".to_owned(),
                            },

                            _ = self.kick.notified() => {
                                handle.close();
                                break "网络变化".to_owned();
                            }
                        }
                    };
                    if !self.send(RelayIn::Disconnected(reason)).await {
                        return;
                    }
                    continue;
                }
                Err(e) => {
                    debug!(error = %e, "连接中转服务器失败");
                    if !self.send(RelayIn::Disconnected(e.to_string())).await {
                        return;
                    }
                }
            }
            if self.stop.load(Ordering::Relaxed) {
                return;
            }
            tokio::select! {
                _ = tokio::time::sleep(jitter(backoff)) => {}
                _ = self.kick.notified() => { backoff = BACKOFF_MIN; continue; }
            }
            if self.out.is_closed() {
                return;
            }
            backoff = (backoff * 2).min(BACKOFF_MAX);
        }
    }
}
