use crate::{CryptoError, Result};

pub fn fill(buf: &mut [u8]) -> Result<()> {
    getrandom::fill(buf).map_err(|_| CryptoError::Rng)
}

pub fn bytes<const N: usize>() -> Result<[u8; N]> {
    let mut out = [0u8; N];
    fill(&mut out)?;
    Ok(out)
}

pub fn u64() -> Result<u64> {
    Ok(u64::from_le_bytes(bytes::<8>()?))
}
