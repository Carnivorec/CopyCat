use core::fmt;

use data_encoding::{BASE32_NOPAD, BASE64URL_NOPAD};
use snow::HandshakeState;
use zeroize::Zeroizing;

use crate::identity::{DeviceId, DeviceIdentity};
use crate::kdf::{self, purpose};
use crate::record::DeviceRecord;
use crate::session::{NOISE_MAX, Session};
use crate::{CryptoError, Result, random};

pub const NOISE_IKPSK2: &str = "Noise_IKpsk2_25519_ChaChaPoly_BLAKE2s";
pub const URI_PREFIX: &str = "copycat://pair?";

pub const DEFAULT_TTL_MS: u64 = 120_000;

#[derive(Clone)]
pub struct PairingSecret(Zeroizing<[u8; 32]>);

impl PairingSecret {
    pub fn generate() -> Result<Self> {
        Ok(Self(Zeroizing::new(random::bytes()?)))
    }

    fn psk(&self) -> Zeroizing<[u8; 32]> {
        Zeroizing::new(kdf::derive_key(purpose::PAIRING_PSK, &self.0[..]))
    }

    pub fn ticket_id(&self) -> [u8; 32] {
        kdf::derive_key(purpose::PAIRING_TICKET, &self.0[..])
    }

    pub fn relay_auth(&self) -> [u8; 32] {
        kdf::derive_key(purpose::PAIRING_RELAY_AUTH, &self.0[..])
    }
}

impl fmt::Debug for PairingSecret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("PairingSecret(..)")
    }
}

#[derive(Clone, Debug)]
pub struct PairingInvite {
    pub inviter_id: DeviceId,
    pub inviter_sign_pub: [u8; 32],
    pub inviter_dh_pub: [u8; 32],
    pub secret: PairingSecret,

    pub relay: Option<String>,

    pub relay_key: Option<[u8; 32]>,

    pub lan: Vec<String>,
    pub expires_at_ms: u64,
}

impl PairingInvite {
    pub fn new(
        inviter: &DeviceIdentity,
        relay: Option<(String, [u8; 32])>,
        lan: Vec<String>,
        now_ms: u64,
    ) -> Result<Self> {
        let (relay, relay_key) = match relay {
            Some((addr, key)) => (Some(addr), Some(key)),
            None => (None, None),
        };
        if lan.len() > MAX_LAN {
            return Err(CryptoError::Malformed("局域网地址过多"));
        }
        for a in relay.iter().chain(lan.iter()) {
            validate_addr(a)?;
        }
        Ok(Self {
            inviter_id: inviter.id(),
            inviter_sign_pub: inviter.sign_pub(),
            inviter_dh_pub: inviter.dh_public(),
            secret: PairingSecret::generate()?,
            relay,
            relay_key,
            lan,
            expires_at_ms: now_ms + DEFAULT_TTL_MS,
        })
    }

    pub fn to_uri(&self) -> String {
        let b64 = |b: &[u8]| BASE64URL_NOPAD.encode(b);
        let mut s = format!(
            "{URI_PREFIX}v=1&id={}&sk={}&dh={}&s={}&exp={}",
            BASE32_NOPAD.encode(&self.inviter_id.0).to_ascii_lowercase(),
            b64(&self.inviter_sign_pub),
            b64(&self.inviter_dh_pub),
            b64(&self.secret.0[..]),
            self.expires_at_ms,
        );
        if let (Some(r), Some(key)) = (&self.relay, &self.relay_key) {
            s.push_str(&format!("&relay={r}&rk={}", b64(key)));
        }
        if !self.lan.is_empty() {
            s.push_str(&format!("&lan={}", self.lan.join(",")));
        }
        s
    }

    pub fn parse_uri(uri: &str) -> Result<Self> {
        let query = uri.strip_prefix(URI_PREFIX).ok_or(CryptoError::Malformed("不是 CopyCat 配对码"))?;
        if query.len() > 2048 {
            return Err(CryptoError::TooLarge);
        }
        let mut f = std::collections::BTreeMap::new();
        for kv in query.split('&') {
            let (k, v) = kv.split_once('=').ok_or(CryptoError::Malformed("配对码字段格式错误"))?;
            if f.insert(k, v).is_some() {
                return Err(CryptoError::Malformed("配对码字段重复"));
            }
        }
        if f.get("v") != Some(&"1") {
            return Err(CryptoError::Malformed("配对码版本不支持"));
        }
        let b64_32 = |k: &str| -> Result<[u8; 32]> {
            let v = f.get(k).ok_or(CryptoError::Malformed("配对码缺少字段"))?;
            BASE64URL_NOPAD
                .decode(v.as_bytes())
                .ok()
                .and_then(|b| b.try_into().ok())
                .ok_or(CryptoError::Malformed("配对码字段编码错误"))
        };
        let inviter_sign_pub = b64_32("sk")?;
        let inviter_dh_pub = b64_32("dh")?;
        let secret = PairingSecret(Zeroizing::new(b64_32("s")?));
        let id_bytes = BASE32_NOPAD
            .decode(f.get("id").ok_or(CryptoError::Malformed("配对码缺少字段"))?.to_ascii_uppercase().as_bytes())
            .map_err(|_| CryptoError::Malformed("配对码字段编码错误"))?;
        let inviter_id = DeviceId::from_slice(&id_bytes)?;
        if inviter_id != DeviceId::from_sign_pub(&inviter_sign_pub) {
            return Err(CryptoError::Malformed("配对码中的设备 ID 与公钥不符"));
        }
        let expires_at_ms =
            f.get("exp").and_then(|v| v.parse().ok()).ok_or(CryptoError::Malformed("配对码缺少过期时间"))?;
        let (relay, relay_key) = match (f.get("relay"), f.contains_key("rk")) {
            (Some(r), true) => {
                validate_addr(r)?;
                (Some((*r).to_owned()), Some(b64_32("rk")?))
            }
            (None, false) => (None, None),
            _ => return Err(CryptoError::Malformed("中转地址与公钥必须同时出现")),
        };
        let lan = match f.get("lan") {
            Some(v) => {
                let list: Vec<String> = v.split(',').map(str::to_owned).collect();
                if list.len() > MAX_LAN {
                    return Err(CryptoError::Malformed("局域网地址过多"));
                }
                for a in &list {
                    validate_addr(a)?;
                }
                list
            }
            None => Vec::new(),
        };
        Ok(Self { inviter_id, inviter_sign_pub, inviter_dh_pub, secret, relay, relay_key, lan, expires_at_ms })
    }
}

const MAX_LAN: usize = 8;

pub fn validate_addr(a: &str) -> Result<()> {
    let ok = !a.is_empty()
        && a.len() <= 255
        && a.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | ':' | '[' | ']'))
        && a.rsplit_once(':').is_some_and(|(_, p)| p.parse::<u16>().is_ok_and(|p| p != 0));
    if ok { Ok(()) } else { Err(CryptoError::Malformed("地址格式错误")) }
}

fn pairing_prologue(inviter: &DeviceId) -> Vec<u8> {
    let mut p = kdf::prologue::PAIRING.to_vec();
    p.extend_from_slice(&inviter.0);
    p
}

fn params() -> Result<snow::params::NoiseParams> {
    NOISE_IKPSK2.parse().map_err(|_| CryptoError::Noise("Noise 参数无效".into()))
}

pub fn sas_code(handshake_hash: &[u8; 32]) -> String {
    let k = kdf::derive_key(purpose::PAIRING_SAS, handshake_hash);
    let v = u64::from_le_bytes(k[..8].try_into().unwrap()) % 1_000_000;
    format!("{v:06}")
}

pub struct Joiner {
    hs: HandshakeState,
    invite: PairingInvite,
}

pub struct JoinerOutcome {
    pub session: Session,

    pub inviter: DeviceRecord,
    pub sas: String,

    pub confirm: Vec<Vec<u8>>,
}

impl Joiner {
    pub fn start(joiner: &DeviceIdentity, invite: PairingInvite, now_ms: u64) -> Result<(Self, Vec<u8>)> {
        if now_ms > invite.expires_at_ms {
            return Err(CryptoError::Expired);
        }
        let psk = invite.secret.psk();
        let mut hs = snow::Builder::new(params()?)
            .local_private_key(joiner.dh_private())?
            .remote_public_key(&invite.inviter_dh_pub)?
            .prologue(&pairing_prologue(&invite.inviter_id))?
            .psk(2, &psk)?
            .build_initiator()?;
        let mut buf = vec![0u8; NOISE_MAX];
        let n = hs.write_message(&[], &mut buf)?;
        buf.truncate(n);
        Ok((Self { hs, invite }, buf))
    }

    pub fn finish(mut self, msg2: &[u8], own_record: &DeviceRecord) -> Result<JoinerOutcome> {
        if msg2.len() > NOISE_MAX {
            return Err(CryptoError::TooLarge);
        }
        let mut payload = vec![0u8; NOISE_MAX];
        let n = self.hs.read_message(msg2, &mut payload)?;
        let inviter = DeviceRecord::verify(&payload[..n])?;
        if inviter.id != self.invite.inviter_id || inviter.dh_pub != self.invite.inviter_dh_pub {
            return Err(CryptoError::InvalidOp("邀请方记录与二维码不符"));
        }
        let mut session = Session::from_handshake(self.hs)?;
        let sas = sas_code(session.handshake_hash());
        let confirm = session.seal(own_record.raw())?;
        Ok(JoinerOutcome { session, inviter, sas, confirm })
    }
}

pub struct PendingJoin {
    session: Session,
}

pub struct JoinRequest {
    pub record: DeviceRecord,
    pub sas: String,
}

pub fn respond(
    inviter: &DeviceIdentity,
    own_record: &DeviceRecord,
    secret: &PairingSecret,
    msg1: &[u8],
) -> Result<(PendingJoin, Vec<u8>)> {
    if msg1.len() > NOISE_MAX {
        return Err(CryptoError::TooLarge);
    }
    let psk = secret.psk();
    let mut hs = snow::Builder::new(params()?)
        .local_private_key(inviter.dh_private())?
        .prologue(&pairing_prologue(&inviter.id()))?
        .psk(2, &psk)?
        .build_responder()?;
    let mut scratch = vec![0u8; NOISE_MAX];
    hs.read_message(msg1, &mut scratch)?;
    let mut msg2 = vec![0u8; NOISE_MAX];
    let n = hs.write_message(own_record.raw(), &mut msg2)?;
    msg2.truncate(n);
    Ok((PendingJoin { session: Session::from_handshake(hs)? }, msg2))
}

impl PendingJoin {
    pub fn confirm(mut self, record: &[u8]) -> Result<(Session, JoinRequest)> {
        let raw = self.session.open(record)?.ok_or(CryptoError::Malformed("设备记录不完整"))?;
        let rec = DeviceRecord::verify(&raw)?;
        if &rec.dh_pub != self.session.remote_static() {
            return Err(CryptoError::InvalidOp("设备记录与握手公钥不符"));
        }
        let sas = sas_code(self.session.handshake_hash());
        Ok((self.session, JoinRequest { record: rec, sas }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::record::Platform;

    fn setup() -> (DeviceIdentity, DeviceRecord, DeviceIdentity, DeviceRecord, PairingInvite) {
        let a = DeviceIdentity::generate().unwrap();
        let ra = DeviceRecord::issue(&a, "台式机", Platform::Windows, 1).unwrap();
        let b = DeviceIdentity::generate().unwrap();
        let rb = DeviceRecord::issue(&b, "手机", Platform::Android, 1).unwrap();
        let invite = PairingInvite::new(
            &a,
            Some(("relay.example.com:443".into(), [5u8; 32])),
            vec!["192.168.1.10:47821".into()],
            1000,
        )
        .unwrap();
        (a, ra, b, rb, invite)
    }

    #[test]
    fn full_pairing_flow() {
        let (a, ra, b, rb, invite) = setup();
        let scanned = PairingInvite::parse_uri(&invite.to_uri()).unwrap();
        assert_eq!(scanned.inviter_id, a.id());
        assert_eq!(scanned.relay.as_deref(), Some("relay.example.com:443"));
        assert_eq!(scanned.lan, vec!["192.168.1.10:47821".to_string()]);

        let (joiner, m1) = Joiner::start(&b, scanned, 2000).unwrap();
        let (pending, m2) = respond(&a, &ra, &invite.secret, &m1).unwrap();
        let out = joiner.finish(&m2, &rb).unwrap();
        assert_eq!(out.inviter, ra);
        let (mut sa, req) = pending.confirm(&out.confirm[0]).unwrap();
        assert_eq!(req.record, rb);
        assert_eq!(req.sas, out.sas);
        assert_eq!(req.sas.len(), 6);

        let mut sb = out.session;
        let rec = sa.seal(b"roster").unwrap();
        assert_eq!(sb.open(&rec[0]).unwrap().unwrap(), b"roster");
    }

    #[test]
    fn wrong_secret_cannot_confirm() {
        let (a, ra, b, rb, invite) = setup();
        let mut forged = invite.clone();
        forged.secret = PairingSecret::generate().unwrap();
        let (joiner, m1) = Joiner::start(&b, forged, 2000).unwrap();

        let (pending, m2) = respond(&a, &ra, &invite.secret, &m1).unwrap();
        assert!(joiner.finish(&m2, &rb).is_err());
        assert!(pending.confirm(&[0u8; 64]).is_err());
    }

    #[test]
    fn impostor_inviter_detected() {
        let (_, _, b, _, invite) = setup();
        let mallory = DeviceIdentity::generate().unwrap();
        let rm = DeviceRecord::issue(&mallory, "x", Platform::Windows, 1).unwrap();
        let (joiner, m1) = Joiner::start(&b, invite.clone(), 2000).unwrap();

        assert!(respond(&mallory, &rm, &invite.secret, &m1).is_err());
        drop(joiner);
    }

    #[test]
    fn expired_invite_rejected() {
        let (_, _, b, _, invite) = setup();
        let exp = invite.expires_at_ms;
        assert!(matches!(Joiner::start(&b, invite, exp + 1), Err(CryptoError::Expired)));
    }

    #[test]
    fn malformed_uris_rejected() {
        let (_, _, _, _, invite) = setup();
        let good = invite.to_uri();
        assert!(PairingInvite::parse_uri("https://evil.example/").is_err());
        assert!(PairingInvite::parse_uri(&good.replace("v=1", "v=2")).is_err());
        assert!(PairingInvite::parse_uri(&format!("{good}&v=1")).is_err());
        assert!(PairingInvite::parse_uri(&good.replace("relay.example.com:443", "evil.example/<script>:443")).is_err());

        let other = DeviceIdentity::generate().unwrap();
        let swapped =
            good.replace(&BASE64URL_NOPAD.encode(&invite.inviter_sign_pub), &BASE64URL_NOPAD.encode(&other.sign_pub()));
        assert!(PairingInvite::parse_uri(&swapped).is_err());
    }

    #[test]
    fn derived_values_are_distinct() {
        let s = PairingSecret::generate().unwrap();
        assert_ne!(s.ticket_id(), s.relay_auth());
        assert_ne!(&s.relay_auth()[..], &s.psk()[..]);
    }
}
