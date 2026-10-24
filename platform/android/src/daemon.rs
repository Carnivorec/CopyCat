use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};

use copycat_app::{Host, MIME_TEXT, Res, Service, Settings, TransferDto, UiEvent};
use copycat_core::{ClipItem, Platform};
use serde::Serialize;
use tracing::{debug, info};

pub const MIME_HTML: &str = "text/html";

pub const RECEIVE_DIR: &str = "/storage/emulated/0/Download/CopyCat";

#[derive(Serialize, Debug, Clone, PartialEq)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum Action {
    Write { text: String, html: Option<String>, mark: bool },

    Event { json: String },

    Attach,

    Finished { json: String },

    Removed { paths: Vec<String> },

    Exit,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Clip {
    pub text: Option<String>,
    pub html: Option<String>,
}

impl Clip {
    pub fn is_empty(&self) -> bool {
        self.text.as_deref().is_none_or(str::is_empty) && self.html.as_deref().is_none_or(str::is_empty)
    }

    pub fn reps(&self) -> Vec<(String, Vec<u8>)> {
        let mut v = Vec::new();
        if let Some(t) = self.text.as_ref().filter(|t| !t.is_empty()) {
            v.push((MIME_TEXT.to_owned(), t.as_bytes().to_vec()));
        }
        if let Some(h) = self.html.as_ref().filter(|h| !h.is_empty()) {
            v.push((MIME_HTML.to_owned(), h.as_bytes().to_vec()));
        }
        v
    }

    pub fn from_item(item: &ClipItem) -> Self {
        let get = |mime: &str| {
            item.reps
                .iter()
                .find(|r| r.mime == mime)
                .and_then(|r| r.inline.as_ref())
                .map(|b| String::from_utf8_lossy(b).into_owned())
        };
        Self { text: get(MIME_TEXT), html: get(MIME_HTML) }
    }
}

pub struct AndroidHost {
    key: [u8; 32],
    device_name: String,
    actions: Sender<Action>,

    last: Arc<Mutex<Option<Clip>>>,
}

impl AndroidHost {
    pub fn send(&self, a: Action) {
        let _ = self.actions.send(a);
    }
}

impl Host for AndroidHost {
    fn platform(&self) -> Platform {
        Platform::Android
    }

    fn default_settings(&self) -> Settings {
        Settings {
            device_name: self.device_name.clone(),
            excluded_apps: vec![
                "com.x8bit.bitwarden".into(),
                "com.kunzisoft.keepass.free".into(),
                "com.kunzisoft.keepass.libre".into(),
                "keepass2android.keepass2android".into(),
                "com.agilebits.onepassword".into(),
            ],
            ..Settings::default()
        }
    }

    fn master_key(&self, _dir: &Path) -> Res<[u8; 32]> {
        Ok(self.key)
    }

    async fn write_clipboard(&self, item: ClipItem, remote: bool) -> Res<()> {
        let clip = Clip::from_item(&item);
        let Some(text) = clip.text.clone().filter(|t| !t.is_empty()) else {
            debug!("暂不支持写入非文本内容");

            return if remote { Ok(()) } else { Err("手机上暂时只能复制文字".into()) };
        };
        if remote {
            *self.last.lock().unwrap_or_else(|p| p.into_inner()) = Some(clip.clone());
        }
        self.send(Action::Write { text, html: clip.html, mark: remote });
        Ok(())
    }

    fn emit(&self, ev: UiEvent) {
        if let Ok(json) = serde_json::to_string(&ev) {
            self.send(Action::Event { json });
        }
    }

    fn default_receive_dir(&self) -> PathBuf {
        PathBuf::from(RECEIVE_DIR)
    }

    fn open_url(&self, _url: &str) {}

    fn transfer_finished(&self, t: &TransferDto) {
        if let Ok(json) = serde_json::to_string(t) {
            self.send(Action::Finished { json });
        }
    }

    fn files_removed(&self, paths: &[PathBuf]) {
        let paths = paths.iter().map(|p| p.to_string_lossy().into_owned()).collect();
        self.send(Action::Removed { paths });
    }
}

pub struct Daemon {
    pub rt: tokio::runtime::Runtime,
    pub svc: Service<AndroidHost>,
    actions: Mutex<Receiver<Action>>,
    last: Arc<Mutex<Option<Clip>>>,
}

impl Daemon {
    pub fn start(data_dir: PathBuf, key: [u8; 32], device_name: String) -> Result<Self, String> {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("copycat")
            .enable_all()
            .build()
            .map_err(|e| format!("创建运行时失败：{e}"))?;
        let (tx, rx) = channel();
        let last = Arc::new(Mutex::new(None));
        let host = AndroidHost { key, device_name, actions: tx, last: last.clone() };
        let svc = Service::new(host, data_dir).map_err(|e| format!("创建数据目录失败：{e}"))?;
        let s2 = svc.clone();
        let joined = rt.block_on(async move { s2.start().await })?;
        info!(joined, "守护进程已启动");
        Ok(Self { rt, svc, actions: Mutex::new(rx), last })
    }

    pub fn local_clip(&self, clip: Clip, source: Option<String>) {
        if clip.is_empty() {
            return;
        }
        {
            let mut last = self.last.lock().unwrap_or_else(|p| p.into_inner());
            if last.as_ref() == Some(&clip) {
                return;
            }
            *last = Some(clip.clone());
        }
        let svc = self.svc.clone();
        self.rt.spawn(async move {
            svc.local_copy(clip.reps(), source.as_deref()).await;
        });
    }

    pub fn send_files(&self, device: &str, files: Vec<(String, std::fs::File)>) -> String {
        let svc = self.svc.clone();
        let r = self.rt.block_on(async move { svc.send_open_files(device, files).await });
        match r {
            Ok(id) => serde_json::json!({ "ok": id }).to_string(),
            Err(e) => serde_json::json!({ "err": e }).to_string(),
        }
    }

    pub fn next_action(&self) -> Option<Action> {
        self.actions.lock().unwrap_or_else(|p| p.into_inner()).recv().ok()
    }

    pub fn stop(&self) {
        let svc = self.svc.clone();
        self.rt.block_on(async move { svc.shutdown().await });
        self.svc.host().send(Action::Exit);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reps_put_plain_text_first_and_skip_empty() {
        let c = Clip { text: Some("你好".into()), html: Some("<b>你好</b>".into()) };
        let r = c.reps();
        assert_eq!(r[0].0, MIME_TEXT);
        assert_eq!(r[1].0, MIME_HTML);
        let empty = Clip { text: Some(String::new()), html: None };
        assert!(empty.is_empty());
        assert!(empty.reps().is_empty());
    }

    #[test]
    fn local_clip_dedupes_and_remote_write_marks_last() {
        let dir = tempfile::tempdir().unwrap();
        let d = Daemon::start(dir.path().to_path_buf(), [7; 32], "测试手机".into()).unwrap();

        let c = Clip { text: Some("一段文字".into()), html: None };
        d.local_clip(c.clone(), None);
        assert_eq!(d.last.lock().unwrap().as_ref(), Some(&c));

        let item = ClipItem {
            id: [1; 16],
            origin: copycat_core::DeviceId([2; 16]),
            seq: 1,
            hlc: copycat_core::Hlc { wall_ms: 1, counter: 0, node: copycat_core::DeviceId([2; 16]) },
            reps: vec![copycat_core::Rep::from_bytes(MIME_TEXT, "远端内容".as_bytes().to_vec())],
        };
        d.rt.block_on(d.svc.host().write_clipboard(item, true)).unwrap();
        let a = d.next_action().unwrap();
        assert_eq!(a, Action::Write { text: "远端内容".into(), html: None, mark: true });
        assert_eq!(d.last.lock().unwrap().as_ref().unwrap().text.as_deref(), Some("远端内容"));
        assert_eq!(d.svc.settings().device_name, "测试手机");
    }

    #[test]
    fn action_json_shape() {
        let j = serde_json::to_string(&Action::Write { text: "a".into(), html: None, mark: true }).unwrap();
        assert_eq!(j, r#"{"type":"write","text":"a","html":null,"mark":true}"#);
        assert_eq!(serde_json::to_string(&Action::Exit).unwrap(), r#"{"type":"exit"}"#);
    }
}
