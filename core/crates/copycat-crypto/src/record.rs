use prost::Message;

use crate::identity::{self, DeviceId, DeviceIdentity};
use crate::kdf::sig_context;
use crate::wire::{RecordBody, SignedRecord};
use crate::{CryptoError, Result};

pub const MAX_NAME_LEN: usize = 64;
const MAX_RECORD_LEN: usize = 512;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Platform {
    Windows,
    Android,
    Other(u32),
}

impl Platform {
    fn to_u32(self) -> u32 {
        match self {
            Platform::Windows => 1,
            Platform::Android => 2,
            Platform::Other(v) => v,
        }
    }

    fn from_u32(v: u32) -> Self {
        match v {
            1 => Platform::Windows,
            2 => Platform::Android,
            v => Platform::Other(v),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeviceRecord {
    pub id: DeviceId,
    pub sign_pub: [u8; 32],
    pub dh_pub: [u8; 32],
    pub name: String,
    pub platform: Platform,
    pub created_at_ms: u64,

    raw: Vec<u8>,
}

impl DeviceRecord {
    pub fn issue(identity: &DeviceIdentity, name: &str, platform: Platform, created_at_ms: u64) -> Result<Self> {
        let name = sanitize_name(name);
        let body = RecordBody {
            sign_pub: identity.sign_pub().to_vec(),
            dh_pub: identity.dh_public().to_vec(),
            name,
            platform: platform.to_u32(),
            created_at_ms,
        }
        .encode_to_vec();
        let sig = identity.sign(sig_context::DEVICE_RECORD, &body);
        let raw = SignedRecord { body, sig: sig.to_vec() }.encode_to_vec();
        Self::verify(&raw)
    }

    pub fn verify(raw: &[u8]) -> Result<Self> {
        if raw.len() > MAX_RECORD_LEN {
            return Err(CryptoError::TooLarge);
        }
        let signed = SignedRecord::decode(raw)?;
        let body = RecordBody::decode(signed.body.as_slice())?;
        let sign_pub: [u8; 32] = body.sign_pub.as_slice().try_into().map_err(|_| CryptoError::BadKey)?;
        let dh_pub: [u8; 32] = body.dh_pub.as_slice().try_into().map_err(|_| CryptoError::BadKey)?;

        if body.name != sanitize_name(&body.name) {
            return Err(CryptoError::Malformed("设备名含非法字符或过长"));
        }
        identity::verify(&sign_pub, sig_context::DEVICE_RECORD, &signed.body, &signed.sig)?;
        Ok(Self {
            id: DeviceId::from_sign_pub(&sign_pub),
            sign_pub,
            dh_pub,
            name: body.name,
            platform: Platform::from_u32(body.platform),
            created_at_ms: body.created_at_ms,
            raw: raw.to_vec(),
        })
    }

    pub fn raw(&self) -> &[u8] {
        &self.raw
    }
}

fn sanitize_name(name: &str) -> String {
    name.chars()
        .filter(|c| !c.is_control() && !is_bidi_control(*c))
        .take(MAX_NAME_LEN)
        .collect::<String>()
        .trim()
        .to_owned()
}

fn is_bidi_control(c: char) -> bool {
    matches!(c, '\u{200E}' | '\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}' | '\u{061C}')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn issue_and_verify() {
        let id = DeviceIdentity::generate().unwrap();
        let r = DeviceRecord::issue(&id, "主人的台式机", Platform::Windows, 1).unwrap();
        assert_eq!(r.id, id.id());
        assert_eq!(r.dh_pub, id.dh_public());
        let again = DeviceRecord::verify(r.raw()).unwrap();
        assert_eq!(again, r);
    }

    #[test]
    fn tampered_record_rejected() {
        let id = DeviceIdentity::generate().unwrap();
        let r = DeviceRecord::issue(&id, "pc", Platform::Windows, 1).unwrap();
        let mut raw = r.raw().to_vec();

        for i in 0..raw.len() {
            raw[i] ^= 0x40;
            assert!(DeviceRecord::verify(&raw).is_err(), "篡改第 {i} 字节未被发现");
            raw[i] ^= 0x40;
        }
    }

    #[test]
    fn swapped_dh_key_rejected() {
        let a = DeviceIdentity::generate().unwrap();
        let b = DeviceIdentity::generate().unwrap();
        let r = DeviceRecord::issue(&a, "a", Platform::Android, 1).unwrap();
        let mut signed = SignedRecord::decode(r.raw()).unwrap();
        let mut body = RecordBody::decode(signed.body.as_slice()).unwrap();
        body.dh_pub = b.dh_public().to_vec();
        signed.body = body.encode_to_vec();
        assert_eq!(DeviceRecord::verify(&signed.encode_to_vec()), Err(CryptoError::BadSignature));
    }

    #[test]
    fn handcrafted_unsanitized_name_rejected() {
        let id = DeviceIdentity::generate().unwrap();
        let body = RecordBody {
            sign_pub: id.sign_pub().to_vec(),
            dh_pub: id.dh_public().to_vec(),
            name: "evil\u{202e}gpj.exe".into(),
            platform: 1,
            created_at_ms: 1,
        }
        .encode_to_vec();
        let sig = id.sign(sig_context::DEVICE_RECORD, &body);
        let raw = SignedRecord { body, sig: sig.to_vec() }.encode_to_vec();
        assert!(matches!(DeviceRecord::verify(&raw), Err(CryptoError::Malformed(_))));
    }

    #[test]
    fn name_is_sanitized() {
        let id = DeviceIdentity::generate().unwrap();
        let r = DeviceRecord::issue(&id, "  a\u{202e}\nb  ", Platform::Windows, 1).unwrap();
        assert_eq!(r.name, "ab");
        let long = "猫".repeat(200);
        let r = DeviceRecord::issue(&id, &long, Platform::Windows, 1).unwrap();
        assert_eq!(r.name.chars().count(), MAX_NAME_LEN);
    }
}
