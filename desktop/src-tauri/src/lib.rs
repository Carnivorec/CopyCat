mod bridge;
mod commands;
pub mod firewall;
mod host;
mod logfile;
mod netwatch;
mod notify;
mod sendwin;
mod tray;

use tauri::{AppHandle, Manager, WindowEvent};

pub fn show_main(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.unminimize();
        let _ = w.show();
        let _ = w.set_focus();
    }
}

fn init(app: &tauri::App) -> Result<(), Box<dyn std::error::Error>> {
    let data_dir = app.path().app_local_data_dir()?;
    let hwnd = bridge::Hwnd::default();
    let changes = bridge::start_watcher(&hwnd)?;
    let svc = host::Service::new(host::DesktopHost { app: app.handle().clone(), hwnd }, data_dir)?;
    app.manage(svc.clone());
    app.manage(sendwin::PendingPaths::default());
    if let Err(e) = sendwin::register_hotkey(app.handle(), &svc.settings().send_hotkey) {
        tracing::warn!(error = %e, "发送窗口快捷键注册失败");
    }
    tauri::async_runtime::spawn(async move {
        if let Err(e) = svc.start().await {
            tracing::warn!(error = %e, "启动节点失败");
        }
        tauri::async_runtime::spawn(netwatch::run(svc.clone()));
        tauri::async_runtime::spawn_blocking(firewall::log_status);
        bridge::run(svc, changes).await;
        tracing::error!("剪贴板桥接任务意外结束，本机复制不再同步");
    });
    Ok(())
}

pub fn run() {
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| "warn,copycat_desktop=info,copycat_core=info,copycat_app=info".into());
    match logfile::open() {
        Some(f) => tracing_subscriber::fmt()
            .with_env_filter(filter)
            .with_ansi(false)
            .with_writer(std::sync::Mutex::new(f))
            .init(),
        None => tracing_subscriber::fmt().with_env_filter(filter).init(),
    }

    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        tracing::error!(%info, "程序内部错误");
        default_hook(info);
    }));
    tracing::info!(version = env!("CARGO_PKG_VERSION"), "CopyCat 启动");

    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| show_main(app)))
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            Some(vec!["--background"]),
        ))
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(sendwin::plugin())
        .setup(|app| {
            init(app)?;
            tray::build(app)?;
            if !std::env::args().any(|a| a == "--background") {
                show_main(app.handle());
            }
            Ok(())
        })
        .on_window_event(|w, e| {
            if let WindowEvent::CloseRequested { api, .. } = e {
                api.prevent_close();
                let _ = w.hide();
            }
        })
        .invoke_handler(tauri::generate_handler![
            commands::overview,
            commands::create_group,
            commands::join_group,
            commands::create_invite,
            commands::respond_pairing,
            commands::remove_device,
            commands::leave_group,
            commands::copy_item,
            commands::forget_item,
            commands::clear_history,
            commands::item_image,
            commands::item_text,
            commands::set_relay_enabled,
            commands::setup_relay,
            commands::relay_setup,
            commands::set_relay_addr,
            commands::remove_relay,
            commands::set_paused,
            commands::save_settings,
            commands::network_changed,
            firewall::firewall_status,
            firewall::firewall_allow,
            commands::transfers,
            commands::summarize,
            commands::send_paths,
            commands::cancel_transfer,
            commands::clear_transfers,
            commands::reveal_transfer,
            commands::open_receive_dir,
            commands::pick_files,
            commands::pick_folder,
            commands::show_send,
            commands::take_send_paths,
            commands::show_main,
        ])
        .run(tauri::generate_context!())
        .expect("启动 CopyCat 失败");
}
