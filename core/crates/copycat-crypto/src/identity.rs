use core::fmt;

use data_encoding::BASE32_NOPAD;
use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};
use snow::params::DHChoice;
use snow::resolvers::{CryptoResolver, DefaultResolver};
use zeroize::Zeroizing;

use crate::{CryptoError, Result, kdf, random};

pub const DEVICE_ID_LEN: usize = copycat_proto::DEVICE_ID_LEN;

pub const SECRET_LEN: usize = 64;

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DeviceId(pub [u8; DEVICE_ID_LEN]);

impl DeviceId {
    pub fn from_sign_pub(sign_pub: &[u8; 32]) -> Self {
        let h = kdf::hash(sign_pub);
        let mut id = [0u8; DEVICE_ID_LEN];
        id.copy_from_slice(&h[..DEVICE_ID_LEN]);
        Self(id)
    }

    pub fn from_slice(b: &[u8]) -> Result<Self> {
        let arr: [u8; DEVICE_ID_LEN] = b.try_into().map_err(|_| CryptoError::Malformed("设备 ID 长度错误"))?;
        Ok(Self(arr))
    }

    pub fn as_bytes(&self) -> &[u8; DEVICE_ID_LEN] {
        &self.0
    }

    pub fn short(&self) -> String {
        self.to_string()[..8].to_owned()
    }
}

impl fmt::Display for DeviceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&BASE32_NOPAD.encode(&self.0).to_ascii_lowercase())
    }
}

impl fmt::Debug for DeviceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "DeviceId({})", self.short())
    }
}

pub struct DeviceIdentity {
    sign: SigningKey,
    dh_private: Zeroizing<[u8; 32]>,
    dh_public: [u8; 32],
    id: DeviceId,
}

impl DeviceIdentity {
    pub fn generate() -> Result<Self> {
        let mut secret = Zeroizing::new([0u8; SECRET_LEN]);
        random::fill(&mut secret[..])?;
        Self::from_secret(&secret)
    }

    pub fn from_secret(secret: &[u8; SECRET_LEN]) -> Result<Self> {
        let mut seed = Zeroizing::new([0u8; 32]);
        seed.copy_from_slice(&secret[..32]);
        let sign = SigningKey::from_bytes(&seed);
        let mut dh_private = Zeroizing::new([0u8; 32]);
        dh_private.copy_from_slice(&secret[32..]);
        let dh_public = x25519_public(&dh_private)?;
        let id = DeviceId::from_sign_pub(&sign.verifying_key().to_bytes());
        Ok(Self { sign, dh_private, dh_public, id })
    }

    pub fn export_secret(&self) -> Zeroizing<[u8; SECRET_LEN]> {
        let mut out = Zeroizing::new([0u8; SECRET_LEN]);
        out[..32].copy_from_slice(&self.sign.to_bytes());
        out[32..].copy_from_slice(&self.dh_private[..]);
        out
    }

    pub fn id(&self) -> DeviceId {
        self.id
    }

    pub fn sign_pub(&self) -> [u8; 32] {
        self.sign.verifying_key().to_bytes()
    }

    pub fn dh_public(&self) -> [u8; 32] {
        self.dh_public
    }

    pub(crate) fn dh_private(&self) -> &[u8; 32] {
        &self.dh_private
    }

    pub fn sign(&self, context: &str, data: &[u8]) -> [u8; 64] {
        self.sign.sign(&kdf::signing_message(context, data)).to_bytes()
    }
}

impl fmt::Debug for DeviceIdentity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DeviceIdentity").field("id", &self.id).finish_non_exhaustive()
    }
}

pub fn verify(sign_pub: &[u8; 32], context: &str, data: &[u8], sig: &[u8]) -> Result<()> {
    let key = VerifyingKey::from_bytes(sign_pub).map_err(|_| CryptoError::BadKey)?;
    let sig_arr: [u8; 64] = sig.try_into().map_err(|_| CryptoError::BadSignature)?;
    let sig = Signature::from_bytes(&sig_arr);
    key.verify_strict(&kdf::signing_message(context, data), &sig).map_err(|_| CryptoError::BadSignature)
}

fn x25519_public(private: &[u8; 32]) -> Result<[u8; 32]> {
    let mut dh =
        DefaultResolver.resolve_dh(&DHChoice::Curve25519).ok_or(CryptoError::Noise("缺少 Curve25519 实现".into()))?;
    dh.set(private);
    dh.pubkey().try_into().map_err(|_| CryptoError::BadKey)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn export_restore_roundtrip() {
        let a = DeviceIdentity::generate().unwrap();
        let b = DeviceIdentity::from_secret(&a.export_secret()).unwrap();
        assert_eq!(a.id(), b.id());
        assert_eq!(a.sign_pub(), b.sign_pub());
        assert_eq!(a.dh_public(), b.dh_public());
    }

    #[test]
    fn sign_verify_with_context() {
        let a = DeviceIdentity::generate().unwrap();
        let sig = a.sign("ctx", b"data");
        verify(&a.sign_pub(), "ctx", b"data", &sig).unwrap();
        assert_eq!(verify(&a.sign_pub(), "other", b"data", &sig), Err(CryptoError::BadSignature));
        assert_eq!(verify(&a.sign_pub(), "ctx", b"datb", &sig), Err(CryptoError::BadSignature));
        let b = DeviceIdentity::generate().unwrap();
        assert_eq!(verify(&b.sign_pub(), "ctx", b"data", &sig), Err(CryptoError::BadSignature));
    }

    #[test]
    fn device_id_display() {
        let a = DeviceIdentity::generate().unwrap();
        let s = a.id().to_string();
        assert_eq!(s.len(), 26);
        assert_eq!(a.id().short().len(), 8);
        assert_eq!(DeviceId::from_slice(a.id().as_bytes()).unwrap(), a.id());
        assert!(DeviceId::from_slice(&[0u8; 3]).is_err());
    }
}
