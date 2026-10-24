use std::sync::mpsc;
use std::thread::JoinHandle;

use windows_sys::Win32::Foundation::{GetLastError, HWND, LPARAM, LRESULT, WPARAM};
use windows_sys::Win32::System::DataExchange::{AddClipboardFormatListener, RemoveClipboardFormatListener};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GWLP_USERDATA, GetMessageW, GetWindowLongPtrW,
    HWND_MESSAGE, IsWindow, MSG, PostMessageW, PostQuitMessage, RegisterClassExW, SetWindowLongPtrW, TranslateMessage,
    WM_CLOSE, WM_DESTROY, WNDCLASSEXW,
};

use crate::{Result, WinError};

const WM_CLIPBOARDUPDATE: u32 = 0x031D;

type Callback = Box<dyn Fn() + Send + 'static>;

pub struct Watcher {
    hwnd: usize,
    thread: Option<JoinHandle<()>>,
}

impl Watcher {
    pub fn spawn(on_change: impl Fn() + Send + 'static) -> Result<Self> {
        let (tx, rx) = mpsc::channel::<Result<usize>>();
        let thread = std::thread::Builder::new()
            .name("copycat-clipboard".into())
            .spawn(move || run(Box::new(on_change), tx))
            .map_err(|_| WinError::Api("spawn", 0))?;
        let hwnd = rx.recv().map_err(|_| WinError::Api("watcher", 0))??;
        Ok(Self { hwnd, thread: Some(thread) })
    }

    pub fn hwnd(&self) -> HWND {
        self.hwnd as HWND
    }
}

impl Drop for Watcher {
    fn drop(&mut self) {
        unsafe { PostMessageW(self.hwnd as HWND, WM_CLOSE, 0, 0) };
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

fn run(cb: Callback, ready: mpsc::Sender<Result<usize>>) {
    let class = wide("CopyCatClipboardWatcher");

    unsafe {
        let hinst = GetModuleHandleW(std::ptr::null());
        let wc = WNDCLASSEXW {
            cbSize: size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(wndproc),
            hInstance: hinst,
            lpszClassName: class.as_ptr(),
            ..std::mem::zeroed()
        };
        RegisterClassExW(&wc);
        let hwnd = CreateWindowExW(
            0,
            class.as_ptr(),
            class.as_ptr(),
            0,
            0,
            0,
            0,
            0,
            HWND_MESSAGE,
            std::ptr::null_mut(),
            hinst,
            std::ptr::null(),
        );
        if hwnd.is_null() {
            let _ = ready.send(Err(WinError::Api("CreateWindowExW", GetLastError())));
            return;
        }
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, Box::into_raw(Box::new(cb)) as isize);
        if AddClipboardFormatListener(hwnd) == 0 {
            let _ = ready.send(Err(WinError::Api("AddClipboardFormatListener", GetLastError())));
            DestroyWindow(hwnd);
            return;
        }
        let _ = ready.send(Ok(hwnd as usize));

        let mut msg: MSG = std::mem::zeroed();
        while GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) > 0 {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }

        if IsWindow(hwnd) != 0 {
            RemoveClipboardFormatListener(hwnd);
            DestroyWindow(hwnd);
        }
    }
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    unsafe {
        match msg {
            WM_CLIPBOARDUPDATE => {
                let p = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const Callback;
                if !p.is_null() {
                    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| (*p)()));
                }
                0
            }
            WM_CLOSE => {
                RemoveClipboardFormatListener(hwnd);
                DestroyWindow(hwnd);
                0
            }
            WM_DESTROY => {
                let p = SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0) as *mut Callback;
                if !p.is_null() {
                    drop(Box::from_raw(p));
                }
                PostQuitMessage(0);
                0
            }
            _ => DefWindowProcW(hwnd, msg, wparam, lparam),
        }
    }
}
