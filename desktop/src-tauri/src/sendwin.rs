use std::sync::Mutex;

use copycat_app::Res;
use tauri::plugin::TauriPlugin;
use tauri::{AppHandle, Emitter, Manager, WebviewUrl, WebviewWindowBuilder, Wry};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut, ShortcutState};

pub const LABEL: &str = "send";

#[derive(Default)]
pub struct PendingPaths(pub Mutex<Vec<String>>);

pub fn plugin() -> TauriPlugin<Wry> {
    tauri_plugin_global_shortcut::Builder::new()
        .with_handler(|app, _shortcut, ev| {
            if ev.state() == ShortcutState::Pressed {
                toggle(app);
            }
        })
        .build()
}

pub fn register_hotkey(app: &AppHandle, keys: &str) -> Res<()> {
    let keys = keys.trim();
    let shortcut = if keys.is_empty() {
        None
    } else {
        Some(keys.parse::<Shortcut>().map_err(|_| format!("无法识别快捷键 {keys}"))?)
    };
    let gs = app.global_shortcut();
    let _ = gs.unregister_all();
    if let Some(s) = shortcut {
        gs.register(s).map_err(|_| format!("快捷键 {keys} 无法注册，可能已被其他程序占用"))?;
    }
    Ok(())
}

fn toggle(app: &AppHandle) {
    match app.get_webview_window(LABEL) {
        Some(w) if w.is_visible().unwrap_or(false) => {
            let _ = w.hide();
        }
        _ => {
            #[cfg(windows)]
            let fg = copycat_win::explorer::foreground();
            show(app);
            #[cfg(windows)]
            {
                let app = app.clone();
                std::thread::spawn(move || {
                    let paths = copycat_win::explorer::selected_paths(fg);
                    if paths.is_empty() {
                        return;
                    }
                    if let Some(p) = app.try_state::<PendingPaths>() {
                        *p.0.lock().unwrap_or_else(|e| e.into_inner()) = paths;
                    }
                    let _ = app.emit_to(LABEL, "cc:send-paths", ());
                });
            }
        }
    }
}

pub fn take_paths(app: &AppHandle) -> Vec<String> {
    app.try_state::<PendingPaths>()
        .map(|p| std::mem::take(&mut *p.0.lock().unwrap_or_else(|e| e.into_inner())))
        .unwrap_or_default()
}

pub fn show(app: &AppHandle) {
    let w = match app.get_webview_window(LABEL) {
        Some(w) => w,
        None => {
            let built = WebviewWindowBuilder::new(app, LABEL, WebviewUrl::App("index.html".into()))
                .title("发送文件 · CopyCat")
                .inner_size(520.0, 620.0)
                .resizable(false)
                .maximizable(false)
                .minimizable(false)
                .decorations(false)
                .shadow(true)
                .always_on_top(true)
                .skip_taskbar(true)
                .center()
                .background_color(tauri::window::Color(255, 255, 255, 255))
                .initialization_script("window.__CC_VIEW = 'send';")
                .build();
            match built {
                Ok(w) => w,
                Err(e) => {
                    tracing::warn!(error = %e, "无法创建发送窗口");
                    return;
                }
            }
        }
    };
    let _ = w.center();
    let _ = w.show();
    let _ = w.unminimize();
    let _ = w.set_focus();

    let _ = app.emit_to(LABEL, "cc:send-open", ());
}
