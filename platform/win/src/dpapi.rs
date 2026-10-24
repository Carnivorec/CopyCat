use windows_sys::Win32::Foundation::{GetLastError, LocalFree};
use windows_sys::Win32::Security::Cryptography::{
    CRYPT_INTEGER_BLOB, CRYPTPROTECT_UI_FORBIDDEN, CryptProtectData, CryptUnprotectData,
};

use crate::{Result, WinError};

fn blob(data: &[u8]) -> CRYPT_INTEGER_BLOB {
    CRYPT_INTEGER_BLOB { cbData: data.len() as u32, pbData: data.as_ptr() as *mut u8 }
}

fn take(out: CRYPT_INTEGER_BLOB) -> Vec<u8> {
    unsafe {
        let v = std::slice::from_raw_parts(out.pbData, out.cbData as usize).to_vec();
        std::ptr::write_bytes(out.pbData, 0, out.cbData as usize);
        LocalFree(out.pbData as _);
        v
    }
}

pub fn protect(data: &[u8], entropy: &[u8]) -> Result<Vec<u8>> {
    let (input, ent) = (blob(data), blob(entropy));
    let mut out = CRYPT_INTEGER_BLOB { cbData: 0, pbData: std::ptr::null_mut() };

    let ok = unsafe {
        CryptProtectData(
            &input,
            std::ptr::null(),
            &ent,
            std::ptr::null(),
            std::ptr::null(),
            CRYPTPROTECT_UI_FORBIDDEN,
            &mut out,
        )
    };
    if ok == 0 {
        return Err(WinError::Api("CryptProtectData", unsafe { GetLastError() }));
    }
    Ok(take(out))
}

pub fn unprotect(data: &[u8], entropy: &[u8]) -> Result<Vec<u8>> {
    let (input, ent) = (blob(data), blob(entropy));
    let mut out = CRYPT_INTEGER_BLOB { cbData: 0, pbData: std::ptr::null_mut() };

    let ok = unsafe {
        CryptUnprotectData(
            &input,
            std::ptr::null_mut(),
            &ent,
            std::ptr::null(),
            std::ptr::null(),
            CRYPTPROTECT_UI_FORBIDDEN,
            &mut out,
        )
    };
    if ok == 0 {
        return Err(WinError::Api("CryptUnprotectData", unsafe { GetLastError() }));
    }
    Ok(take(out))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        let ct = protect(b"master key", b"copycat").unwrap();
        assert_ne!(ct, b"master key");
        assert_eq!(unprotect(&ct, b"copycat").unwrap(), b"master key");
        assert!(unprotect(&ct, b"other").is_err(), "熵不同必须解密失败");
    }
}
