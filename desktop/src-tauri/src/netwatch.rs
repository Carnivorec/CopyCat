use std::net::Ipv4Addr;
use std::time::{Duration, SystemTime};

use tracing::info;

use crate::host::Service;

const EVERY: Duration = Duration::from_secs(5);

const SLEPT: Duration = Duration::from_secs(30);

pub async fn run(svc: Service) {
    let mut ips = lan_ips();
    let mut last = SystemTime::now();
    let mut tick = tokio::time::interval(EVERY);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tick.tick().await;
        let now = SystemTime::now();
        let gap = now.duration_since(last).unwrap_or_default();
        last = now;
        let cur = lan_ips();
        let woke = gap >= SLEPT;
        let changed = cur != ips;
        ips = cur;
        if woke {
            info!(secs = gap.as_secs(), "电脑从睡眠中醒来，重新连接");
        } else if changed {
            info!(count = ips.len(), "本机局域网地址有变化，重新连接");
        }
        if woke || changed {
            svc.network_changed().await;
        }
    }
}

fn lan_ips() -> Vec<Ipv4Addr> {
    let mut v = copycat_core::local_lan_ips();
    v.sort_unstable();
    v
}
