use snow::HandshakeState;
use zeroize::Zeroizing;

use crate::identity::{DeviceId, DeviceIdentity};
use crate::kdf;
use crate::session::NOISE_MAX;
use crate::{CryptoError, Result};

pub const NOISE_K: &str = "Noise_K_25519_ChaChaPoly_BLAKE2s";
pub const MAX_PLAINTEXT: usize = 256 * 1024;
const CHUNK: usize = 60 * 1024;
const FLAG_FINAL: u8 = 0;
const FLAG_MORE: u8 = 1;

pub struct MailboxContext<'a> {
    pub group_id: &'a [u8; 32],
    pub sender: DeviceId,
    pub recipient: DeviceId,
    pub msg_id: &'a [u8],
}

impl MailboxContext<'_> {
    fn prologue(&self) -> Vec<u8> {
        let mut p = kdf::prologue::MAILBOX.to_vec();
        p.extend_from_slice(self.group_id);
        p.extend_from_slice(&self.sender.0);
        p.extend_from_slice(&self.recipient.0);
        p.extend_from_slice(&(self.msg_id.len() as u32).to_be_bytes());
        p.extend_from_slice(self.msg_id);
        p
    }
}

fn params() -> Result<snow::params::NoiseParams> {
    NOISE_K.parse().map_err(|_| CryptoError::Noise("Noise 参数无效".into()))
}

pub fn seal(
    sender: &DeviceIdentity,
    recipient_dh: &[u8; 32],
    ctx: &MailboxContext,
    plaintext: &[u8],
) -> Result<Vec<u8>> {
    if plaintext.len() > MAX_PLAINTEXT {
        return Err(CryptoError::TooLarge);
    }
    if ctx.sender != sender.id() {
        return Err(CryptoError::InvalidOp("发送方与上下文不符"));
    }
    let mut hs: HandshakeState = snow::Builder::new(params()?)
        .local_private_key(sender.dh_private())?
        .remote_public_key(recipient_dh)?
        .prologue(&ctx.prologue())?
        .build_initiator()?;

    let chunks: Vec<&[u8]> = if plaintext.is_empty() { vec![&[][..]] } else { plaintext.chunks(CHUNK).collect() };
    let mut out = Vec::with_capacity(plaintext.len() + chunks.len() * 64);
    let mut buf = vec![0u8; NOISE_MAX];
    let last = chunks.len() - 1;

    let frame = |i: usize, c: &[u8]| -> Zeroizing<Vec<u8>> {
        let mut pt = Zeroizing::new(Vec::with_capacity(c.len() + 1));
        pt.push(if i == last { FLAG_FINAL } else { FLAG_MORE });
        pt.extend_from_slice(c);
        pt
    };

    let n = hs.write_message(&frame(0, chunks[0]), &mut buf)?;
    push_msg(&mut out, &buf[..n]);
    let mut ts = hs.into_transport_mode()?;
    for (i, c) in chunks.iter().enumerate().skip(1) {
        let n = ts.write_message(&frame(i, c), &mut buf)?;
        push_msg(&mut out, &buf[..n]);
    }
    Ok(out)
}

pub fn open(recipient: &DeviceIdentity, sender_dh: &[u8; 32], ctx: &MailboxContext, data: &[u8]) -> Result<Vec<u8>> {
    if ctx.recipient != recipient.id() {
        return Err(CryptoError::InvalidOp("接收方与上下文不符"));
    }
    if data.len() > MAX_PLAINTEXT + 64 * 1024 {
        return Err(CryptoError::TooLarge);
    }
    let mut hs: HandshakeState = snow::Builder::new(params()?)
        .local_private_key(recipient.dh_private())?
        .remote_public_key(sender_dh)?
        .prologue(&ctx.prologue())?
        .build_responder()?;

    let mut msgs = split_msgs(data)?.into_iter();
    let first = msgs.next().ok_or(CryptoError::Decrypt)?;
    let mut buf = Zeroizing::new(vec![0u8; NOISE_MAX]);
    let mut out = Vec::new();

    let n = hs.read_message(first, &mut buf)?;
    let mut done = take_chunk(&buf[..n], &mut out)?;
    let mut ts = hs.into_transport_mode()?;
    for m in msgs {
        if done {
            return Err(CryptoError::Decrypt);
        }
        let n = ts.read_message(m, &mut buf)?;
        done = take_chunk(&buf[..n], &mut out)?;
    }
    if !done {
        return Err(CryptoError::Decrypt);
    }
    Ok(out)
}

fn push_msg(out: &mut Vec<u8>, m: &[u8]) {
    out.extend_from_slice(&(m.len() as u32).to_be_bytes());
    out.extend_from_slice(m);
}

fn split_msgs(mut data: &[u8]) -> Result<Vec<&[u8]>> {
    let mut v = Vec::new();
    while !data.is_empty() {
        if data.len() < 4 {
            return Err(CryptoError::Decrypt);
        }
        let len = u32::from_be_bytes(data[..4].try_into().unwrap()) as usize;
        if len > NOISE_MAX || data.len() < 4 + len {
            return Err(CryptoError::Decrypt);
        }
        v.push(&data[4..4 + len]);
        data = &data[4 + len..];
    }
    Ok(v)
}

fn take_chunk(pt: &[u8], out: &mut Vec<u8>) -> Result<bool> {
    let (flag, chunk) = pt.split_first().ok_or(CryptoError::Decrypt)?;
    if out.len() + chunk.len() > MAX_PLAINTEXT {
        return Err(CryptoError::TooLarge);
    }
    out.extend_from_slice(chunk);
    match *flag {
        FLAG_FINAL => Ok(true),
        FLAG_MORE => Ok(false),
        _ => Err(CryptoError::Decrypt),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx<'a>(g: &'a [u8; 32], s: &DeviceIdentity, r: &DeviceIdentity, id: &'a [u8]) -> MailboxContext<'a> {
        MailboxContext { group_id: g, sender: s.id(), recipient: r.id(), msg_id: id }
    }

    #[test]
    fn roundtrip_various_sizes() {
        let (a, b) = (DeviceIdentity::generate().unwrap(), DeviceIdentity::generate().unwrap());
        let g = [1u8; 32];
        for size in [0usize, 1, CHUNK - 1, CHUNK, CHUNK + 1, MAX_PLAINTEXT] {
            let pt: Vec<u8> = (0..size).map(|i| i as u8).collect();
            let c = seal(&a, &b.dh_public(), &ctx(&g, &a, &b, b"m1"), &pt).unwrap();
            assert_eq!(open(&b, &a.dh_public(), &ctx(&g, &a, &b, b"m1"), &c).unwrap(), pt, "size={size}");
        }
    }

    #[test]
    fn wrong_sender_or_context_rejected() {
        let (a, b, m) = (
            DeviceIdentity::generate().unwrap(),
            DeviceIdentity::generate().unwrap(),
            DeviceIdentity::generate().unwrap(),
        );
        let g = [1u8; 32];
        let c = seal(&a, &b.dh_public(), &ctx(&g, &a, &b, b"m1"), b"hi").unwrap();

        assert!(open(&b, &m.dh_public(), &ctx(&g, &a, &b, b"m1"), &c).is_err());

        assert!(open(&b, &a.dh_public(), &ctx(&g, &a, &b, b"m2"), &c).is_err());

        assert!(open(&b, &a.dh_public(), &ctx(&[2u8; 32], &a, &b, b"m1"), &c).is_err());

        assert!(open(&m, &a.dh_public(), &ctx(&g, &a, &m, b"m1"), &c).is_err());
    }

    #[test]
    fn truncation_and_tamper_rejected() {
        let (a, b) = (DeviceIdentity::generate().unwrap(), DeviceIdentity::generate().unwrap());
        let g = [1u8; 32];
        let pt = vec![7u8; CHUNK * 2 + 10];
        let c = seal(&a, &b.dh_public(), &ctx(&g, &a, &b, b"m"), &pt).unwrap();
        let msgs = split_msgs(&c).unwrap();
        assert_eq!(msgs.len(), 3);

        let truncated_len = c.len() - 4 - msgs[2].len();
        assert!(open(&b, &a.dh_public(), &ctx(&g, &a, &b, b"m"), &c[..truncated_len]).is_err());

        let mut t = c.clone();
        t[100] ^= 1;
        assert!(open(&b, &a.dh_public(), &ctx(&g, &a, &b, b"m"), &t).is_err());

        let mut extra = c.clone();
        push_msg(&mut extra, &[0u8; 40]);
        assert!(open(&b, &a.dh_public(), &ctx(&g, &a, &b, b"m"), &extra).is_err());
    }

    #[test]
    fn each_seal_is_unique() {
        let (a, b) = (DeviceIdentity::generate().unwrap(), DeviceIdentity::generate().unwrap());
        let g = [1u8; 32];
        let c1 = seal(&a, &b.dh_public(), &ctx(&g, &a, &b, b"m"), b"x").unwrap();
        let c2 = seal(&a, &b.dh_public(), &ctx(&g, &a, &b, b"m"), b"x").unwrap();
        assert_ne!(c1, c2);
    }
}
