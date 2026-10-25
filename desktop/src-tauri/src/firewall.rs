use std::path::PathBuf;

use copycat_app::Res;
use copycat_win::firewall::{self, State, Status};
use serde::Serialize;

pub fn cli() -> Option<i32> {
    let mut args = std::env::args().skip(1);
    if args.next()? != "--firewall" {
        return None;
    }
    let Ok(exe) = std::env::current_exe() else { return Some(1) };
    Some(match args.next().as_deref() {
        Some("allow") => match firewall::status(&exe) {
            Ok(Status { state: State::Open, .. }) => 0,
            _ if firewall::is_admin() => i32::from(firewall::allow(&exe).is_err()),
            _ => elevate(&exe, "--firewall allow"),
        },
        Some("remove") => match firewall::has_own_rule(&exe) {
            Ok(false) => 0,
            _ if firewall::is_admin() => i32::from(firewall::remove(&exe).is_err()),
            _ => elevate(&exe, "--firewall remove"),
        },
        _ => 64,
    })
}

fn elevate(exe: &std::path::Path, args: &str) -> i32 {
    match firewall::run_elevated(exe, args) {
        Ok(Some(code)) => code as i32,
        Ok(None) => 2,
        Err(_) => 1,
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FirewallView {
    pub state: &'static str,
    pub public: bool,
}

fn exe() -> Res<PathBuf> {
    std::env::current_exe().map_err(|e| format!("找不到程序路径：{e}"))
}

fn view(s: Status) -> FirewallView {
    let state = match s.state {
        State::Open => "open",
        State::NoRule => "noRule",
        State::Blocked => "blocked",
        State::BlockAll => "blockAll",
    };
    FirewallView { state, public: s.public }
}

fn query() -> Res<Status> {
    firewall::status(&exe()?).map_err(|e| format!("读取防火墙规则失败：{e}"))
}

pub fn log_status() {
    match query() {
        Ok(Status { state: State::Open, public }) => tracing::info!(public, "Windows 防火墙已放行局域网直连"),
        Ok(Status { state, public }) => {
            tracing::warn!(?state, public, "Windows 防火墙会拦下局域网内其他设备的连接")
        }
        Err(e) => tracing::warn!(error = %e, "读取防火墙规则失败"),
    }
}

#[tauri::command]
pub async fn firewall_status() -> Res<FirewallView> {
    tauri::async_runtime::spawn_blocking(query).await.map_err(|e| e.to_string())?.map(view)
}

#[tauri::command]
pub async fn firewall_allow() -> Res<FirewallView> {
    tauri::async_runtime::spawn_blocking(|| {
        let exe = exe()?;
        match firewall::run_elevated(&exe, "--firewall allow") {
            Ok(Some(0)) => {}
            Ok(None) => return Err("已取消".to_owned()),
            Ok(Some(code)) => return Err(format!("修改防火墙规则失败（退出码 {code}）")),
            Err(e) => return Err(format!("无法以管理员身份运行：{e}")),
        }
        let s = query()?;
        tracing::info!(state = ?s.state, "已放行 Windows 防火墙");
        Ok(view(s))
    })
    .await
    .map_err(|e| e.to_string())?
}
