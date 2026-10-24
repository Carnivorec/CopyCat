use snow::{HandshakeState, TransportState};
use zeroize::Zeroizing;

use crate::identity::DeviceIdentity;
use crate::{CryptoError, Result};

pub const NOISE_IK: &str = "Noise_IK_25519_ChaChaPoly_BLAKE2s";

pub const NOISE_MAX: usize = 65535;
const TAG_LEN: usize = 16;

pub const MAX_CHUNK: usize = NOISE_MAX - TAG_LEN - 1;

pub const MAX_MESSAGE: usize = 2 * 1024 * 1024;

pub const REKEY_INTERVAL: u64 = 1 << 16;

const FLAG_FINAL: u8 = 0;
const FLAG_MORE: u8 = 1;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PathKind {
    Lan = 1,
    Relay = 2,
}

pub fn session_prologue(group_id: &[u8; 32], path: PathKind) -> Vec<u8> {
    let mut p = crate::kdf::prologue::SESSION.to_vec();
    p.extend_from_slice(group_id);
    p.push(path as u8);
    p
}

fn params(pattern: &str) -> Result<snow::params::NoiseParams> {
    pattern.parse().map_err(|_| CryptoError::Noise("Noise 参数无效".into()))
}

fn write_handshake(hs: &mut HandshakeState, payload: &[u8]) -> Result<Vec<u8>> {
    let mut buf = vec![0u8; NOISE_MAX];
    let n = hs.write_message(payload, &mut buf)?;
    buf.truncate(n);
    Ok(buf)
}

fn read_handshake(hs: &mut HandshakeState, msg: &[u8]) -> Result<Zeroizing<Vec<u8>>> {
    if msg.len() > NOISE_MAX {
        return Err(CryptoError::TooLarge);
    }
    let mut buf = Zeroizing::new(vec![0u8; NOISE_MAX]);
    let n = hs.read_message(msg, &mut buf)?;
    buf.truncate(n);
    Ok(buf)
}

fn remote_static(hs: &HandshakeState) -> Result<[u8; 32]> {
    hs.get_remote_static().and_then(|k| k.try_into().ok()).ok_or(CryptoError::Noise("缺少对端静态公钥".into()))
}

pub struct Initiator {
    hs: HandshakeState,
}

impl Initiator {
    pub fn start(local: &DeviceIdentity, remote_dh: &[u8; 32], prologue: &[u8]) -> Result<(Self, Vec<u8>)> {
        let mut hs = snow::Builder::new(params(NOISE_IK)?)
            .local_private_key(local.dh_private())?
            .remote_public_key(remote_dh)?
            .prologue(prologue)?
            .build_initiator()?;
        let msg = write_handshake(&mut hs, &[])?;
        Ok((Self { hs }, msg))
    }

    pub fn finish(mut self, msg: &[u8]) -> Result<Session> {
        read_handshake(&mut self.hs, msg)?;
        Session::from_handshake(self.hs)
    }
}

pub struct Responder {
    hs: HandshakeState,
    remote: [u8; 32],
}

impl Responder {
    pub fn read(local: &DeviceIdentity, prologue: &[u8], msg: &[u8]) -> Result<Self> {
        let mut hs = snow::Builder::new(params(NOISE_IK)?)
            .local_private_key(local.dh_private())?
            .prologue(prologue)?
            .build_responder()?;
        read_handshake(&mut hs, msg)?;
        let remote = remote_static(&hs)?;
        Ok(Self { hs, remote })
    }

    pub fn remote_static(&self) -> &[u8; 32] {
        &self.remote
    }

    pub fn accept(mut self) -> Result<(Session, Vec<u8>)> {
        let msg = write_handshake(&mut self.hs, &[])?;
        Ok((Session::from_handshake(self.hs)?, msg))
    }
}

pub struct Session {
    ts: TransportState,
    remote: [u8; 32],
    handshake_hash: [u8; 32],
    sent: u64,
    received: u64,
    reassembly: Zeroizing<Vec<u8>>,
}

impl Session {
    pub(crate) fn from_handshake(hs: HandshakeState) -> Result<Self> {
        let remote = remote_static(&hs)?;
        let handshake_hash =
            hs.get_handshake_hash().try_into().map_err(|_| CryptoError::Noise("握手哈希长度异常".into()))?;
        Ok(Self {
            ts: hs.into_transport_mode()?,
            remote,
            handshake_hash,
            sent: 0,
            received: 0,
            reassembly: Zeroizing::new(Vec::new()),
        })
    }

    pub fn remote_static(&self) -> &[u8; 32] {
        &self.remote
    }

    pub fn handshake_hash(&self) -> &[u8; 32] {
        &self.handshake_hash
    }

    pub fn seal(&mut self, msg: &[u8]) -> Result<Vec<Vec<u8>>> {
        if msg.len() > MAX_MESSAGE {
            return Err(CryptoError::TooLarge);
        }
        let mut records = Vec::with_capacity(msg.len() / MAX_CHUNK + 1);
        let mut chunks = msg.chunks(MAX_CHUNK).peekable();
        if chunks.peek().is_none() {
            records.push(self.seal_record(FLAG_FINAL, &[])?);
        }
        while let Some(chunk) = chunks.next() {
            let flag = if chunks.peek().is_some() { FLAG_MORE } else { FLAG_FINAL };
            records.push(self.seal_record(flag, chunk)?);
        }
        Ok(records)
    }

    fn seal_record(&mut self, flag: u8, chunk: &[u8]) -> Result<Vec<u8>> {
        let mut pt = Zeroizing::new(Vec::with_capacity(chunk.len() + 1));
        pt.push(flag);
        pt.extend_from_slice(chunk);
        let mut out = vec![0u8; pt.len() + TAG_LEN];
        let n = self.ts.write_message(&pt, &mut out)?;
        out.truncate(n);
        self.sent += 1;
        if self.sent.is_multiple_of(REKEY_INTERVAL) {
            self.ts.rekey_outgoing();
        }
        Ok(out)
    }

    pub fn open(&mut self, record: &[u8]) -> Result<Option<Vec<u8>>> {
        if record.len() > NOISE_MAX || record.len() < TAG_LEN + 1 {
            return Err(CryptoError::Decrypt);
        }
        let mut pt = Zeroizing::new(vec![0u8; record.len()]);
        let n = self.ts.read_message(record, &mut pt)?;
        self.received += 1;
        if self.received.is_multiple_of(REKEY_INTERVAL) {
            self.ts.rekey_incoming();
        }
        let (flag, chunk) = pt[..n].split_first().ok_or(CryptoError::Decrypt)?;
        if self.reassembly.len() + chunk.len() > MAX_MESSAGE {
            return Err(CryptoError::TooLarge);
        }
        self.reassembly.extend_from_slice(chunk);
        match *flag {
            FLAG_FINAL => Ok(Some(core::mem::take(&mut *self.reassembly))),
            FLAG_MORE => Ok(None),
            _ => Err(CryptoError::Decrypt),
        }
    }
}

impl core::fmt::Debug for Session {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Session").field("sent", &self.sent).field("received", &self.received).finish_non_exhaustive()
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) fn pair() -> (DeviceIdentity, DeviceIdentity, Session, Session) {
        let a = DeviceIdentity::generate().unwrap();
        let b = DeviceIdentity::generate().unwrap();
        let p = session_prologue(&[9u8; 32], PathKind::Relay);
        let (init, m1) = Initiator::start(&a, &b.dh_public(), &p).unwrap();
        let resp = Responder::read(&b, &p, &m1).unwrap();
        assert_eq!(resp.remote_static(), &a.dh_public());
        let (sb, m2) = resp.accept().unwrap();
        let sa = init.finish(&m2).unwrap();
        (a, b, sa, sb)
    }

    fn deliver(from: &mut Session, to: &mut Session, msg: &[u8]) -> Vec<u8> {
        let records = from.seal(msg).unwrap();
        let mut out = None;
        for (i, r) in records.iter().enumerate() {
            let got = to.open(r).unwrap();
            if i + 1 < records.len() {
                assert!(got.is_none());
            } else {
                out = got;
            }
        }
        out.unwrap()
    }

    #[test]
    fn handshake_and_messages() {
        let (a, b, mut sa, mut sb) = pair();
        assert_eq!(sa.remote_static(), &b.dh_public());
        assert_eq!(sb.remote_static(), &a.dh_public());
        assert_eq!(sa.handshake_hash(), sb.handshake_hash());
        assert_eq!(deliver(&mut sa, &mut sb, b"hello"), b"hello");
        assert_eq!(deliver(&mut sb, &mut sa, b""), b"");
        let big: Vec<u8> = (0..300_000u32).map(|i| i as u8).collect();
        assert_eq!(deliver(&mut sa, &mut sb, &big), big);
    }

    #[test]
    fn wrong_responder_key_fails() {
        let a = DeviceIdentity::generate().unwrap();
        let b = DeviceIdentity::generate().unwrap();
        let mallory = DeviceIdentity::generate().unwrap();
        let p = session_prologue(&[0u8; 32], PathKind::Lan);

        let (_, m1) = Initiator::start(&a, &b.dh_public(), &p).unwrap();
        assert!(Responder::read(&mallory, &p, &m1).is_err());
    }

    #[test]
    fn prologue_mismatch_fails() {
        let a = DeviceIdentity::generate().unwrap();
        let b = DeviceIdentity::generate().unwrap();
        let (_, m1) = Initiator::start(&a, &b.dh_public(), &session_prologue(&[0; 32], PathKind::Lan)).unwrap();
        assert!(Responder::read(&b, &session_prologue(&[0; 32], PathKind::Relay), &m1).is_err());
        assert!(Responder::read(&b, &session_prologue(&[1; 32], PathKind::Lan), &m1).is_err());
    }

    #[test]
    fn reordered_record_rejected() {
        let (_, _, mut sa, mut sb) = pair();
        let _r1 = sa.seal(b"one").unwrap().remove(0);
        let r2 = sa.seal(b"two").unwrap().remove(0);
        assert!(sb.open(&r2).is_err());
    }

    #[test]
    fn tampered_record_rejected() {
        let (_, _, mut sa, mut sb) = pair();
        let mut r1 = sa.seal(b"one").unwrap().remove(0);
        r1[3] ^= 1;
        assert!(sb.open(&r1).is_err());
    }

    #[test]
    fn replayed_record_rejected() {
        let (_, _, mut sa, mut sb) = pair();
        let r1 = sa.seal(b"one").unwrap().remove(0);
        assert_eq!(sb.open(&r1).unwrap().unwrap(), b"one");
        assert!(sb.open(&r1).is_err());
    }

    #[test]
    fn record_from_other_session_rejected() {
        let (_, _, mut sa, _) = pair();
        let (_, _, _, mut other) = pair();
        let r1 = sa.seal(b"one").unwrap().remove(0);
        assert!(other.open(&r1).is_err());
    }

    #[test]
    fn rekey_keeps_both_sides_in_sync() {
        let (_, _, mut sa, mut sb) = pair();
        for i in 0..(REKEY_INTERVAL + 5) {
            let r = sa.seal(&i.to_le_bytes()).unwrap().remove(0);
            assert_eq!(sb.open(&r).unwrap().unwrap(), i.to_le_bytes());
        }
    }
}
