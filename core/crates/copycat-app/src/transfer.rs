use std::fs::File;
use std::path::{Path, PathBuf};

use copycat_core::{DeviceRecord, FileSource, OutgoingFile, TransferInfo, TransferKind, TransferState};
use serde::Serialize;

use crate::Res;
use crate::dto::platform_str;

pub const MAX_SEND_FILES: usize = 10_000;

const LIST_FILES: usize = 20;

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct FileDto {
    pub name: String,
    pub size: u64,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct TransferDto {
    pub id: String,
    pub device_id: String,
    pub device: String,
    pub platform: &'static str,
    pub outgoing: bool,

    pub kind: &'static str,

    pub files: Vec<FileDto>,
    pub count: usize,
    pub text: Option<String>,
    pub total: u64,
    pub done: u64,
    pub rate: u64,

    pub state: &'static str,
    pub error: Option<String>,
    pub lan: bool,
    pub dir: Option<String>,
    pub saved: Vec<String>,

    pub saved_dirs: Vec<bool>,
    pub time: u64,
    pub finished: Option<u64>,
}

pub fn transfer_dto(t: &TransferInfo, members: &[DeviceRecord]) -> TransferDto {
    let rec = members.iter().find(|m| m.id == t.peer);
    let (state, error) = match &t.state {
        TransferState::Preparing => ("preparing", None),
        TransferState::Waiting => ("waiting", None),
        TransferState::Asking => ("asking", None),
        TransferState::Active => ("active", None),
        TransferState::Done => ("done", None),
        TransferState::Failed(e) => ("failed", Some(e.clone())),
        TransferState::Cancelled => ("cancelled", None),
    };
    TransferDto {
        id: hex::encode(t.id),
        device_id: t.peer.to_string(),
        device: rec.map_or_else(|| "已移除的设备".to_owned(), |r| r.name.clone()),
        platform: rec.map_or("other", |r| platform_str(r.platform)),
        outgoing: t.outgoing,
        kind: match t.kind {
            TransferKind::Files => "files",
            TransferKind::Screenshot => "screenshot",
        },
        files: t
            .files
            .iter()
            .take(LIST_FILES)
            .map(|(name, size)| FileDto { name: name.clone(), size: *size })
            .collect(),
        count: t.files.len(),
        text: t.text.clone(),
        total: t.total,
        done: t.done,
        rate: t.rate,
        state,
        error,
        lan: t.lan,
        dir: t.dir.as_ref().map(|d| d.display().to_string()),
        saved: t.saved.iter().map(|p| p.display().to_string()).collect(),
        saved_dirs: t.saved.iter().map(|p| p.is_dir()).collect(),
        time: t.created_ms,
        finished: t.finished_ms,
    }
}

#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SendSummary {
    pub count: usize,
    pub total: u64,
}

pub fn collect_files(paths: &[PathBuf]) -> Res<(Vec<OutgoingFile>, u64)> {
    let mut out = Vec::new();
    let mut total = 0u64;
    for p in paths {
        let name = p
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .ok_or_else(|| format!("无法发送 {}", p.display()))?;
        walk(p, &name, &mut out, &mut total)?;
    }
    if out.is_empty() {
        return Err("没有可发送的文件（文件夹是空的）".into());
    }
    Ok((out, total))
}

fn walk(path: &Path, name: &str, out: &mut Vec<OutgoingFile>, total: &mut u64) -> Res<()> {
    let meta = std::fs::metadata(path).map_err(|e| format!("无法读取 {name}：{e}"))?;
    if meta.is_file() {
        if out.len() >= MAX_SEND_FILES {
            return Err(format!("一次最多发送 {MAX_SEND_FILES} 个文件"));
        }
        *total += meta.len();
        out.push(OutgoingFile { name: name.to_owned(), source: FileSource::Path(path.to_path_buf()) });
        return Ok(());
    }
    if !meta.is_dir() || std::fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_symlink()) {
        return Ok(());
    }
    let mut entries: Vec<_> =
        std::fs::read_dir(path).map_err(|e| format!("无法读取文件夹 {name}：{e}"))?.filter_map(|e| e.ok()).collect();
    entries.sort_by_key(|e| e.file_name());
    for e in entries {
        let child = format!("{name}/{}", e.file_name().to_string_lossy());
        walk(&e.path(), &child, out, total)?;
    }
    Ok(())
}

pub fn open_files(files: Vec<(String, File)>) -> Res<(Vec<OutgoingFile>, u64)> {
    if files.is_empty() || files.len() > MAX_SEND_FILES {
        return Err("文件数量不合适".into());
    }
    let mut total = 0;
    let out = files
        .into_iter()
        .map(|(name, f)| {
            let meta = f.metadata().map_err(|e| format!("无法读取 {name}：{e}"))?;
            if !meta.is_file() {
                return Err(format!("{name} 不是普通文件"));
            }
            total += meta.len();
            Ok(OutgoingFile { name, source: FileSource::Open(f) })
        })
        .collect::<Res<Vec<_>>>()?;
    Ok((out, total))
}

pub fn first_link(text: &str) -> Option<&str> {
    let lower = text.to_ascii_lowercase();
    let start = [lower.find("https://"), lower.find("http://")].into_iter().flatten().min()?;
    let rest = &text[start..];

    let end = rest
        .char_indices()
        .find(|(_, c)| c.is_whitespace() || "<>\"'`".contains(*c) || !c.is_ascii())
        .map_or(rest.len(), |(i, _)| i);
    let link = rest[..end].trim_end_matches(['.', ',', ';', ':', '!', '?', ')', ']', '}']);

    (link.len() > link.find("://")? + 3).then_some(link)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn links_in_shared_text() {
        assert_eq!(first_link("https://example.com/a?b=1"), Some("https://example.com/a?b=1"));
        assert_eq!(first_link("看看这个 https://example.com/xYz 不错"), Some("https://example.com/xYz"));
        assert_eq!(
            first_link("【标题】https://example.com/h.abc?tk=1」复制打开"),
            Some("https://example.com/h.abc?tk=1")
        );
        assert_eq!(first_link("链接：http://example.com/x，记得看"), Some("http://example.com/x"));
        assert_eq!(first_link("see (https://example.com/b)."), Some("https://example.com/b"));
        assert_eq!(first_link("HTTPS://EXAMPLE.COM"), Some("HTTPS://EXAMPLE.COM"));
        assert_eq!(first_link("没有链接 ftp://example.com"), None);
        assert_eq!(first_link("https:// 空的"), None);
        assert_eq!(first_link("javascript:alert(1)"), None);
    }

    #[test]
    fn collects_folders_recursively() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("相册");
        std::fs::create_dir_all(root.join("子")).unwrap();
        std::fs::write(root.join("a.jpg"), [1u8; 10]).unwrap();
        std::fs::write(root.join("子").join("b.jpg"), [2u8; 5]).unwrap();
        std::fs::write(dir.path().join("c.txt"), b"x").unwrap();
        std::fs::create_dir_all(dir.path().join("空")).unwrap();
        let (files, total) = collect_files(&[root, dir.path().join("c.txt"), dir.path().join("空")]).unwrap();
        let names: Vec<&str> = files.iter().map(|f| f.name.as_str()).collect();
        assert_eq!(names, ["相册/a.jpg", "相册/子/b.jpg", "c.txt"]);
        assert_eq!(total, 16);
        assert!(collect_files(&[dir.path().join("空")]).is_err());
        assert!(collect_files(&[dir.path().join("不存在")]).is_err());
    }
}
