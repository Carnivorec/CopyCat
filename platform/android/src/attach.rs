use std::path::PathBuf;

use inotify::{Inotify, WatchMask};
use tracing::warn;

use crate::daemon::{Action, AndroidHost};

pub const TRIGGER: &str = "attach";

pub fn spawn(svc: copycat_app::Service<AndroidHost>, dir: PathBuf) {
    let r = std::thread::Builder::new().name("attach".into()).spawn(move || {
        let mut ino = match Inotify::init() {
            Ok(i) => i,
            Err(e) => return warn!(error = %e, "无法监听 attach 请求"),
        };
        if let Err(e) = ino.watches().add(&dir, WatchMask::CREATE | WatchMask::ATTRIB | WatchMask::CLOSE_WRITE) {
            return warn!(error = %e, "无法监听数据目录");
        }
        let mut buf = [0u8; 4096];
        loop {
            let events = match ino.read_events_blocking(&mut buf) {
                Ok(ev) => ev,
                Err(e) => return warn!(error = %e, "attach 监听中断"),
            };
            if events.into_iter().any(|e| e.name.is_some_and(|n| n == TRIGGER)) {
                svc.host().send(Action::Attach);
            }
        }
    });
    if let Err(e) = r {
        warn!(error = %e, "无法创建 attach 监听线程");
    }
}
