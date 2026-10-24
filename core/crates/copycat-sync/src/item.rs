use copycat_crypto::DeviceId;
use copycat_proto::app;
use thiserror::Error;

use crate::hlc::Hlc;

pub const MAX_REPS: usize = 8;
pub const MAX_MIME_LEN: usize = 128;

pub const MAX_REP_SIZE: u64 = 64 * 1024 * 1024 * 1024;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ItemError {
    #[error("条目字段非法: {0}")]
    Invalid(&'static str),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rep {
    pub mime: String,
    pub size: u64,
    pub hash: [u8; 32],

    pub inline: Option<Vec<u8>>,
}

impl Rep {
    pub fn from_bytes(mime: impl Into<String>, data: Vec<u8>) -> Self {
        Self { mime: mime.into(), size: data.len() as u64, hash: *blake3::hash(&data).as_bytes(), inline: Some(data) }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClipItem {
    pub id: [u8; 16],
    pub origin: DeviceId,

    pub seq: u64,
    pub hlc: Hlc,
    pub reps: Vec<Rep>,
}

impl ClipItem {
    pub fn fingerprint(&self) -> Option<[u8; 32]> {
        self.reps.first().map(|r| r.hash)
    }

    pub fn is_complete(&self) -> bool {
        self.reps.iter().all(|r| r.inline.is_some())
    }

    pub fn to_wire(&self, budget: usize) -> app::ClipItem {
        let mut p = self.to_proto();
        let mut left = budget;
        for r in &mut p.reps {
            match &r.inline {
                Some(d) if d.len() <= left => left -= d.len(),
                Some(_) => r.inline = None,
                None => {}
            }
        }
        p
    }

    pub fn fill(&mut self, rep: usize, data: Vec<u8>) -> bool {
        let Some(r) = self.reps.get_mut(rep) else { return false };
        if r.inline.is_some() {
            return true;
        }
        if data.len() as u64 != r.size || *blake3::hash(&data).as_bytes() != r.hash {
            return false;
        }
        r.inline = Some(data);
        true
    }

    pub fn to_proto(&self) -> app::ClipItem {
        app::ClipItem {
            id: self.id.to_vec(),
            origin: self.origin.0.to_vec(),
            seq: self.seq,
            hlc: Some(app::Hlc {
                wall_ms: self.hlc.wall_ms,
                counter: self.hlc.counter,
                node: self.hlc.node.0.to_vec(),
            }),
            reps: self
                .reps
                .iter()
                .map(|r| app::Representation {
                    mime: r.mime.clone(),
                    size: r.size,
                    hash: r.hash.to_vec(),
                    inline: r.inline.clone(),
                })
                .collect(),
        }
    }

    pub fn from_proto(p: app::ClipItem) -> Result<Self, ItemError> {
        let id: [u8; 16] = p.id.as_slice().try_into().map_err(|_| ItemError::Invalid("id"))?;
        let origin = DeviceId::from_slice(&p.origin).map_err(|_| ItemError::Invalid("origin"))?;
        if p.seq == 0 {
            return Err(ItemError::Invalid("seq"));
        }
        let h = p.hlc.ok_or(ItemError::Invalid("hlc"))?;
        let hlc = Hlc {
            wall_ms: h.wall_ms,
            counter: h.counter,
            node: DeviceId::from_slice(&h.node).map_err(|_| ItemError::Invalid("hlc.node"))?,
        };
        if hlc.node != origin {
            return Err(ItemError::Invalid("hlc.node 与来源不符"));
        }
        if p.reps.is_empty() || p.reps.len() > MAX_REPS {
            return Err(ItemError::Invalid("reps 数量"));
        }
        let mut reps = Vec::with_capacity(p.reps.len());
        for r in p.reps {
            if r.mime.is_empty() || r.mime.len() > MAX_MIME_LEN || !r.mime.bytes().all(|b| b.is_ascii_graphic()) {
                return Err(ItemError::Invalid("mime"));
            }
            if r.size > MAX_REP_SIZE {
                return Err(ItemError::Invalid("size"));
            }
            let hash: [u8; 32] = r.hash.as_slice().try_into().map_err(|_| ItemError::Invalid("hash"))?;
            if let Some(data) = &r.inline
                && (data.len() as u64 != r.size || *blake3::hash(data).as_bytes() != hash)
            {
                return Err(ItemError::Invalid("内联内容与哈希不符"));
            }
            reps.push(Rep { mime: r.mime, size: r.size, hash, inline: r.inline });
        }
        Ok(Self { id, origin, seq: p.seq, hlc, reps })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> ClipItem {
        let node = DeviceId([1; 16]);
        ClipItem {
            id: [2; 16],
            origin: node,
            seq: 1,
            hlc: Hlc { wall_ms: 10, counter: 0, node },
            reps: vec![Rep::from_bytes("text/plain", "喵".as_bytes().to_vec())],
        }
    }

    #[test]
    fn proto_roundtrip() {
        let it = sample();
        assert_eq!(ClipItem::from_proto(it.to_proto()).unwrap(), it);
    }

    #[test]
    fn wire_form_strips_what_does_not_fit_and_fill_restores() {
        let mut it = sample();
        it.reps.push(Rep::from_bytes("image/png", vec![7u8; 300]));
        it.reps.push(Rep::from_bytes("text/html", vec![b'h'; 10]));
        let wire = it.to_wire(100);
        assert!(wire.reps[0].inline.is_some());
        assert!(wire.reps[1].inline.is_none(), "放不下的表示只带元数据");
        assert!(wire.reps[2].inline.is_some(), "后面放得下的仍然内联");
        let mut back = ClipItem::from_proto(wire).unwrap();
        assert!(!back.is_complete());
        assert!(!back.fill(1, vec![8u8; 300]), "哈希不符必须拒绝");
        assert!(!back.fill(1, vec![7u8; 299]), "大小不符必须拒绝");
        assert!(!back.fill(9, vec![]));
        assert!(back.fill(1, vec![7u8; 300]));
        assert!(back.is_complete());
        assert_eq!(back, it);
    }

    #[test]
    fn inline_hash_mismatch_rejected() {
        let mut p = sample().to_proto();
        p.reps[0].inline = Some(b"other".to_vec());
        assert!(ClipItem::from_proto(p).is_err());
    }

    #[test]
    fn invalid_fields_rejected() {
        let mut p = sample().to_proto();
        p.seq = 0;
        assert!(ClipItem::from_proto(p).is_err());

        let mut p = sample().to_proto();
        p.reps[0].mime = "text/plain\n".into();
        assert!(ClipItem::from_proto(p).is_err());

        let mut p = sample().to_proto();
        p.hlc.as_mut().unwrap().node = vec![9; 16];
        assert!(ClipItem::from_proto(p).is_err());

        let mut p = sample().to_proto();
        p.reps.clear();
        assert!(ClipItem::from_proto(p).is_err());
    }
}
