use std::path::PathBuf;

use copycat_app::{Invite, LeaveResult, Overview, RelaySetup, Res, SendSummary, Settings, TransferDto};
use tauri::{AppHandle, State};
use tauri_plugin_dialog::DialogExt;
use tauri_plugin_opener::OpenerExt;

use crate::host::Service;
use crate::{sendwin, tray};

#[tauri::command]
pub async fn overview(svc: State<'_, Service>) -> Res<Overview> {
    svc.overview().await
}

#[tauri::command]
pub async fn create_group(svc: State<'_, Service>, name: String) -> Res<()> {
    svc.create_group(&name).await
}

#[tauri::command]
pub async fn join_group(svc: State<'_, Service>, uri: String, name: String) -> Res<()> {
    svc.join_group(&uri, &name).await
}

#[tauri::command]
pub async fn create_invite(svc: State<'_, Service>) -> Res<Invite> {
    svc.create_invite().await
}

#[tauri::command]
pub async fn respond_pairing(svc: State<'_, Service>, accept: bool) -> Res<()> {
    svc.respond_pairing(accept).await
}

#[tauri::command]
pub async fn remove_device(svc: State<'_, Service>, id: String) -> Res<()> {
    svc.remove_device(&id).await
}

#[tauri::command]
pub async fn leave_group(svc: State<'_, Service>) -> Res<LeaveResult> {
    svc.leave_group().await
}

#[tauri::command]
pub async fn copy_item(svc: State<'_, Service>, id: String) -> Res<()> {
    svc.copy_item(&id).await
}

#[tauri::command]
pub async fn forget_item(svc: State<'_, Service>, id: String) -> Res<()> {
    svc.forget_item(&id).await
}

#[tauri::command]
pub async fn clear_history(svc: State<'_, Service>, everywhere: Option<bool>) -> Res<usize> {
    if everywhere == Some(true) { svc.clear_history_everywhere().await } else { svc.clear_history().await.map(|()| 0) }
}

#[tauri::command]
pub async fn item_image(svc: State<'_, Service>, id: String) -> Res<Option<String>> {
    svc.item_image(&id).await
}

#[tauri::command]
pub async fn item_text(svc: State<'_, Service>, id: String) -> Res<Option<String>> {
    svc.item_text(&id).await
}

#[tauri::command]
pub async fn setup_relay(svc: State<'_, Service>, addr: String) -> Res<RelaySetup> {
    svc.setup_relay(&addr).await
}

#[tauri::command]
pub async fn relay_setup(svc: State<'_, Service>) -> Res<Option<RelaySetup>> {
    svc.relay_setup().await
}

#[tauri::command]
pub async fn set_relay_addr(svc: State<'_, Service>, addr: String) -> Res<()> {
    svc.set_relay_addr(&addr).await
}

#[tauri::command]
pub async fn remove_relay(svc: State<'_, Service>) -> Res<()> {
    svc.remove_relay().await
}

#[tauri::command]
pub async fn set_relay_enabled(svc: State<'_, Service>, enabled: bool) -> Res<()> {
    svc.set_relay_enabled(enabled).await
}

#[tauri::command]
pub async fn set_paused(app: AppHandle, svc: State<'_, Service>, paused: bool) -> Res<()> {
    svc.set_paused(paused);
    tray::sync_pause(&app, paused);
    Ok(())
}

#[tauri::command]
pub async fn save_settings(app: AppHandle, svc: State<'_, Service>, settings: Settings) -> Res<()> {
    let old = svc.settings().send_hotkey;
    if old != settings.send_hotkey
        && let Err(e) = sendwin::register_hotkey(&app, &settings.send_hotkey)
    {
        let _ = sendwin::register_hotkey(&app, &old);
        return Err(e);
    }
    svc.save_settings(settings)
}

#[tauri::command]
pub async fn transfers(svc: State<'_, Service>) -> Res<Vec<TransferDto>> {
    svc.transfers().await
}

#[tauri::command]
pub async fn summarize(svc: State<'_, Service>, paths: Vec<String>) -> Res<SendSummary> {
    let svc = svc.inner().clone();
    let paths: Vec<PathBuf> = paths.into_iter().map(PathBuf::from).collect();
    tokio::task::spawn_blocking(move || svc.summarize(&paths)).await.map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn send_paths(svc: State<'_, Service>, device: String, paths: Vec<String>) -> Res<String> {
    let svc = svc.inner().clone();
    let paths: Vec<PathBuf> = paths.into_iter().map(PathBuf::from).collect();
    svc.send_paths(&device, &paths).await
}

#[tauri::command]
pub async fn cancel_transfer(svc: State<'_, Service>, id: String) -> Res<()> {
    svc.cancel_transfer(&id).await
}

#[tauri::command]
pub async fn clear_transfers(svc: State<'_, Service>) -> Res<()> {
    svc.clear_transfers().await
}

#[tauri::command]
pub async fn reveal_transfer(app: AppHandle, svc: State<'_, Service>, id: String) -> Res<()> {
    let t = svc.transfers().await?.into_iter().find(|t| t.id == id).ok_or("传输记录已不存在")?;
    let r = match (t.saved.first(), t.dir) {
        (Some(p), _) => app.opener().reveal_item_in_dir(p),
        (None, Some(d)) => app.opener().open_path(d, None::<&str>),
        (None, None) => return Err("还没有保存任何文件".into()),
    };
    r.map_err(|e| format!("无法打开：{e}"))
}

#[tauri::command]
pub async fn open_receive_dir(app: AppHandle, svc: State<'_, Service>) -> Res<()> {
    let dir = svc.receive_dir();
    std::fs::create_dir_all(&dir).map_err(|e| format!("无法创建接收目录：{e}"))?;
    app.opener().open_path(dir.display().to_string(), None::<&str>).map_err(|e| format!("无法打开：{e}"))
}

#[tauri::command]
pub async fn pick_files(app: AppHandle) -> Res<Vec<String>> {
    let files =
        tokio::task::spawn_blocking(move || app.dialog().file().set_title("选择要发送的文件").blocking_pick_files())
            .await
            .map_err(|e| e.to_string())?;
    Ok(files
        .unwrap_or_default()
        .into_iter()
        .filter_map(|f| f.into_path().ok())
        .map(|p| p.display().to_string())
        .collect())
}

#[tauri::command]
pub async fn pick_folder(app: AppHandle, title: String) -> Res<Option<String>> {
    let dir = tokio::task::spawn_blocking(move || app.dialog().file().set_title(title).blocking_pick_folder())
        .await
        .map_err(|e| e.to_string())?;
    Ok(dir.and_then(|d| d.into_path().ok()).map(|p| p.display().to_string()))
}

#[tauri::command]
pub fn show_send(app: AppHandle) {
    sendwin::show(&app);
}

#[tauri::command]
pub fn take_send_paths(app: AppHandle) -> Vec<String> {
    sendwin::take_paths(&app)
}

#[tauri::command]
pub fn show_main(app: AppHandle) {
    crate::show_main(&app);
}

#[tauri::command]
pub async fn network_changed(svc: State<'_, Service>) -> Res<()> {
    svc.network_changed().await;
    Ok(())
}
