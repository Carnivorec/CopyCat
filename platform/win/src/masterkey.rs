use std::path::Path;

use crate::{Result, WinError, dpapi};

const ENTROPY: &[u8] = b"copycat master key v1";

pub fn load_or_create(path: &Path, generate: impl FnOnce() -> [u8; 32]) -> Result<[u8; 32]> {
    if path.exists() {
        let sealed = std::fs::read(path).map_err(|_| WinError::Api("读取主密钥文件", 0))?;
        let raw = dpapi::unprotect(&sealed, ENTROPY)?;
        return raw.as_slice().try_into().map_err(|_| WinError::Api("主密钥文件损坏", 0));
    }
    let key = generate();
    let sealed = dpapi::protect(&key, ENTROPY)?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|_| WinError::Api("创建数据目录", 0))?;
    }
    std::fs::write(path, sealed).map_err(|_| WinError::Api("写入主密钥文件", 0))?;
    Ok(key)
}
