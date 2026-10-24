use std::time::Duration;

use copycat_app::Service;
use tracing::warn;

use crate::daemon::AndroidHost;

const CHECK_EVERY: Duration = Duration::from_secs(2);

const MIN_SLEEP: Duration = Duration::from_secs(30);

fn clock(id: libc::clockid_t) -> Duration {
    let mut ts = libc::timespec { tv_sec: 0, tv_nsec: 0 };

    unsafe { libc::clock_gettime(id, &mut ts) };
    Duration::new(ts.tv_sec as u64, ts.tv_nsec as u32)
}

fn slept_total() -> Duration {
    clock(libc::CLOCK_BOOTTIME).saturating_sub(clock(libc::CLOCK_MONOTONIC))
}

pub fn spawn(svc: Service<AndroidHost>, rt: tokio::runtime::Handle) {
    let r = std::thread::Builder::new().name("resume".into()).spawn(move || {
        let mut last = slept_total();
        loop {
            std::thread::sleep(CHECK_EVERY);
            let now = slept_total();
            let slept = now.saturating_sub(last);
            last = now;
            if slept >= MIN_SLEEP {
                let svc = svc.clone();
                rt.spawn(async move { svc.resumed(slept).await });
            }
        }
    });
    if let Err(e) = r {
        warn!(error = %e, "无法创建唤醒检测线程");
    }
}
