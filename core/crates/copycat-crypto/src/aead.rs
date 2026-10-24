use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};

use crate::{CryptoError, Result, random};

pub const NONCE_LEN: usize = 24;
pub const TAG_LEN: usize = 16;

pub fn seal(key: &[u8; 32], aad: &[u8], plaintext: &[u8]) -> Result<Vec<u8>> {
    let cipher = XChaCha20Poly1305::new(key.into());
    let nonce_bytes = random::bytes::<NONCE_LEN>()?;
    let nonce = XNonce::from(nonce_bytes);
    let ct = cipher.encrypt(&nonce, Payload { msg: plaintext, aad }).map_err(|_| CryptoError::Decrypt)?;
    let mut out = Vec::with_capacity(NONCE_LEN + ct.len());
    out.extend_from_slice(&nonce_bytes);
    out.extend_from_slice(&ct);
    Ok(out)
}

pub fn open(key: &[u8; 32], aad: &[u8], data: &[u8]) -> Result<Vec<u8>> {
    if data.len() < NONCE_LEN + TAG_LEN {
        return Err(CryptoError::Decrypt);
    }
    let (nonce_bytes, ct) = data.split_at(NONCE_LEN);
    let nonce_arr: [u8; NONCE_LEN] = nonce_bytes.try_into().expect("长度已检查");
    let cipher = XChaCha20Poly1305::new(key.into());
    cipher.decrypt(&XNonce::from(nonce_arr), Payload { msg: ct, aad }).map_err(|_| CryptoError::Decrypt)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_and_tamper() {
        let key = [7u8; 32];
        let sealed = seal(&key, b"aad", b"secret").unwrap();
        assert_eq!(open(&key, b"aad", &sealed).unwrap(), b"secret");
        assert!(open(&key, b"other", &sealed).is_err());
        assert!(open(&[8u8; 32], b"aad", &sealed).is_err());
        for i in 0..sealed.len() {
            let mut t = sealed.clone();
            t[i] ^= 1;
            assert!(open(&key, b"aad", &t).is_err(), "篡改第 {i} 字节未被发现");
        }
        assert!(open(&key, b"aad", &sealed[..10]).is_err());
    }

    #[test]
    fn nonces_are_random() {
        let key = [1u8; 32];
        assert_ne!(seal(&key, b"", b"x").unwrap(), seal(&key, b"", b"x").unwrap());
    }
}
