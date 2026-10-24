use std::time::Instant;

use copycat_crypto::session::{Initiator, PathKind, Responder, session_prologue};
use copycat_crypto::{DeviceIdentity, DeviceRecord, Session, kdf, random};
use copycat_proto::peer::{PeerFrame, peer_frame::Kind};

use crate::{CoreError, Result};

pub(crate) enum Link {
    Initiating { init: Box<Initiator>, sid: u64, since: Instant },
    Up { session: Box<Session>, sid: u64 },
}

impl Link {
    pub fn is_up(&self) -> bool {
        matches!(self, Link::Up { .. })
    }

    pub fn sid(&self) -> u64 {
        match self {
            Link::Initiating { sid, .. } | Link::Up { sid, .. } => *sid,
        }
    }
}

pub(crate) fn frame(sid: u64, kind: Kind) -> PeerFrame {
    PeerFrame { session_id: sid, kind: Some(kind) }
}

pub(crate) fn initiate(
    identity: &DeviceIdentity,
    group: &[u8; 32],
    peer: &DeviceRecord,
    path: PathKind,
) -> Result<(Link, PeerFrame)> {
    let sid = random::u64()?;
    let (init, m1) = Initiator::start(identity, &peer.dh_pub, &session_prologue(group, path))?;
    Ok((Link::Initiating { init: Box::new(init), sid, since: Instant::now() }, frame(sid, Kind::HandshakeInit(m1))))
}

pub(crate) fn respond(
    identity: &DeviceIdentity,
    group: &[u8; 32],
    peer: &DeviceRecord,
    path: PathKind,
    sid: u64,
    m1: &[u8],
) -> Result<(Link, PeerFrame)> {
    let resp = Responder::read(identity, &session_prologue(group, path), m1)?;
    if !kdf::ct_eq(resp.remote_static(), &peer.dh_pub) {
        return Err(CoreError::InvalidState("握手公钥与名册不符"));
    }
    let (session, m2) = resp.accept()?;
    Ok((Link::Up { session: Box::new(session), sid }, frame(sid, Kind::HandshakeResp(m2))))
}

pub(crate) fn respond_unknown(
    identity: &DeviceIdentity,
    group: &[u8; 32],
    path: PathKind,
    sid: u64,
    m1: &[u8],
    lookup: impl Fn(&[u8; 32]) -> Option<DeviceRecord>,
) -> Result<(DeviceRecord, Link, PeerFrame)> {
    let resp = Responder::read(identity, &session_prologue(group, path), m1)?;
    let peer = lookup(resp.remote_static()).ok_or(CoreError::InvalidState("对端不在名册中"))?;
    let (session, m2) = resp.accept()?;
    Ok((peer, Link::Up { session: Box::new(session), sid }, frame(sid, Kind::HandshakeResp(m2))))
}

pub(crate) fn finish(link: Link, peer: &DeviceRecord, m2: &[u8]) -> Result<Link> {
    let Link::Initiating { init, sid, .. } = link else {
        return Err(CoreError::InvalidState("未在握手中"));
    };
    let session = init.finish(m2)?;
    if !kdf::ct_eq(session.remote_static(), &peer.dh_pub) {
        return Err(CoreError::InvalidState("握手公钥与名册不符"));
    }
    Ok(Link::Up { session: Box::new(session), sid })
}
