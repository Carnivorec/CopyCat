pub mod content;

#[cfg(windows)]
pub mod clipboard;
#[cfg(windows)]
pub mod dpapi;
#[cfg(windows)]
pub mod explorer;
#[cfg(windows)]
pub mod masterkey;
#[cfg(windows)]
pub mod time;
#[cfg(windows)]
pub mod watcher;

#[derive(Debug, thiserror::Error)]
pub enum WinError {
    #[error("剪贴板被其他程序占用")]
    Busy,
    #[error("Win32 调用失败: {0}（错误码 {1}）")]
    Api(&'static str, u32),
    #[error("内容过大")]
    TooLarge,
    #[error("没有可以写入剪贴板的内容格式")]
    Unsupported,
}

pub type Result<T, E = WinError> = core::result::Result<T, E>;
