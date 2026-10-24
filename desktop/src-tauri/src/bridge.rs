use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use copycat_app::Res;
use copycat_core::ClipItem;
use tokio::sync::mpsc::UnboundedReceiver;

use crate::host::Service;

#[derive(Clone, Default)]
pub struct Hwnd(Arc<AtomicUsize>);

impl Hwnd {
    pub fn get(&self) -> usize {
        self.0.load(Ordering::Relaxed)
    }
}

#[cfg(windows)]
pub fn start_watcher(hwnd: &Hwnd) -> Result<UnboundedReceiver<()>, Box<dyn std::error::Error>> {
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<()>();
    let watcher = copycat_win::watcher::Watcher::spawn(move || {
        let _ = tx.send(());
    })?;
    hwnd.0.store(watcher.hwnd() as usize, Ordering::Relaxed);

    std::mem::forget(watcher);
    Ok(rx)
}

#[cfg(not(windows))]
pub fn start_watcher(_hwnd: &Hwnd) -> Result<UnboundedReceiver<()>, Box<dyn std::error::Error>> {
    Ok(tokio::sync::mpsc::unbounded_channel().1)
}

#[cfg(windows)]
pub async fn run(svc: Service, mut rx: UnboundedReceiver<()>) {
    use std::time::Duration;

    use copycat_win::clipboard;
    use tracing::{error, info, warn};

    const POLL: Duration = Duration::from_secs(2);

    const MISSES_BEFORE_RESTART: u32 = 3;

    let mut seen = clipboard::sequence();

    let mut pending: Option<u32> = None;
    let mut misses = 0u32;
    let mut poll = tokio::time::interval(POLL);
    poll.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            n = rx.recv() => {
                if n.is_none() {
                    error!("剪贴板监听线程已退出，正在重建");
                    rx = restart_watcher(&svc).await;
                    pending = None;
                    continue;
                }
                misses = 0;
            }
            _ = poll.tick() => {
                let now = clipboard::sequence();
                if now == seen {
                    pending = None;
                    continue;
                }
                if pending != Some(now) {
                    pending = Some(now);
                    continue;
                }
                misses += 1;
                warn!(misses, "剪贴板有变化但没有收到通知，主动读取");
                if misses >= MISSES_BEFORE_RESTART {
                    warn!("剪贴板监听多次漏掉通知，重建监听窗口");
                    rx = restart_watcher(&svc).await;
                    misses = 0;
                }
            }
        }
        pending = None;

        tokio::time::sleep(Duration::from_millis(40)).await;
        while rx.try_recv().is_ok() {}
        seen = clipboard::sequence();
        if svc.is_paused() {
            continue;
        }
        let Some(snap) = read(&svc).await else { continue };
        if snap.ours {
            if let Some(exe) = snap.owner_exe.as_deref().map(exe_name)
                && !exe.eq_ignore_ascii_case(own_exe())
            {
                info!(source = exe, "其他程序写入了已同步过的内容，不再同步");
            }
            continue;
        }
        if snap.sensitive {
            info!(source = snap.owner_exe.as_deref().map(exe_name), "来源程序标记为敏感内容，不同步");
            continue;
        }
        slow_warning(svc.local_copy(snap.reps, snap.owner_exe.as_deref()), "同步引擎迟迟没有处理本机复制").await;
    }
}

#[cfg(windows)]
async fn read(svc: &Service) -> Option<copycat_win::content::Snapshot> {
    use std::time::Duration;

    use copycat_win::clipboard;
    use tracing::{debug, warn};

    for delay in [0u64, 300, 1000, 3000] {
        tokio::time::sleep(Duration::from_millis(delay)).await;
        let hwnd = svc.host().hwnd.get();
        let task = tokio::task::spawn_blocking(move || clipboard::read(hwnd as _));
        match tokio::time::timeout(CLIPBOARD_CALL_LIMIT, task).await {
            Ok(Ok(Ok(s))) => return Some(s),
            Ok(Ok(Err(e))) => debug!(error = %e, "读取剪贴板失败，稍后重试"),
            Ok(Err(e)) => {
                warn!(error = %e, "读取剪贴板的任务异常结束");
                return None;
            }
            Err(_) => {
                warn!("读取剪贴板超时，来源程序可能没有响应，这次复制没有同步");
                return None;
            }
        }
    }
    warn!("剪贴板一直被其他程序占用，这次复制没有同步");
    None
}

#[cfg(windows)]
async fn restart_watcher(svc: &Service) -> UnboundedReceiver<()> {
    use std::time::Duration;

    let mut wait = Duration::from_secs(1);
    loop {
        match start_watcher(&svc.host().hwnd) {
            Ok(rx) => {
                tracing::info!("剪贴板监听已重建");
                return rx;
            }
            Err(e) => tracing::error!(error = %e, "重建剪贴板监听失败，稍后重试"),
        }
        tokio::time::sleep(wait).await;
        wait = (wait * 2).min(Duration::from_secs(60));
    }
}

#[cfg(windows)]
async fn slow_warning<T>(fut: impl Future<Output = T>, what: &'static str) -> T {
    use std::time::Duration;

    tokio::pin!(fut);
    match tokio::time::timeout(Duration::from_secs(10), &mut fut).await {
        Ok(v) => v,
        Err(_) => {
            tracing::warn!("{what}（已等待 10 秒）");
            let started = std::time::Instant::now();
            let v = fut.await;
            tracing::info!(secs = started.elapsed().as_secs() + 10, "{what}：现已完成");
            v
        }
    }
}

#[cfg(windows)]
fn exe_name(p: &str) -> &str {
    p.rsplit(['\\', '/']).next().unwrap_or(p)
}

#[cfg(windows)]
fn own_exe() -> &'static str {
    static NAME: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    NAME.get_or_init(|| {
        std::env::current_exe()
            .ok()
            .and_then(|p| Some(p.file_name()?.to_string_lossy().into_owned()))
            .unwrap_or_default()
    })
}

#[cfg(not(windows))]
pub async fn run(_svc: Service, _rx: UnboundedReceiver<()>) {}

#[cfg(windows)]
const CLIPBOARD_CALL_LIMIT: std::time::Duration = std::time::Duration::from_secs(10);

pub async fn write(hwnd: &Hwnd, item: &ClipItem, remote: bool) -> Res<()> {
    let reps: Vec<(String, Vec<u8>)> =
        item.reps.iter().filter_map(|r| Some((r.mime.clone(), r.inline.clone()?))).collect();
    let marker = if remote { item.id.to_vec() } else { Vec::new() };
    write_raw(hwnd, reps, marker).await
}

#[cfg(not(windows))]
pub async fn copy_received(_hwnd: &Hwnd, _saved: &[String], _dirs: &[bool], _as_image: bool) -> bool {
    false
}

#[cfg(windows)]
pub async fn copy_received(hwnd: &Hwnd, saved: &[String], dirs: &[bool], as_image: bool) -> bool {
    use copycat_win::content::{MIME_FILES, MIME_PNG};

    let image = match (saved, dirs) {
        ([path], [false]) if as_image && is_image_name(path) => {
            let p = path.clone();
            tokio::task::spawn_blocking(move || image_png(std::path::Path::new(&p))).await.ok().flatten()
        }
        _ => None,
    };
    let reps = match image {
        Some(png) => vec![(MIME_PNG.to_owned(), png)],
        None => {
            let paths: Vec<String> = saved
                .iter()
                .map(|p| std::path::absolute(p).map_or_else(|_| p.clone(), |a| a.display().to_string()))
                .collect();
            vec![(MIME_FILES.to_owned(), paths.join("\n").into_bytes())]
        }
    };
    let marker = copycat_crypto::random::bytes::<16>().unwrap_or([1; 16]).to_vec();
    match write_raw(hwnd, reps, marker).await {
        Ok(()) => true,
        Err(e) => {
            tracing::warn!(error = %e, "收到的文件没能复制到剪贴板");
            false
        }
    }
}

#[cfg(windows)]
fn is_image_name(path: &str) -> bool {
    let ext = path.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()).unwrap_or_default();
    matches!(ext.as_str(), "png" | "jpg" | "jpeg" | "gif" | "bmp" | "webp")
}

#[cfg(windows)]
fn image_png(path: &std::path::Path) -> Option<Vec<u8>> {
    const MAX_FILE: u64 = 64 * 1024 * 1024;

    use image::ImageEncoder;

    if std::fs::metadata(path).ok()?.len() > MAX_FILE {
        return None;
    }
    let bytes = std::fs::read(path).ok()?;
    if copycat_win::content::is_png(&bytes) {
        return copycat_win::content::png_to_dib(&bytes).is_some().then_some(bytes);
    }
    let reader = image::ImageReader::new(std::io::Cursor::new(&bytes)).with_guessed_format().ok()?;
    let decoded = (|| {
        let mut dec = reader.into_decoder()?;

        let (w, h) = image::ImageDecoder::dimensions(&dec);
        if u64::from(w) * u64::from(h) > copycat_win::content::MAX_PIXELS {
            return Ok(None);
        }

        let orientation = image::ImageDecoder::orientation(&mut dec)?;
        let mut img = image::DynamicImage::from_decoder(dec)?;
        img.apply_orientation(orientation);
        Ok::<_, image::ImageError>(Some(img))
    })();
    let img = decoded.map_err(|e| tracing::warn!(error = %e, "收到的图片无法解码，按文件复制")).ok()??;
    let mut out = Vec::new();
    image::codecs::png::PngEncoder::new_with_quality(
        &mut out,
        image::codecs::png::CompressionType::Fast,
        image::codecs::png::FilterType::Adaptive,
    )
    .write_image(img.to_rgba8().as_raw(), img.width(), img.height(), image::ExtendedColorType::Rgba8)
    .ok()?;
    Some(out)
}

#[cfg(windows)]
async fn write_raw(hwnd: &Hwnd, reps: Vec<(String, Vec<u8>)>, marker: Vec<u8>) -> Res<()> {
    use std::time::Duration;

    use copycat_win::WinError;
    use tracing::{debug, warn};

    let data = Arc::new((reps, marker));
    for delay in [0u64, 300, 1000, 3000] {
        tokio::time::sleep(Duration::from_millis(delay)).await;
        let d = data.clone();
        let owner = hwnd.get();
        let task = tokio::task::spawn_blocking(move || copycat_win::clipboard::write(owner as _, &d.0, &d.1));
        match tokio::time::timeout(CLIPBOARD_CALL_LIMIT, task).await {
            Ok(Ok(Ok(()))) => return Ok(()),
            Ok(Ok(Err(WinError::Busy))) => debug!("剪贴板被其他程序占用，稍后重试写入"),
            Ok(Ok(Err(e))) => {
                warn!(error = %e, "写入剪贴板失败");
                return Err(e.to_string());
            }
            Ok(Err(e)) => {
                warn!(error = %e, "写入剪贴板的任务异常结束");
                return Err("写入剪贴板时出错".into());
            }
            Err(_) => {
                warn!("写入剪贴板超时，剪贴板的所有者程序可能没有响应");
                return Err("剪贴板长时间没有响应".into());
            }
        }
    }
    warn!("剪贴板一直被其他程序占用，没能写入");
    Err("剪贴板一直被其他程序占用".into())
}

#[cfg(not(windows))]
async fn write_raw(_hwnd: &Hwnd, _reps: Vec<(String, Vec<u8>)>, _marker: Vec<u8>) -> Res<()> {
    Err("仅支持 Windows".into())
}
