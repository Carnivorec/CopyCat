use std::path::{Path, PathBuf};

use copycat_app::{Host, Res, Settings, TransferDto, UiEvent};
use copycat_core::{ClipItem, Platform};
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_dialog::DialogExt;
use tauri_plugin_opener::OpenerExt;

use crate::{bridge, notify};

pub type Service = copycat_app::Service<DesktopHost>;

pub struct DesktopHost {
    pub app: AppHandle,

    pub hwnd: bridge::Hwnd,
}

impl Host for DesktopHost {
    fn platform(&self) -> Platform {
        Platform::Windows
    }

    fn default_settings(&self) -> Settings {
        Settings {
            device_name: std::env::var("COMPUTERNAME").unwrap_or_else(|_| "Windows 电脑".into()),
            excluded_apps: vec![
                "KeePass.exe".into(),
                "KeePassXC.exe".into(),
                "1Password.exe".into(),
                "Bitwarden.exe".into(),
            ],
            ..Settings::default()
        }
    }

    fn master_key(&self, dir: &Path) -> Res<[u8; 32]> {
        master_key(dir)
    }

    async fn write_clipboard(&self, item: ClipItem, remote: bool) -> Res<()> {
        bridge::write(&self.hwnd, &item, remote).await
    }

    fn emit(&self, ev: UiEvent) {
        let app = &self.app;
        let _ = match ev {
            UiEvent::Changed(what) => {
                if what == "status"
                    && let Some(svc) = app.try_state::<Service>()
                {
                    crate::tray::sync_pause(app, svc.is_paused());
                }
                app.emit("cc:changed", what)
            }
            UiEvent::Arrived(id) => app.emit("cc:arrived", id),
            UiEvent::PairRequest(req) => {
                crate::show_main(app);
                app.emit("cc:pair-request", req)
            }
            UiEvent::Paired(id) => app.emit("cc:paired", id),
            UiEvent::Revoked => app.emit("cc:revoked", ()),
            UiEvent::JoinSas(sas) => app.emit("cc:join-sas", sas),
            UiEvent::Transfer(t) => app.emit("cc:transfer", t),
            UiEvent::SharedText(s) => {
                notify::shared(app, &s);
                app.emit("cc:shared", s)
            }
        };
    }

    fn default_receive_dir(&self) -> PathBuf {
        let downloads = self.app.path().download_dir().unwrap_or_else(|_| {
            std::env::var_os("USERPROFILE").map(PathBuf::from).unwrap_or_default().join("Downloads")
        });
        downloads.join("CopyCat")
    }

    async fn ask_receive_dir(&self, t: TransferDto) -> Option<PathBuf> {
        let (tx, rx) = tokio::sync::oneshot::channel();
        let what = if t.count == 1 {
            t.files.first().map_or_else(String::new, |f| f.name.clone())
        } else {
            format!("{} 个文件", t.count)
        };
        let start = copycat_app_receive_dir(&self.app);
        self.app
            .dialog()
            .file()
            .set_title(format!("保存 {} 发来的 {what}（取消则拒绝接收）", t.device))
            .set_directory(start)
            .pick_folder(move |p| {
                let _ = tx.send(p);
            });
        rx.await.ok().flatten().and_then(|p| p.into_path().ok())
    }

    fn open_url(&self, url: &str) {
        if let Err(e) = self.app.opener().open_url(url, None::<&str>) {
            tracing::warn!(error = %e, "无法打开链接");
        }
    }

    fn transfer_finished(&self, t: &TransferDto) {
        let s = self.app.state::<Service>().settings();
        if !(s.copy_received && !t.outgoing && t.state == "done" && !t.saved.is_empty()) {
            notify::transfer(&self.app, t, false);
            return;
        }

        let (app, hwnd, t) = (self.app.clone(), self.hwnd.clone(), t.clone());
        tauri::async_runtime::spawn(async move {
            let copied = bridge::copy_received(&hwnd, &t.saved, &t.saved_dirs, s.copy_received_image).await;
            notify::transfer(&app, &t, copied);
        });
    }

    fn writes_images(&self) -> bool {
        true
    }

    #[cfg(windows)]
    fn screenshot_name(&self) -> String {
        format!("截图_{}.png", copycat_win::time::local_stamp())
    }
}

fn copycat_app_receive_dir(app: &AppHandle) -> PathBuf {
    let dir = app.state::<Service>().receive_dir();
    let _ = std::fs::create_dir_all(&dir);
    dir
}

#[cfg(windows)]
fn master_key(dir: &Path) -> Res<[u8; 32]> {
    copycat_win::masterkey::load_or_create(&dir.join("master.key"), || {
        copycat_crypto::random::bytes::<32>().expect("系统随机数源可用")
    })
    .map_err(|e| format!("无法解开主密钥（是否换了 Windows 用户？）：{e}"))
}

#[cfg(not(windows))]
fn master_key(_dir: &Path) -> Res<[u8; 32]> {
    Err("仅支持 Windows".into())
}
