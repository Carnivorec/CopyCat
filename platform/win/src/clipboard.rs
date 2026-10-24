use std::ptr::null_mut;
use std::sync::OnceLock;
use std::time::Duration;

use windows_sys::Win32::Foundation::{CloseHandle, GetLastError, GlobalFree, HANDLE, HGLOBAL, HWND};
use windows_sys::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, EnumClipboardFormats, GetClipboardData, GetClipboardOwner,
    GetClipboardSequenceNumber, OpenClipboard, RegisterClipboardFormatW, SetClipboardData,
};
use windows_sys::Win32::System::Memory::{GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock};
use windows_sys::Win32::System::Ole::{CF_DIB, CF_DIBV5, CF_HDROP, CF_UNICODETEXT};
use windows_sys::Win32::System::Threading::{
    OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
};
use windows_sys::Win32::UI::Shell::{DragQueryFileW, HDROP};
use windows_sys::Win32::UI::WindowsAndMessaging::GetWindowThreadProcessId;

use crate::content::{self, MIME_FILES, MIME_HTML, MIME_PNG, MIME_TEXT, Snapshot};
use crate::{Result, WinError};

const MAX_FORMAT_BYTES: usize = 256 * 1024 * 1024;
const OPEN_RETRY: [u64; 6] = [0, 10, 20, 40, 80, 160];

struct Formats {
    html: u32,
    png: u32,
    exclude: u32,
    history: u32,
    cloud: u32,
    viewer_ignore: u32,
    marker: u32,
    drop_effect: u32,
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

fn register(name: &str) -> u32 {
    let w = wide(name);

    unsafe { RegisterClipboardFormatW(w.as_ptr()) }
}

fn formats() -> &'static Formats {
    static F: OnceLock<Formats> = OnceLock::new();
    F.get_or_init(|| Formats {
        html: register("HTML Format"),
        png: register("PNG"),
        exclude: register("ExcludeClipboardContentFromMonitorProcessing"),
        history: register("CanIncludeInClipboardHistory"),
        cloud: register("CanUploadToCloudClipboard"),
        viewer_ignore: register("Clipboard Viewer Ignore"),
        marker: register("CopyCat.Origin"),
        drop_effect: register("Preferred DropEffect"),
    })
}

struct OpenGuard;

impl OpenGuard {
    fn open(owner: HWND) -> Result<Self> {
        for delay in OPEN_RETRY {
            if delay > 0 {
                std::thread::sleep(Duration::from_millis(delay));
            }

            if unsafe { OpenClipboard(owner) } != 0 {
                return Ok(Self);
            }
        }
        Err(WinError::Busy)
    }
}

impl Drop for OpenGuard {
    fn drop(&mut self) {
        unsafe { CloseClipboard() };
    }
}

struct OwnedGlobal(HGLOBAL);

impl OwnedGlobal {
    fn from_bytes(data: &[u8]) -> Result<Self> {
        unsafe {
            let h = GlobalAlloc(GMEM_MOVEABLE, data.len().max(1));
            if h.is_null() {
                return Err(WinError::Api("GlobalAlloc", GetLastError()));
            }
            let owned = Self(h);
            let p = GlobalLock(h) as *mut u8;
            if p.is_null() {
                return Err(WinError::Api("GlobalLock", GetLastError()));
            }
            std::ptr::copy_nonoverlapping(data.as_ptr(), p, data.len());
            GlobalUnlock(h);
            Ok(owned)
        }
    }

    fn release(mut self) {
        self.0 = null_mut();
    }
}

impl Drop for OwnedGlobal {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe { GlobalFree(self.0) };
        }
    }
}

fn read_format(fmt: u32) -> Option<Vec<u8>> {
    unsafe {
        let h: HANDLE = GetClipboardData(fmt);
        if h.is_null() {
            return None;
        }
        let size = GlobalSize(h);
        if size == 0 || size > MAX_FORMAT_BYTES {
            return None;
        }
        let p = GlobalLock(h) as *const u8;
        if p.is_null() {
            return None;
        }
        let v = std::slice::from_raw_parts(p, size).to_vec();
        GlobalUnlock(h);
        Some(v)
    }
}

fn read_dword(fmt: u32) -> Option<u32> {
    read_format(fmt).and_then(|b| Some(u32::from_le_bytes(b.get(..4)?.try_into().ok()?)))
}

fn utf16_bytes_to_string(b: &[u8]) -> String {
    let units: Vec<u16> =
        b.as_chunks::<2>().0.iter().map(|c| u16::from_le_bytes([c[0], c[1]])).take_while(|&u| u != 0).collect();
    String::from_utf16_lossy(&units)
}

fn read_files() -> Vec<String> {
    unsafe {
        let h = GetClipboardData(u32::from(CF_HDROP));
        if h.is_null() {
            return Vec::new();
        }
        let hdrop = h as HDROP;
        let count = DragQueryFileW(hdrop, u32::MAX, null_mut(), 0).min(10_000);
        let mut out = Vec::new();
        for i in 0..count {
            let len = DragQueryFileW(hdrop, i, null_mut(), 0);
            let mut buf = vec![0u16; len as usize + 1];
            let n = DragQueryFileW(hdrop, i, buf.as_mut_ptr(), buf.len() as u32);
            out.push(String::from_utf16_lossy(&buf[..n as usize]));
        }
        out
    }
}

pub fn sequence() -> u32 {
    unsafe { GetClipboardSequenceNumber() }
}

fn is_sensitive(exclude: bool, viewer_ignore: bool, history: Option<u32>, cloud: Option<u32>, marker: bool) -> bool {
    exclude || viewer_ignore || history == Some(0) || (cloud == Some(0) && !marker)
}

fn flag_formats(marker: &[u8]) -> Vec<(u32, Vec<u8>)> {
    if marker.is_empty() {
        return Vec::new();
    }
    let f = formats();
    vec![(f.marker, marker.to_vec()), (f.cloud, 0u32.to_le_bytes().to_vec())]
}

pub fn owner_exe() -> Option<String> {
    unsafe {
        let hwnd = GetClipboardOwner();
        if hwnd.is_null() {
            return None;
        }
        let mut pid = 0u32;
        GetWindowThreadProcessId(hwnd, &mut pid);
        if pid == 0 {
            return None;
        }
        let proc = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if proc.is_null() {
            return None;
        }
        let mut buf = [0u16; 1024];
        let mut len = buf.len() as u32;
        let ok = QueryFullProcessImageNameW(proc, PROCESS_NAME_WIN32, buf.as_mut_ptr(), &mut len);
        CloseHandle(proc);
        (ok != 0).then(|| String::from_utf16_lossy(&buf[..len as usize]))
    }
}

pub fn read(owner: HWND) -> Result<Snapshot> {
    let f = formats();
    let owner_exe = owner_exe();

    let (mut snap, text, html_raw, png_raw, dib_raw) = {
        let _g = OpenGuard::open(owner)?;
        let mut present = Vec::new();
        let mut fmt = 0u32;
        loop {
            fmt = unsafe { EnumClipboardFormats(fmt) };
            if fmt == 0 || present.len() > 512 {
                break;
            }
            present.push(fmt);
        }
        let has = |x: u32| present.contains(&x);
        let sensitive = is_sensitive(
            has(f.exclude),
            has(f.viewer_ignore),
            has(f.history).then(|| read_dword(f.history)).flatten(),
            has(f.cloud).then(|| read_dword(f.cloud)).flatten(),
            has(f.marker),
        );
        let mut snap = Snapshot { sensitive, ours: has(f.marker), owner_exe, ..Snapshot::default() };
        if sensitive {
            return Ok(snap);
        }
        let text = has(u32::from(CF_UNICODETEXT)).then(|| read_format(u32::from(CF_UNICODETEXT))).flatten();
        let html_raw = has(f.html).then(|| read_format(f.html)).flatten();
        let png_raw = has(f.png).then(|| read_format(f.png)).flatten();
        let dib_raw = if png_raw.is_none() {
            [u32::from(CF_DIBV5), u32::from(CF_DIB)].into_iter().find(|x| has(*x)).and_then(read_format)
        } else {
            None
        };
        if has(u32::from(CF_HDROP)) {
            snap.files = read_files();
        }
        (snap, text, html_raw, png_raw, dib_raw)
    };

    if let Some(t) = text {
        let s = content::normalize_newlines(&utf16_bytes_to_string(&t));
        if !s.is_empty() {
            snap.reps.push((MIME_TEXT.into(), s.into_bytes()));
        }
    }
    if let Some(h) = html_raw.as_deref().and_then(content::cf_html_extract) {
        snap.reps.push((MIME_HTML.into(), h.into_bytes()));
    }
    let png = match png_raw {
        Some(p) if content::is_png(&p) => Some(p),
        _ => dib_raw.as_deref().and_then(content::dib_to_png),
    };
    if let Some(p) = png {
        snap.reps.push((MIME_PNG.into(), p));
    }
    if !snap.files.is_empty() {
        snap.reps.clear();
    }
    Ok(snap)
}

pub fn write(owner: HWND, reps: &[(String, Vec<u8>)], marker: &[u8]) -> Result<()> {
    let f = formats();

    let mut blocks: Vec<(u32, OwnedGlobal)> = Vec::new();
    for (mime, data) in reps {
        match mime.as_str() {
            MIME_TEXT => {
                let s = String::from_utf8_lossy(data);
                let mut w: Vec<u8> = content::to_crlf(&s).encode_utf16().flat_map(u16::to_le_bytes).collect();
                w.extend_from_slice(&[0, 0]);
                blocks.push((u32::from(CF_UNICODETEXT), OwnedGlobal::from_bytes(&w)?));
            }
            MIME_HTML => {
                let s = String::from_utf8_lossy(data);
                blocks.push((f.html, OwnedGlobal::from_bytes(&content::cf_html_build(&s))?));
            }
            MIME_PNG if content::is_png(data) => {
                blocks.push((f.png, OwnedGlobal::from_bytes(data)?));
                if let Some(dib) = content::png_to_dib(data) {
                    blocks.push((u32::from(CF_DIB), OwnedGlobal::from_bytes(&dib)?));
                }
            }
            MIME_FILES => {
                let s = String::from_utf8_lossy(data);
                let paths: Vec<&str> = s.lines().filter(|l| !l.is_empty()).collect();
                if !paths.is_empty() {
                    blocks.push((u32::from(CF_HDROP), OwnedGlobal::from_bytes(&content::dropfiles_build(&paths))?));

                    blocks.push((f.drop_effect, OwnedGlobal::from_bytes(&1u32.to_le_bytes())?));
                }
            }
            _ => {}
        }
    }
    if blocks.is_empty() {
        return Err(WinError::Unsupported);
    }
    for (fmt, data) in flag_formats(marker) {
        blocks.push((fmt, OwnedGlobal::from_bytes(&data)?));
    }

    let _g = OpenGuard::open(owner)?;

    if unsafe { EmptyClipboard() } == 0 {
        return Err(WinError::Api("EmptyClipboard", unsafe { GetLastError() }));
    }
    let mut failed = None;
    for (fmt, block) in blocks {
        if unsafe { SetClipboardData(fmt, block.0) }.is_null() {
            failed.get_or_insert(unsafe { GetLastError() });
        } else {
            block.release();
        }
    }
    match failed {
        Some(code) => Err(WinError::Api("SetClipboardData", code)),
        None => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reads_back_as_sensitive(marker: &[u8]) -> bool {
        let f = formats();
        let flags = flag_formats(marker);
        let get = |fmt: u32| {
            flags.iter().find(|(x, _)| *x == fmt).map(|(_, d)| u32::from_le_bytes(d[..4].try_into().unwrap()))
        };
        is_sensitive(false, false, None, get(f.cloud), flags.iter().any(|(x, _)| *x == f.marker))
    }

    #[test]
    fn manual_copy_is_not_filtered_as_sensitive() {
        assert!(flag_formats(&[]).is_empty());
        assert!(!reads_back_as_sensitive(&[]));

        assert!(!reads_back_as_sensitive(&[1; 16]));
    }

    #[test]
    fn sensitive_sources_are_detected() {
        assert!(is_sensitive(true, false, None, None, false));
        assert!(is_sensitive(false, true, None, None, false));
        assert!(is_sensitive(false, false, Some(0), None, false));
        assert!(is_sensitive(false, false, None, Some(0), false));
        assert!(!is_sensitive(false, false, Some(1), Some(1), false));
        assert!(!is_sensitive(false, false, None, None, false));
    }
}
