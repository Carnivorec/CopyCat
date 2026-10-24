use anyhow::Result;
use copycat_core::{ClipItem, Node};
use tokio::sync::mpsc::UnboundedReceiver;

#[cfg_attr(not(windows), allow(dead_code))]
const MAX_IMAGE: usize = 64 * 1024 * 1024;

#[cfg(windows)]
pub fn start(node: Node, mut apply: UnboundedReceiver<ClipItem>) -> Result<()> {
    use std::time::Duration;

    use copycat_core::Rep;
    use copycat_win::clipboard;
    use copycat_win::content::MIME_PNG;
    use copycat_win::watcher::Watcher;
    use tracing::{debug, warn};

    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<()>();
    let watcher = Watcher::spawn(move || {
        let _ = tx.send(());
    })?;
    let hwnd = watcher.hwnd() as usize;

    std::mem::forget(watcher);

    let n = node.clone();
    tokio::spawn(async move {
        while rx.recv().await.is_some() {
            tokio::time::sleep(Duration::from_millis(40)).await;
            while rx.try_recv().is_ok() {}
            let snap = match tokio::task::spawn_blocking(move || clipboard::read(hwnd as _)).await {
                Ok(Ok(s)) => s,
                Ok(Err(e)) => {
                    debug!(error = %e, "读取剪贴板失败");
                    continue;
                }
                Err(_) => continue,
            };
            if snap.ours || snap.sensitive || snap.reps.is_empty() {
                if snap.sensitive {
                    debug!("敏感内容（密码管理器等），不同步");
                }
                continue;
            }
            let reps: Vec<Rep> = snap
                .reps
                .into_iter()
                .filter(|(mime, data)| mime != MIME_PNG || data.len() <= MAX_IMAGE)
                .map(|(mime, data)| Rep::from_bytes(mime, data))
                .collect();
            let Some(first) = reps.first() else { continue };
            if n.is_echo(first.hash).await.unwrap_or(false) {
                continue;
            }
            if let Err(e) = n.copy(reps).await {
                warn!(error = %e, "同步失败");
            }
        }
    });

    tokio::spawn(async move {
        while let Some(item) = apply.recv().await {
            let reps: Vec<(String, Vec<u8>)> =
                item.reps.iter().filter_map(|r| Some((r.mime.clone(), r.inline.clone()?))).collect();
            let marker = item.id;
            match tokio::task::spawn_blocking(move || clipboard::write(hwnd as _, &reps, &marker)).await {
                Ok(Ok(())) => {}
                Ok(Err(e)) => warn!(error = %e, "写入剪贴板失败"),
                Err(_) => {}
            }
        }
    });
    Ok(())
}

#[cfg(not(windows))]
pub fn start(_node: Node, mut apply: UnboundedReceiver<ClipItem>) -> Result<()> {
    tokio::spawn(async move {
        while let Some(item) = apply.recv().await {
            let text = item.reps.iter().find(|r| r.mime == "text/plain").and_then(|r| r.inline.clone());
            println!("收到：{}", text.map(|t| String::from_utf8_lossy(&t).into_owned()).unwrap_or_default());
        }
    });
    Ok(())
}
