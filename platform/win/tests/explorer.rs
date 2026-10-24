#![cfg(windows)]

use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::HWND;

#[test]
#[ignore = "会弹出资源管理器窗口，手动运行"]
fn reads_selection_of_foreground_explorer() {
    let dir = std::env::temp_dir().join(format!("copycat-explorer-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("选中的文件.txt");
    std::fs::write(&file, b"x").unwrap();

    let _ = std::process::Command::new("explorer").arg(format!("/select,{}", file.display())).status();

    let start = Instant::now();
    let mut got = Vec::new();
    let mut fg: HWND = std::ptr::null_mut();
    while start.elapsed() < Duration::from_secs(10) {
        std::thread::sleep(Duration::from_millis(300));

        let Some(h) = test_window() else { continue };
        fg = h as HWND;
        got = copycat_win::explorer::selected_paths(fg as usize);
        if !got.is_empty() {
            break;
        }
    }
    let _ = fg;
    close_test_windows();
    std::fs::remove_file(&file).unwrap();
    std::fs::remove_dir(&dir).unwrap();

    assert_eq!(got.len(), 1, "{got:?}");
    assert!(got[0].ends_with("选中的文件.txt"), "{got:?}");
}

fn close_test_windows() {
    let script = "$sh = New-Object -ComObject Shell.Application; $sh.Windows() | Where-Object { $_.LocationURL -like '*copycat-explorer*' } | ForEach-Object { $_.Quit() }";
    let _ = std::process::Command::new("powershell").args(["-NoProfile", "-Command", script]).status();
}

fn test_window() -> Option<usize> {
    let script = "$sh = New-Object -ComObject Shell.Application; $sh.Windows() | Where-Object { $_.LocationURL -like '*copycat-explorer*' } | ForEach-Object { $_.HWND }";
    let out = std::process::Command::new("powershell").args(["-NoProfile", "-Command", script]).output().ok()?;
    String::from_utf8_lossy(&out.stdout).lines().find_map(|l| l.trim().parse::<i64>().ok()).map(|h| h as usize)
}
