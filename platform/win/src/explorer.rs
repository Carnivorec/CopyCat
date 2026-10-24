use std::path::Path;

use windows::Win32::Foundation::HWND;
use windows::Win32::System::Com::{
    CLSCTX_ALL, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx, CoUninitialize, IDispatch, IServiceProvider,
};
use windows::Win32::System::Variant::VARIANT;
use windows::Win32::UI::Shell::{
    IShellBrowser, IShellFolderViewDual, IShellWindows, IWebBrowser2, SID_STopLevelBrowser, SVGIO_BACKGROUND,
    SWC_DESKTOP, SWFO_NEEDDISPATCH, ShellWindows,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GA_ROOT, GetAncestor, GetClassNameW, GetForegroundWindow, IsWindowVisible,
};
use windows::core::Interface;

pub fn foreground() -> usize {
    unsafe { GetForegroundWindow().0 as usize }
}

fn class_of(hwnd: HWND) -> String {
    let mut buf = [0u16; 64];

    let n = unsafe { GetClassNameW(hwnd, &mut buf) };
    String::from_utf16_lossy(&buf[..n.max(0) as usize])
}

pub fn selected_paths(foreground: usize) -> Vec<String> {
    std::thread::spawn(move || unsafe {
        if CoInitializeEx(None, COINIT_APARTMENTTHREADED).is_err() {
            return Vec::new();
        }
        let r = query(HWND(foreground as *mut _)).unwrap_or_default();
        CoUninitialize();
        r
    })
    .join()
    .unwrap_or_default()
}

unsafe fn query(fg: HWND) -> windows::core::Result<Vec<String>> {
    unsafe {
        let root = GetAncestor(fg, GA_ROOT);
        let class = class_of(root);
        let windows: IShellWindows = CoCreateInstance(&ShellWindows, None, CLSCTX_ALL)?;
        let browser = match class.as_str() {
            "CabinetWClass" | "ExploreWClass" => explorer_browser(&windows, root)?,
            "Progman" | "WorkerW" => desktop_browser(&windows)?,
            _ => return Ok(Vec::new()),
        };
        let Some(browser) = browser else { return Ok(Vec::new()) };
        let view = browser.QueryActiveShellView()?;
        let disp: IDispatch = view.GetItemObject(SVGIO_BACKGROUND)?;
        let folder: IShellFolderViewDual = disp.cast()?;
        let items = folder.SelectedItems()?;
        let mut out = Vec::new();
        for i in 0..items.Count()?.min(10_000) {
            let Ok(item) = items.Item(&VARIANT::from(i)) else { continue };
            let Ok(path) = item.Path() else { continue };
            let path = path.to_string();
            if !path.is_empty() && Path::new(&path).exists() {
                out.push(path);
            }
        }
        Ok(out)
    }
}

unsafe fn explorer_browser(windows: &IShellWindows, root: HWND) -> windows::core::Result<Option<IShellBrowser>> {
    unsafe {
        for i in 0..windows.Count()? {
            let Ok(disp) = windows.Item(&VARIANT::from(i)) else { continue };
            let Ok(web) = disp.cast::<IWebBrowser2>() else { continue };
            if web.HWND().map(|h| h.0 as usize) != Ok(root.0 as usize) {
                continue;
            }
            let Ok(sp) = web.cast::<IServiceProvider>() else { continue };
            let Ok(browser) = sp.QueryService::<IShellBrowser>(&SID_STopLevelBrowser) else { continue };

            if browser.GetWindow().is_ok_and(|tab| IsWindowVisible(tab).as_bool()) {
                return Ok(Some(browser));
            }
        }
        Ok(None)
    }
}

unsafe fn desktop_browser(windows: &IShellWindows) -> windows::core::Result<Option<IShellBrowser>> {
    unsafe {
        let mut hwnd = 0i32;
        let disp = windows.FindWindowSW(
            &VARIANT::from(0i32),
            &VARIANT::default(),
            SWC_DESKTOP,
            &mut hwnd,
            SWFO_NEEDDISPATCH,
        )?;
        let sp: IServiceProvider = disp.cast()?;
        Ok(Some(sp.QueryService::<IShellBrowser>(&SID_STopLevelBrowser)?))
    }
}
