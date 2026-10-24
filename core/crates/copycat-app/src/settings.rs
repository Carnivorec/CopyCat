use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Settings {
    pub device_name: String,
    pub sync_images: bool,
    pub lan_enabled: bool,

    pub mailbox: bool,

    pub relay_enabled: bool,

    pub excluded_apps: Vec<String>,

    pub receive_dir: String,

    pub ask_receive_dir: bool,

    pub copy_received: bool,

    pub copy_received_image: bool,

    pub open_shared_links: bool,

    pub send_hotkey: String,

    pub relay_confirm_mb: u32,

    pub screenshots: bool,

    pub screenshot_tools: Vec<String>,

    pub screenshot_keep_days: u32,

    pub screenshot_max_count: u32,

    pub screenshot_max_mb: u32,

    pub history_days: u32,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            device_name: String::new(),
            sync_images: true,
            lan_enabled: true,
            mailbox: true,
            relay_enabled: true,
            excluded_apps: Vec::new(),
            receive_dir: String::new(),
            ask_receive_dir: false,
            copy_received: false,
            copy_received_image: true,
            open_shared_links: true,
            send_hotkey: DEFAULT_HOTKEY.into(),
            relay_confirm_mb: 50,
            screenshots: false,
            screenshot_tools: DEFAULT_SCREENSHOT_TOOLS.iter().map(|s| (*s).to_owned()).collect(),
            screenshot_keep_days: 0,
            screenshot_max_count: 0,
            screenshot_max_mb: 0,
            history_days: 1,
        }
    }
}

pub const DEFAULT_HOTKEY: &str = "Ctrl+Shift+F";

pub const DEFAULT_SCREENSHOT_TOOLS: &[&str] = &[
    "SnippingTool.exe",
    "ScreenClippingHost.exe",
    "ScreenSketch.exe",
    "Snipaste.exe",
    "PixPin.exe",
    "ShareX.exe",
    "Greenshot.exe",
    "FSCapture.exe",
    "Lightshot.exe",
    "flameshot.exe",
];

const REMOTE_CONTROL_TOOLS: &[&str] =
    &["gameviewer", "rdpclip", "sunlogin", "todesk", "anydesk", "teamviewer", "rustdesk", "parsec"];

const FILE: &str = "settings.json";

const LEGACY_FILE: &str = "desktop.json";

fn path(dir: &Path) -> PathBuf {
    dir.join(FILE)
}

impl Settings {
    pub fn load(dir: &Path, defaults: impl FnOnce() -> Self) -> Self {
        [FILE, LEGACY_FILE]
            .iter()
            .find_map(|f| std::fs::read(dir.join(f)).ok())
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_else(defaults)
    }

    pub fn save(&self, dir: &Path) -> std::io::Result<()> {
        let tmp = dir.join("settings.json.tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(self).expect("可序列化"))?;
        std::fs::rename(tmp, path(dir))?;
        let _ = std::fs::remove_file(dir.join(LEGACY_FILE));
        Ok(())
    }

    pub fn is_excluded(&self, source: Option<&str>) -> bool {
        let Some(source) = source else { return false };
        self.excluded_apps.iter().any(|x| x.eq_ignore_ascii_case(file_name(source)))
    }

    pub fn is_screenshot_source(&self, source: Option<&str>) -> bool {
        match source {
            None => true,
            Some(s) => self.screenshot_tools.iter().any(|x| x.eq_ignore_ascii_case(file_name(s))),
        }
    }
}

pub fn is_remote_control_source(source: Option<&str>) -> bool {
    let Some(name) = source.map(file_name) else { return false };
    let name = name.to_ascii_lowercase();
    REMOTE_CONTROL_TOOLS.iter().any(|t| name.starts_with(t))
}

pub(crate) fn file_name(path: &str) -> &str {
    path.rsplit(['\\', '/']).next().unwrap_or(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exclusion_matches_file_or_package_name_case_insensitive() {
        let s =
            Settings { excluded_apps: vec!["KeePass.exe".into(), "com.x8bit.bitwarden".into()], ..Settings::default() };
        assert!(s.is_excluded(Some(r"C:\Program Files\KeePass\keepass.exe")));
        assert!(s.is_excluded(Some("com.x8bit.bitwarden")));
        assert!(!s.is_excluded(Some(r"C:\Windows\notepad.exe")));
        assert!(!s.is_excluded(Some("com.tencent.tim")));
        assert!(!s.is_excluded(None));
    }

    #[test]
    fn remote_control_sources() {
        assert!(is_remote_control_source(Some(r"C:\Program Files (x86)\Netease\UURemote\GameViewer.exe")));
        assert!(is_remote_control_source(Some(r"C:\Windows\System32\rdpclip.exe")));
        assert!(is_remote_control_source(Some(r"C:\Program Files\Oray\SunLogin\SunloginClient.exe")));
        assert!(!is_remote_control_source(Some(r"C:\Program Files\PixPin\PixPin.exe")));
        assert!(!is_remote_control_source(None));
    }

    #[test]
    fn screenshot_sources() {
        let s = Settings::default();
        assert!(s.is_screenshot_source(Some(
            r"C:\Program Files\WindowsApps\Microsoft.ScreenSketch\SnippingTool\SnippingTool.exe"
        )));
        assert!(s.is_screenshot_source(Some(r"D:\Tools\snipaste.exe")));
        assert!(s.is_screenshot_source(None), "按 PrtScn 键时剪贴板没有所有者");
        assert!(!s.is_screenshot_source(Some(r"C:\Program Files\Google\Chrome\Application\chrome.exe")));
    }

    #[test]
    fn load_falls_back_to_defaults_then_roundtrips() {
        let dir = tempfile::tempdir().unwrap();
        let s = Settings::load(dir.path(), || Settings { device_name: "默认".into(), ..Settings::default() });
        assert_eq!(s.device_name, "默认");
        let s2 = Settings { device_name: "台式机".into(), mailbox: false, ..s };
        s2.save(dir.path()).unwrap();
        let back = Settings::load(dir.path(), Settings::default);
        assert_eq!(back.device_name, "台式机");
        assert!(!back.mailbox);
    }
}
