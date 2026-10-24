#[cfg(any(target_os = "android", target_os = "linux"))]
mod attach;
pub mod daemon;
pub mod ipc;
mod jni_api;
#[cfg(any(target_os = "android", target_os = "linux"))]
mod netlink;
#[cfg(any(target_os = "android", target_os = "linux"))]
mod resume;

fn init_logging() {
    let _ = tracing_subscriber::fmt()
        .with_ansi(false)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "warn,copycat=info,copycat_core=info,copycat_app=info".into()),
        )
        .try_init();
}

fn harden() {
    #[cfg(any(target_os = "android", target_os = "linux"))]
    unsafe {
        libc::prctl(libc::PR_SET_DUMPABLE, 0, 0, 0, 0);
    }
}
