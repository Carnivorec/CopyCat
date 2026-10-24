use std::time::Duration;

use copycat_app::Service;
use netlink_sys::{Socket, SocketAddr, protocols::NETLINK_ROUTE};
use tracing::{info, warn};

use crate::daemon::AndroidHost;

const RTMGRP_LINK: u32 = 0x1;
const RTMGRP_IPV4_IFADDR: u32 = 0x10;
const RTMGRP_IPV6_IFADDR: u32 = 0x100;

const DEBOUNCE: Duration = Duration::from_millis(1500);

pub fn spawn(svc: Service<AndroidHost>, rt: tokio::runtime::Handle) {
    let r = std::thread::Builder::new().name("netlink".into()).spawn(move || {
        let mut sock = match Socket::new(NETLINK_ROUTE) {
            Ok(s) => s,
            Err(e) => return warn!(error = %e, "无法监听网络变化"),
        };
        if let Err(e) = sock.bind(&SocketAddr::new(0, RTMGRP_LINK | RTMGRP_IPV4_IFADDR | RTMGRP_IPV6_IFADDR)) {
            return warn!(error = %e, "无法订阅网络变化");
        }
        let mut buf = vec![0u8; 16 * 1024];
        loop {
            if let Err(e) = sock.recv(&mut &mut buf[..], 0) {
                warn!(error = %e, "网络变化监听中断");
                return;
            }
            std::thread::sleep(DEBOUNCE);
            while sock.recv(&mut &mut buf[..], libc::MSG_DONTWAIT).is_ok() {}
            info!("网络发生变化，立即重连");
            let svc = svc.clone();
            rt.spawn(async move { svc.network_changed().await });
        }
    });
    if let Err(e) = r {
        warn!(error = %e, "无法创建网络监听线程");
    }
}
