use copycat_core::{ClipItem, DeviceId, DeviceRecord, NodeStatus, Platform};
use serde::Serialize;

use crate::settings::Settings;
use crate::transfer::TransferDto;

pub const MIME_TEXT: &str = "text/plain";
pub const MIME_PNG: &str = "image/png";

const PREVIEW_CHARS: usize = 600;

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct DeviceDto {
    pub id: String,
    pub short: String,
    pub name: String,
    pub platform: &'static str,
    pub me: bool,
    pub relay: bool,
    pub lan: bool,

    pub screenshots: bool,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ClipDto {
    pub id: String,
    pub device: String,
    pub platform: &'static str,
    pub mine: bool,
    pub time: u64,
    pub kind: &'static str,
    pub text: Option<String>,
    pub chars: usize,
    pub has_image: bool,

    pub pending: bool,
    pub size: u64,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct RelayDto {
    pub addr: String,
    pub configured: bool,
    pub connected: bool,

    pub enabled: bool,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Overview {
    pub initialized: bool,
    pub me: Option<DeviceDto>,
    pub relay: Option<RelayDto>,
    pub devices: Vec<DeviceDto>,
    pub history: Vec<ClipDto>,
    pub settings: Settings,
    pub paused: bool,
    pub lan_port: Option<u16>,

    pub transfers: Vec<TransferDto>,

    pub receive_dir: String,
}

pub fn platform_str(p: Platform) -> &'static str {
    match p {
        Platform::Windows => "windows",
        Platform::Android => "android",
        Platform::Other(_) => "other",
    }
}

pub fn devices(members: &[DeviceRecord], status: &NodeStatus) -> Vec<DeviceDto> {
    let mut v: Vec<DeviceDto> = members
        .iter()
        .map(|m| {
            let st = status.peers.iter().find(|p| p.id == m.id);
            DeviceDto {
                id: m.id.to_string(),
                short: m.id.short(),
                name: m.name.clone(),
                platform: platform_str(m.platform),
                me: m.id == status.me,
                relay: st.is_some_and(|s| s.relay),
                lan: st.is_some_and(|s| s.lan),
                screenshots: st.is_some_and(|s| s.screenshots),
            }
        })
        .collect();

    v.sort_by(|a, b| b.me.cmp(&a.me).then_with(|| a.name.cmp(&b.name)));
    v
}

pub fn clip(it: &ClipItem, members: &[DeviceRecord], me: DeviceId) -> ClipDto {
    let rec = members.iter().find(|m| m.id == it.origin);
    let text = it
        .reps
        .iter()
        .find(|r| r.mime == MIME_TEXT)
        .and_then(|r| r.inline.as_ref())
        .map(|b| String::from_utf8_lossy(b).into_owned());
    let has_image = it.reps.iter().any(|r| r.mime == MIME_PNG);
    let kind = match (&text, has_image) {
        (Some(t), _) => classify(t),
        (None, true) => "image",
        (None, false) => "other",
    };
    ClipDto {
        id: hex::encode(it.id),
        device: rec.map_or_else(|| "已移除的设备".to_owned(), |r| r.name.clone()),
        platform: rec.map_or("other", |r| platform_str(r.platform)),
        mine: it.origin == me,
        time: it.hlc.wall_ms,
        kind,
        chars: text.as_ref().map_or(0, |t| t.chars().count()),
        text: text.map(|t| t.chars().take(PREVIEW_CHARS).collect()),
        has_image,
        pending: !it.is_complete(),
        size: it.reps.iter().map(|r| r.size).max().unwrap_or(0),
    }
}

pub fn history(items: &[ClipItem], members: &[DeviceRecord], me: DeviceId) -> Vec<ClipDto> {
    let mut seen = std::collections::HashSet::new();
    items.iter().filter(|it| it.fingerprint().is_none_or(|f| seen.insert(f))).map(|it| clip(it, members, me)).collect()
}

pub fn classify(text: &str) -> &'static str {
    let t = text.trim();
    let digits: String = t.chars().filter(char::is_ascii_digit).collect();
    let only_digits = !t.is_empty() && t.chars().all(|c| c.is_ascii_digit() || c == ' ' || c == '-');
    let otp_words = ["验证码", "校验码", "动态码", "code", "Code", "OTP"];
    if (4..=8).contains(&digits.len())
        && (only_digits || (t.chars().count() <= 40 && otp_words.iter().any(|w| t.contains(w))))
    {
        return "otp";
    }
    let single_line = !t.contains('\n');
    let no_space = !t.contains(char::is_whitespace);
    if single_line && no_space && t.len() < 400 {
        let lower = t.to_ascii_lowercase();
        let looks_domain = t.contains('.') && t.chars().all(|c| c.is_ascii_graphic()) && !t.ends_with('.');
        if lower.starts_with("http://") || lower.starts_with("https://") || (looks_domain && t.contains('/')) {
            return "link";
        }
    }
    let code_starts = [
        "$ ", "cargo ", "git ", "npm ", "pnpm ", "sudo ", "cd ", "docker ", "kubectl ", "python ", "fn ", "let ",
        "const ", "import ", "def ", "#include", "SELECT ", "select ",
    ];
    let code_marks = ["=>", "::", "();", "{\n", "};", "&&", "|| "];
    if code_starts.iter().any(|p| t.starts_with(p)) || code_marks.iter().any(|m| t.contains(m)) {
        return "code";
    }
    "text"
}

#[cfg(test)]
mod tests {
    use super::classify;

    #[test]
    fn kinds() {
        assert_eq!(classify("482913"), "otp");
        assert_eq!(classify("您的验证码是 730165，五分钟内有效"), "otp");
        assert_eq!(classify("https://example.com/a?b=1"), "link");
        assert_eq!(classify("github.com/Carnivorec/CopyCat"), "link");
        assert_eq!(classify("cargo test --workspace"), "code");
        assert_eq!(classify("let x = foo::bar();"), "code");
        assert_eq!(classify("会议纪要：周五 14:00，三楼会议室"), "text");
        assert_eq!(classify("2026 年 10 月 3 日"), "text");
        assert_eq!(classify("v1.2.3"), "text");
    }
}
