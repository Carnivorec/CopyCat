use copycat_app::SharedText;
use copycat_app::TransferDto;
use tauri::AppHandle;

fn what(t: &TransferDto) -> String {
    match (t.count, t.files.first()) {
        (1, Some(f)) => f.name.rsplit('/').next().unwrap_or(&f.name).to_owned(),
        (n, _) => format!("{n} 个文件"),
    }
}

pub fn transfer(app: &AppHandle, t: &TransferDto, copied: bool) {
    match (t.outgoing, t.state) {
        (false, "done") => {
            let target = t.saved.first().cloned();
            let dir = t.dir.clone().unwrap_or_default();
            show(
                app,
                &format!("已收到 {}{}", what(t), if copied { "，已复制" } else { "" }),
                &format!("来自 {} · 保存在 {dir}", t.device),
                target.map(|p| Box::new(move || reveal(&p)) as Box<dyn Fn() + Send>),
            );
        }
        (true, "done") if t.text.is_none() => show(app, &format!("已发送给 {}", t.device), &what(t), None),
        (_, "failed") => show(
            app,
            &format!("{} {} 失败", if t.outgoing { "发送给" } else { "接收自" }, t.device),
            t.error.as_deref().unwrap_or("传输中断"),
            None,
        ),
        _ => {}
    }
}

pub fn shared(app: &AppHandle, s: &SharedText) {
    let preview: String = s.text.chars().take(80).collect();
    if s.opened {
        show(app, &format!("已打开 {} 分享的链接", s.device), s.link.as_deref().unwrap_or_default(), None);
    } else {
        show(app, &format!("{} 分享了文字，已复制", s.device), &preview, None);
    }
}

fn reveal(path: &str) {
    if let Err(e) = tauri_plugin_opener::reveal_item_in_dir(path) {
        tracing::warn!(error = %e, "无法在资源管理器中显示文件");
    }
}

#[cfg(windows)]
fn show(app: &AppHandle, title: &str, body: &str, on_click: Option<Box<dyn Fn() + Send>>) {
    use tauri_winrt_notification::Toast;

    let id = if tauri::is_dev() { Toast::POWERSHELL_APP_ID.to_owned() } else { app.config().identifier.clone() };
    let mut toast = Toast::new(&id).title(title).text1(body);
    if let Some(f) = on_click {
        toast = toast.on_activated(move |_| {
            f();
            Ok(())
        });
    }
    if let Err(e) = toast.show() {
        tracing::warn!(error = %e, "系统通知发送失败");
    }
}

#[cfg(not(windows))]
fn show(_app: &AppHandle, _title: &str, _body: &str, _on_click: Option<Box<dyn Fn() + Send>>) {}
