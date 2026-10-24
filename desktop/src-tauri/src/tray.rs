use tauri::menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager, Wry};

use crate::host::Service;

const TRAY_ID: &str = "main";

pub struct TrayItems {
    pause: CheckMenuItem<Wry>,
}

pub fn build(app: &tauri::App) -> tauri::Result<()> {
    let show = MenuItem::with_id(app, "show", "显示 CopyCat", true, None::<&str>)?;
    let send = MenuItem::with_id(app, "send", "发送文件…", true, None::<&str>)?;
    let pause = CheckMenuItem::with_id(app, "pause", "暂停同步", true, false, None::<&str>)?;
    let pause_hour = MenuItem::with_id(app, "pause-hour", "暂停 1 小时", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "退出", true, None::<&str>)?;
    let menu =
        Menu::with_items(app, &[&show, &send, &pause, &pause_hour, &PredefinedMenuItem::separator(app)?, &quit])?;
    app.manage(TrayItems { pause: pause.clone() });

    let mut builder = TrayIconBuilder::with_id(TRAY_ID)
        .tooltip("CopyCat")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, e| match e.id.as_ref() {
            "show" => crate::show_main(app),
            "send" => crate::sendwin::show(app),
            "pause" => {
                let svc = app.state::<Service>();
                let now = !svc.is_paused();
                svc.set_paused(now);
                sync_pause(app, now);
            }

            "pause-hour" => app.state::<Service>().pause_for(std::time::Duration::from_secs(3600)),
            "quit" => {
                let app = app.clone();
                tauri::async_runtime::spawn(async move {
                    let svc = app.state::<Service>().inner().clone();
                    svc.shutdown().await;
                    app.exit(0);
                });
            }
            _ => {}
        })
        .on_tray_icon_event(|tray, e| {
            if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = e {
                crate::show_main(tray.app_handle());
            }
        });
    if let Some(icon) = app.default_window_icon() {
        builder = builder.icon(icon.clone());
    }
    builder.build(app)?;
    Ok(())
}

pub fn sync_pause(app: &AppHandle, paused: bool) {
    if let Some(items) = app.try_state::<TrayItems>() {
        let _ = items.pause.set_checked(paused);
    }
    if let Some(t) = app.tray_by_id(TRAY_ID) {
        let _ = t.set_tooltip(Some(if paused { "CopyCat · 已暂停" } else { "CopyCat" }));
    }
}
