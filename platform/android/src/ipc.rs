use copycat_app::{Host, Res, Service, Settings};
use serde::Deserialize;
use serde_json::{Value, json};

#[derive(Deserialize)]
#[serde(tag = "cmd", rename_all = "camelCase", rename_all_fields = "camelCase")]
enum Request {
    Overview,
    CreateGroup {
        name: String,
    },
    JoinGroup {
        uri: String,
        name: String,
    },
    CreateInvite,
    RespondPairing {
        accept: bool,
    },
    RemoveDevice {
        id: String,
    },
    LeaveGroup,
    CopyItem {
        id: String,
    },
    ForgetItem {
        id: String,
    },
    ClearHistory {
        #[serde(default)]
        everywhere: bool,
    },
    ItemImage {
        id: String,
    },
    ItemText {
        id: String,
    },
    SetRelayEnabled {
        enabled: bool,
    },
    SetPaused {
        paused: bool,
    },
    SaveSettings {
        settings: Settings,
    },
    NetworkChanged,
    DaemonInfo,
    ShareText {
        device: String,
        text: String,
    },
    Transfers,
    CancelTransfer {
        id: String,
    },
    ClearTransfers,
}

fn to_value<T: serde::Serialize>(r: Res<T>) -> Res<Value> {
    r.map(|v| serde_json::to_value(v).unwrap_or(Value::Null))
}

pub async fn handle<H: Host>(svc: &Service<H>, req: &str) -> String {
    let out = match serde_json::from_str::<Request>(req) {
        Err(e) => Err(format!("无法识别的请求：{e}")),
        Ok(r) => dispatch(svc, r).await,
    };
    match out {
        Ok(v) => json!({ "ok": v }).to_string(),
        Err(e) => json!({ "err": e }).to_string(),
    }
}

async fn dispatch<H: Host>(svc: &Service<H>, r: Request) -> Res<Value> {
    match r {
        Request::Overview => to_value(svc.overview().await),
        Request::CreateGroup { name } => to_value(svc.create_group(&name).await),
        Request::JoinGroup { uri, name } => to_value(svc.join_group(&uri, &name).await),
        Request::CreateInvite => to_value(svc.create_invite().await),
        Request::RespondPairing { accept } => to_value(svc.respond_pairing(accept).await),
        Request::RemoveDevice { id } => to_value(svc.remove_device(&id).await),
        Request::LeaveGroup => to_value(svc.leave_group().await),
        Request::CopyItem { id } => to_value(svc.copy_item(&id).await),
        Request::ForgetItem { id } => to_value(svc.forget_item(&id).await),
        Request::ClearHistory { everywhere: false } => to_value(svc.clear_history().await),
        Request::ClearHistory { everywhere: true } => to_value(svc.clear_history_everywhere().await),
        Request::ItemImage { id } => to_value(svc.item_image(&id).await),
        Request::ItemText { id } => to_value(svc.item_text(&id).await),
        Request::SetRelayEnabled { enabled } => to_value(svc.set_relay_enabled(enabled).await),
        Request::SetPaused { paused } => {
            svc.set_paused(paused);
            Ok(Value::Null)
        }
        Request::SaveSettings { settings } => to_value(svc.save_settings(settings)),
        Request::NetworkChanged => {
            svc.network_changed().await;
            Ok(Value::Null)
        }
        Request::ShareText { device, text } => to_value(svc.share_text(&device, &text).await),
        Request::Transfers => to_value(svc.transfers().await),
        Request::CancelTransfer { id } => to_value(svc.cancel_transfer(&id).await),
        Request::ClearTransfers => to_value(svc.clear_transfers().await),
        Request::DaemonInfo => Ok(json!({
            "version": env!("CARGO_PKG_VERSION"),
            "pid": std::process::id(),
            "dataDir": svc.data_dir().display().to_string(),
        })),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::daemon::Daemon;

    #[test]
    fn requests_roundtrip_through_service() {
        let dir = tempfile::tempdir().unwrap();
        let d = Daemon::start(dir.path().to_path_buf(), [9; 32], "手机".into()).unwrap();
        let run = |req: &str| d.rt.block_on(handle(&d.svc, req));

        let ov: Value = serde_json::from_str(&run(r#"{"cmd":"overview"}"#)).unwrap();
        assert_eq!(ov["ok"]["initialized"], false);
        assert_eq!(ov["ok"]["settings"]["deviceName"], "手机");

        let err: Value = serde_json::from_str(&run(r#"{"cmd":"createInvite"}"#)).unwrap();
        assert_eq!(err["err"], "还没有加入设备组");

        let bad: Value = serde_json::from_str(&run(r#"{"cmd":"joinGroup","uri":"https://x","name":"手机"}"#)).unwrap();
        assert_eq!(bad["err"], "这不是有效的 CopyCat 配对码");

        let mut s = d.svc.settings();
        s.mailbox = false;
        let req = json!({ "cmd": "saveSettings", "settings": s }).to_string();
        assert_eq!(run(&req), r#"{"ok":null}"#);
        assert!(!d.svc.settings().mailbox);

        let unknown: Value = serde_json::from_str(&run(r#"{"cmd":"rmRf"}"#)).unwrap();
        assert!(unknown["err"].as_str().unwrap().starts_with("无法识别的请求"));
    }
}
