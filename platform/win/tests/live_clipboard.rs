#![cfg(windows)]

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use copycat_win::clipboard;
use copycat_win::content::{self, MIME_HTML, MIME_PNG, MIME_TEXT};
use copycat_win::watcher::Watcher;

fn wait_until(mut f: impl FnMut() -> bool) -> bool {
    for _ in 0..100 {
        if f() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    false
}

#[test]
#[ignore]
fn write_read_roundtrip_and_watch() {
    let hits = Arc::new(AtomicUsize::new(0));
    let h = hits.clone();
    let w = Watcher::spawn(move || {
        h.fetch_add(1, Ordering::SeqCst);
    })
    .unwrap();

    let original = clipboard::read(w.hwnd()).unwrap();

    let rgba: Vec<u8> = (0..16u32 * 9).flat_map(|i| [i as u8, 255 - i as u8, 7, 255]).collect();
    let png = content::rgba_to_png(16, 9, &rgba).unwrap();
    let reps = vec![
        (MIME_TEXT.to_string(), "CopyCat 测试\n第二行 🐱".as_bytes().to_vec()),
        (MIME_HTML.to_string(), "<b>CopyCat</b> 测试".as_bytes().to_vec()),
        (MIME_PNG.to_string(), png.clone()),
    ];
    let before = hits.load(Ordering::SeqCst);
    clipboard::write(w.hwnd(), &reps, b"item-id-1234").unwrap();
    assert!(wait_until(|| hits.load(Ordering::SeqCst) > before), "写入后应收到剪贴板变化事件");

    let got = clipboard::read(w.hwnd()).unwrap();
    assert!(got.ours, "应识别出自己写入的标记");
    assert!(!got.sensitive);
    assert_eq!(got.reps[0], reps[0], "纯文本必须逐字节一致（含换行与表情）");
    assert_eq!(got.reps[1], reps[1]);
    assert_eq!(got.reps[2].0, MIME_PNG);
    assert_eq!(got.reps[2].1, png, "PNG 应原样读回");

    if !original.sensitive && !original.reps.is_empty() {
        clipboard::write(w.hwnd(), &original.reps, b"restore").unwrap();
    }
}

#[test]
#[ignore]
fn clipboard_is_never_left_open() {
    let w = Watcher::spawn(|| {}).unwrap();
    let original = clipboard::read(w.hwnd()).unwrap();

    for i in 0..50 {
        let reps = vec![(MIME_TEXT.to_string(), format!("循环 {i}").into_bytes())];
        clipboard::write(w.hwnd(), &reps, b"x").unwrap();
        let _ = clipboard::read(w.hwnd()).unwrap();

        assert!(clipboard::write(w.hwnd(), &[(MIME_PNG.to_string(), b"not a png".to_vec())], b"x").is_err());
    }
    assert!(clipboard::read(std::ptr::null_mut()).is_ok(), "剪贴板必须可被再次打开");
    if !original.sensitive && !original.reps.is_empty() {
        clipboard::write(w.hwnd(), &original.reps, b"restore").unwrap();
    }
}
