use std::path::Path;

use anyhow::{Context, Result};

#[cfg(windows)]
pub fn load_or_create(path: &Path) -> Result<[u8; 32]> {
    copycat_win::masterkey::load_or_create(path, || copycat_crypto::random::bytes::<32>().expect("系统随机数源可用"))
        .context("无法读取或创建主密钥（是否换了 Windows 用户？）")
}

#[cfg(not(windows))]
pub fn load_or_create(path: &Path) -> Result<[u8; 32]> {
    if path.exists() {
        let raw = std::fs::read(path)?;
        return raw.as_slice().try_into().context("主密钥文件损坏");
    }
    let key = copycat_crypto::random::bytes::<32>()?;
    std::fs::write(path, key)?;
    Ok(key)
}
