use std::collections::{BTreeMap, BTreeSet};

use prost::Message;

use crate::identity::{self, DeviceId, DeviceIdentity};
use crate::kdf::{self, sig_context};
use crate::record::DeviceRecord;
use crate::wire::{OpBody, RemoveOp, SignedOp, op_body};
use crate::{CryptoError, Result};

pub type OpHash = [u8; 32];

pub const MAX_OPS: usize = 4096;

const MAX_PENDING: usize = 256;
const MAX_OP_LEN: usize = 16 * 1024;

#[derive(Clone, Debug)]
enum OpKind {
    Add(DeviceRecord),
    Remove { target: DeviceId, grandfathered: BTreeSet<OpHash> },
}

#[derive(Clone, Debug)]
struct ParsedOp {
    hash: OpHash,
    raw: Vec<u8>,
    body: Vec<u8>,
    signer: DeviceId,
    sig: Vec<u8>,
    group: Vec<u8>,
    kind: OpKind,
}

fn parse_op(raw: &[u8]) -> Result<ParsedOp> {
    if raw.len() > MAX_OP_LEN {
        return Err(CryptoError::TooLarge);
    }
    let signed = SignedOp::decode(raw)?;
    let body = OpBody::decode(signed.body.as_slice())?;
    let kind = match body.kind {
        Some(op_body::Kind::Add(rec)) => OpKind::Add(DeviceRecord::verify(&rec)?),
        Some(op_body::Kind::Remove(r)) => {
            let mut grandfathered = BTreeSet::new();
            for h in r.grandfathered {
                grandfathered.insert(h.as_slice().try_into().map_err(|_| CryptoError::Malformed("操作哈希长度错误"))?);
            }
            OpKind::Remove { target: DeviceId::from_slice(&r.target)?, grandfathered }
        }
        None => return Err(CryptoError::Malformed("名册操作缺少内容")),
    };
    Ok(ParsedOp {
        hash: kdf::hash(raw),
        raw: raw.to_vec(),
        signer: DeviceId::from_slice(&signed.signer)?,
        sig: signed.sig,
        group: body.group,
        body: signed.body,
        kind,
    })
}

fn sign_op(identity: &DeviceIdentity, group: &[u8], issued_at_ms: u64, kind: op_body::Kind) -> Vec<u8> {
    let body = OpBody { group: group.to_vec(), issued_at_ms, kind: Some(kind) }.encode_to_vec();
    let sig = identity.sign(sig_context::ROSTER_OP, &body);
    SignedOp { body, signer: identity.id().0.to_vec(), sig: sig.to_vec() }.encode_to_vec()
}

#[derive(Clone, Debug)]
pub struct Roster {
    group_id: OpHash,
    genesis: DeviceId,

    ops: BTreeMap<OpHash, ParsedOp>,

    pending: BTreeMap<OpHash, ParsedOp>,
    members: BTreeMap<DeviceId, DeviceRecord>,
    revoked: BTreeSet<DeviceId>,
}

impl Roster {
    pub fn create(identity: &DeviceIdentity, record: &DeviceRecord, now_ms: u64) -> Result<(Self, Vec<u8>)> {
        if record.id != identity.id() {
            return Err(CryptoError::InvalidOp("创世记录不属于本机"));
        }
        let raw = sign_op(identity, &[], now_ms, op_body::Kind::Add(record.raw().to_vec()));
        let roster = Self::from_ops(&kdf::hash(&raw), [raw.as_slice()])?;
        Ok((roster, raw))
    }

    pub fn from_ops<'a>(group_id: &OpHash, ops: impl IntoIterator<Item = &'a [u8]>) -> Result<Self> {
        let parsed: Vec<ParsedOp> = ops.into_iter().filter_map(|r| parse_op(r).ok()).collect();
        let genesis = parsed.iter().find(|op| &op.hash == group_id).ok_or(CryptoError::InvalidOp("缺少创世操作"))?;
        let OpKind::Add(rec) = &genesis.kind else {
            return Err(CryptoError::InvalidOp("创世操作必须是 Add"));
        };
        if !genesis.group.is_empty() || rec.id != genesis.signer {
            return Err(CryptoError::InvalidOp("创世操作格式错误"));
        }
        identity::verify(&rec.sign_pub, sig_context::ROSTER_OP, &genesis.body, &genesis.sig)?;
        let mut roster = Self {
            group_id: *group_id,
            genesis: rec.id,
            ops: BTreeMap::new(),
            pending: BTreeMap::new(),
            members: BTreeMap::new(),
            revoked: BTreeSet::new(),
        };
        roster.recompute(parsed);
        Ok(roster)
    }

    pub fn merge<'a>(&mut self, ops: impl IntoIterator<Item = &'a [u8]>) -> bool {
        let mut candidates: Vec<ParsedOp> = self.ops.values().chain(self.pending.values()).cloned().collect();
        let before = self.digest();
        for raw in ops {
            if candidates.len() >= MAX_OPS + MAX_PENDING {
                break;
            }
            if let Ok(op) = parse_op(raw)
                && !self.ops.contains_key(&op.hash)
                && !self.pending.contains_key(&op.hash)
            {
                candidates.push(op);
            }
        }
        let members_before = self.members.clone();
        self.recompute(candidates);
        before != self.digest() || members_before != self.members
    }

    pub fn add(&mut self, identity: &DeviceIdentity, record: &DeviceRecord, now_ms: u64) -> Result<Vec<u8>> {
        self.require_member(identity.id())?;
        if self.revoked.contains(&record.id) {
            return Err(CryptoError::Revoked);
        }
        let raw = sign_op(identity, &self.group_id, now_ms, op_body::Kind::Add(record.raw().to_vec()));
        self.merge([raw.as_slice()]);
        Ok(raw)
    }

    pub fn remove(&mut self, identity: &DeviceIdentity, target: DeviceId, now_ms: u64) -> Result<Vec<u8>> {
        self.require_member(identity.id())?;
        let grandfathered = self
            .ops
            .values()
            .filter(|op| op.signer == target && matches!(op.kind, OpKind::Add(_)))
            .map(|op| op.hash.to_vec())
            .collect();
        let raw = sign_op(
            identity,
            &self.group_id,
            now_ms,
            op_body::Kind::Remove(RemoveOp { target: target.0.to_vec(), grandfathered }),
        );
        self.merge([raw.as_slice()]);
        Ok(raw)
    }

    pub fn group_id(&self) -> &OpHash {
        &self.group_id
    }

    pub fn genesis_id(&self) -> DeviceId {
        self.genesis
    }

    pub fn is_member(&self, id: &DeviceId) -> bool {
        self.members.contains_key(id)
    }

    pub fn is_revoked(&self, id: &DeviceId) -> bool {
        self.revoked.contains(id)
    }

    pub fn member(&self, id: &DeviceId) -> Option<&DeviceRecord> {
        self.members.get(id)
    }

    pub fn member_by_dh(&self, dh_pub: &[u8]) -> Option<&DeviceRecord> {
        self.members.values().find(|r| kdf::ct_eq(&r.dh_pub, dh_pub))
    }

    pub fn members(&self) -> impl Iterator<Item = &DeviceRecord> {
        self.members.values()
    }

    pub fn ops_raw(&self) -> Vec<Vec<u8>> {
        self.ops.values().map(|op| op.raw.clone()).collect()
    }

    pub fn op_count(&self) -> usize {
        self.ops.len()
    }

    pub fn digest(&self) -> [u8; 32] {
        let mut h = blake3::Hasher::new();
        for k in self.ops.keys() {
            h.update(k);
        }
        *h.finalize().as_bytes()
    }

    fn require_member(&self, id: DeviceId) -> Result<()> {
        if self.revoked.contains(&id) {
            Err(CryptoError::Revoked)
        } else if !self.members.contains_key(&id) {
            Err(CryptoError::NotMember)
        } else {
            Ok(())
        }
    }

    fn recompute(&mut self, candidates: Vec<ParsedOp>) {
        let mut pending: BTreeMap<OpHash, ParsedOp> = candidates
            .into_iter()
            .filter(|op| op.hash == self.group_id || op.group.as_slice() == self.group_id)
            .map(|op| (op.hash, op))
            .collect();

        let mut sign_keys: BTreeMap<DeviceId, [u8; 32]> = BTreeMap::new();
        let mut verified: BTreeMap<OpHash, ParsedOp> = BTreeMap::new();
        let genesis_op = pending.remove(&self.group_id).expect("创世操作已在 from_ops 中校验");
        if let OpKind::Add(rec) = &genesis_op.kind {
            sign_keys.insert(rec.id, rec.sign_pub);
        }
        verified.insert(genesis_op.hash, genesis_op);
        loop {
            let ready: Vec<OpHash> = pending
                .values()
                .filter(|op| {
                    sign_keys
                        .get(&op.signer)
                        .is_some_and(|pk| identity::verify(pk, sig_context::ROSTER_OP, &op.body, &op.sig).is_ok())
                })
                .map(|op| op.hash)
                .collect();
            if ready.is_empty() {
                break;
            }
            for h in ready {
                let op = pending.remove(&h).expect("刚从 pending 中取出");
                if let OpKind::Add(rec) = &op.kind {
                    sign_keys.insert(rec.id, rec.sign_pub);
                }
                verified.insert(h, op);
            }
        }

        while pending.len() > MAX_PENDING {
            pending.pop_last();
        }
        while verified.len() > MAX_OPS {
            let last = *verified.keys().next_back().expect("非空");
            if last == self.group_id {
                break;
            }
            verified.remove(&last);
        }

        let mut revoked = BTreeSet::new();
        let mut grandfathered: BTreeMap<DeviceId, BTreeSet<OpHash>> = BTreeMap::new();
        for op in verified.values() {
            if let OpKind::Remove { target, grandfathered: g } = &op.kind {
                revoked.insert(*target);
                grandfathered.entry(*target).or_default().extend(g.iter().copied());
            }
        }

        let mut reach: BTreeSet<DeviceId> = BTreeSet::from([self.genesis]);
        let mut valid_adds: Vec<&ParsedOp> = Vec::new();
        loop {
            let mut changed = false;
            for op in verified.values() {
                let OpKind::Add(rec) = &op.kind else { continue };
                if valid_adds.iter().any(|v| v.hash == op.hash) || !reach.contains(&op.signer) {
                    continue;
                }
                let signer_ok = !revoked.contains(&op.signer)
                    || grandfathered.get(&op.signer).is_some_and(|g| g.contains(&op.hash));
                if signer_ok {
                    valid_adds.push(op);
                    changed |= reach.insert(rec.id);
                }
            }
            if !changed {
                break;
            }
        }

        let mut members: BTreeMap<DeviceId, DeviceRecord> = BTreeMap::new();
        for op in valid_adds {
            let OpKind::Add(rec) = &op.kind else { continue };
            if revoked.contains(&rec.id) {
                continue;
            }
            let replace = members.get(&rec.id).is_none_or(|cur| {
                (rec.created_at_ms, kdf::hash(rec.raw())) > (cur.created_at_ms, kdf::hash(cur.raw()))
            });
            if replace {
                members.insert(rec.id, rec.clone());
            }
        }

        self.ops = verified;
        self.pending = pending;
        self.members = members;
        self.revoked = revoked;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::record::Platform;
    use proptest::prelude::*;

    struct Dev {
        id: DeviceIdentity,
        rec: DeviceRecord,
    }

    fn dev(name: &str) -> Dev {
        let id = DeviceIdentity::generate().unwrap();
        let rec = DeviceRecord::issue(&id, name, Platform::Windows, 1).unwrap();
        Dev { id, rec }
    }

    fn ops_of(r: &Roster) -> Vec<Vec<u8>> {
        r.ops_raw()
    }

    fn rebuild(group: &OpHash, ops: &[Vec<u8>]) -> Roster {
        Roster::from_ops(group, ops.iter().map(Vec::as_slice)).unwrap()
    }

    #[test]
    fn genesis_and_add() {
        let (a, b) = (dev("a"), dev("b"));
        let (mut r, _) = Roster::create(&a.id, &a.rec, 1).unwrap();
        assert!(r.is_member(&a.id.id()));
        r.add(&a.id, &b.rec, 2).unwrap();
        assert!(r.is_member(&b.id.id()));
        assert_eq!(r.member_by_dh(&b.id.dh_public()).unwrap().id, b.id.id());

        let copy = rebuild(r.group_id(), &ops_of(&r));
        assert_eq!(copy.digest(), r.digest());
        assert!(copy.is_member(&b.id.id()));
    }

    #[test]
    fn non_member_cannot_add() {
        let (a, x, y) = (dev("a"), dev("x"), dev("y"));
        let (mut r, _) = Roster::create(&a.id, &a.rec, 1).unwrap();
        assert_eq!(r.add(&x.id, &y.rec, 2), Err(CryptoError::NotMember));

        let forged = sign_op(&x.id, r.group_id(), 2, op_body::Kind::Add(y.rec.raw().to_vec()));
        r.merge([forged.as_slice()]);
        assert!(!r.is_member(&y.id.id()));
        assert_eq!(r.op_count(), 1, "无法验签的操作不得计入名册");
    }

    #[test]
    fn signer_field_cannot_be_spoofed() {
        let (a, x, y) = (dev("a"), dev("x"), dev("y"));
        let (mut r, _) = Roster::create(&a.id, &a.rec, 1).unwrap();

        let body = OpBody {
            group: r.group_id().to_vec(),
            issued_at_ms: 2,
            kind: Some(op_body::Kind::Add(y.rec.raw().to_vec())),
        }
        .encode_to_vec();
        let sig = x.id.sign(sig_context::ROSTER_OP, &body);
        let raw = SignedOp { body, signer: a.id.id().0.to_vec(), sig: sig.to_vec() }.encode_to_vec();
        r.merge([raw.as_slice()]);
        assert!(!r.is_member(&y.id.id()));
    }

    #[test]
    fn removed_device_cannot_add_but_prior_adds_survive() {
        let (a, b, c, w) = (dev("a"), dev("b"), dev("c"), dev("w"));
        let (mut r, _) = Roster::create(&a.id, &a.rec, 1).unwrap();
        r.add(&a.id, &b.rec, 2).unwrap();
        r.add(&b.id, &c.rec, 3).unwrap();
        r.remove(&a.id, b.id.id(), 4).unwrap();
        assert!(r.is_revoked(&b.id.id()));
        assert!(!r.is_member(&b.id.id()));
        assert!(r.is_member(&c.id.id()), "被移除设备此前添加的设备应保留");

        let backdated = sign_op(&b.id, r.group_id(), 0, op_body::Kind::Add(w.rec.raw().to_vec()));
        r.merge([backdated.as_slice()]);
        assert!(!r.is_member(&w.id.id()), "被移除设备签发的新操作必须无效");
        assert_eq!(r.add(&b.id, &w.rec, 5), Err(CryptoError::Revoked));
    }

    #[test]
    fn revoked_device_cannot_be_readded() {
        let (a, b) = (dev("a"), dev("b"));
        let (mut r, _) = Roster::create(&a.id, &a.rec, 1).unwrap();
        r.add(&a.id, &b.rec, 2).unwrap();
        r.remove(&a.id, b.id.id(), 3).unwrap();
        assert_eq!(r.add(&a.id, &b.rec, 4), Err(CryptoError::Revoked));

        let raw = sign_op(&a.id, r.group_id(), 5, op_body::Kind::Add(b.rec.raw().to_vec()));
        r.merge([raw.as_slice()]);
        assert!(!r.is_member(&b.id.id()));
    }

    #[test]
    fn genesis_can_be_removed() {
        let (a, b, c) = (dev("a"), dev("b"), dev("c"));
        let (mut r, _) = Roster::create(&a.id, &a.rec, 1).unwrap();
        r.add(&a.id, &b.rec, 2).unwrap();
        r.remove(&b.id, a.id.id(), 3).unwrap();
        assert!(!r.is_member(&a.id.id()));
        assert!(r.is_member(&b.id.id()));
        r.add(&b.id, &c.rec, 4).unwrap();
        assert!(r.is_member(&c.id.id()));
    }

    #[test]
    fn device_can_remove_itself() {
        let (a, b, c) = (dev("a"), dev("b"), dev("c"));
        let (mut r, _) = Roster::create(&a.id, &a.rec, 1).unwrap();
        r.add(&a.id, &b.rec, 2).unwrap();
        r.add(&b.id, &c.rec, 3).unwrap();

        let mut other = r.clone();
        let op = r.remove(&b.id, b.id.id(), 4).unwrap();
        other.merge([op.as_slice()]);
        for r in [&r, &other] {
            assert!(!r.is_member(&b.id.id()));
            assert!(r.is_member(&a.id.id()));
            assert!(r.is_member(&c.id.id()));
        }
        assert_eq!(r.digest(), other.digest());
        assert_eq!(other.add(&a.id, &b.rec, 5), Err(CryptoError::Revoked));
    }

    #[test]
    fn mutual_removal_revokes_both() {
        let (a, b, c) = (dev("a"), dev("b"), dev("c"));
        let (mut r1, _) = Roster::create(&a.id, &a.rec, 1).unwrap();
        r1.add(&a.id, &b.rec, 2).unwrap();
        r1.add(&a.id, &c.rec, 2).unwrap();
        let mut r2 = r1.clone();
        r1.remove(&a.id, b.id.id(), 3).unwrap();
        r2.remove(&b.id, a.id.id(), 3).unwrap();
        r1.merge(ops_of(&r2).iter().map(Vec::as_slice));
        r2.merge(ops_of(&r1).iter().map(Vec::as_slice));
        assert_eq!(r1.digest(), r2.digest());
        for r in [&r1, &r2] {
            assert!(!r.is_member(&a.id.id()));
            assert!(!r.is_member(&b.id.id()));
            assert!(r.is_member(&c.id.id()));
        }
    }

    #[test]
    fn rename_takes_latest_record() {
        let a = dev("a");
        let (mut r, _) = Roster::create(&a.id, &a.rec, 1).unwrap();
        let renamed = DeviceRecord::issue(&a.id, "新名字", Platform::Windows, 10).unwrap();
        r.add(&a.id, &renamed, 10).unwrap();
        assert_eq!(r.member(&a.id.id()).unwrap().name, "新名字");
    }

    #[test]
    fn foreign_group_ops_ignored() {
        let (a, b, z) = (dev("a"), dev("b"), dev("z"));
        let (mut r, _) = Roster::create(&a.id, &a.rec, 1).unwrap();
        let (mut other, _) = Roster::create(&z.id, &z.rec, 1).unwrap();
        other.add(&z.id, &b.rec, 2).unwrap();
        r.merge(ops_of(&other).iter().map(Vec::as_slice));
        assert!(!r.is_member(&b.id.id()));
        assert!(!r.is_member(&z.id.id()));
    }

    #[test]
    fn wrong_group_id_rejected() {
        let a = dev("a");
        let (r, _) = Roster::create(&a.id, &a.rec, 1).unwrap();
        assert!(Roster::from_ops(&[0u8; 32], ops_of(&r).iter().map(Vec::as_slice)).is_err());
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(64))]

        #[test]
        fn merge_order_independent(seed in any::<u64>(), split in 0usize..20) {
            let devs: Vec<Dev> = (0..5).map(|i| dev(&format!("d{i}"))).collect();
            let (mut r, _) = Roster::create(&devs[0].id, &devs[0].rec, 1).unwrap();
            r.add(&devs[0].id, &devs[1].rec, 2).unwrap();
            r.add(&devs[1].id, &devs[2].rec, 3).unwrap();
            r.add(&devs[2].id, &devs[3].rec, 4).unwrap();
            r.remove(&devs[0].id, devs[1].id.id(), 5).unwrap();
            r.add(&devs[3].id, &devs[4].rec, 6).unwrap();
            r.remove(&devs[3].id, devs[2].id.id(), 7).unwrap();
            let all = ops_of(&r);

            let mut shuffled = all.clone();
            let mut s = seed;
            for i in (1..shuffled.len()).rev() {
                s = s.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
                shuffled.swap(i, (s >> 33) as usize % (i + 1));
            }
            let genesis_raw = all.iter().find(|o| kdf::hash(o) == *r.group_id()).unwrap().clone();
            let mut r2 = Roster::from_ops(r.group_id(), [genesis_raw.as_slice()]).unwrap();
            let cut = split % (shuffled.len() + 1);
            r2.merge(shuffled[..cut].iter().map(Vec::as_slice));
            r2.merge(shuffled[cut..].iter().map(Vec::as_slice));

            prop_assert_eq!(r2.digest(), r.digest());
            let m1: Vec<_> = r.members().map(|m| m.id).collect();
            let m2: Vec<_> = r2.members().map(|m| m.id).collect();
            prop_assert_eq!(m1, m2);
        }
    }
}
