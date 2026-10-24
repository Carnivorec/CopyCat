use std::io;
use std::net::{IpAddr, Ipv4Addr, SocketAddr, SocketAddrV4};
use std::sync::Mutex;

use socket2::{Domain, Protocol, SockRef, Socket, Type};
use tokio::net::UdpSocket;

use crate::lan::is_lan_ip;

pub const DISCOVERY_PORT: u16 = 47822;

pub const MULTICAST: Ipv4Addr = Ipv4Addr::new(239, 255, 70, 77);
pub const BEACON_LEN: usize = 4 + 1 + 2 + 4 + 16;
const MAGIC: &[u8; 4] = b"CCB1";
const FLAG_WANT_REPLY: u8 = 1;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Beacon {
    pub want_reply: bool,
    pub port: u16,
    pub bucket: u32,
    pub tag: [u8; 16],
}

impl Beacon {
    pub fn encode(&self) -> [u8; BEACON_LEN] {
        let mut b = [0u8; BEACON_LEN];
        b[..4].copy_from_slice(MAGIC);
        b[4] = if self.want_reply { FLAG_WANT_REPLY } else { 0 };
        b[5..7].copy_from_slice(&self.port.to_be_bytes());
        b[7..11].copy_from_slice(&self.bucket.to_be_bytes());
        b[11..].copy_from_slice(&self.tag);
        b
    }

    pub fn decode(b: &[u8]) -> Option<Self> {
        if b.len() != BEACON_LEN || &b[..4] != MAGIC {
            return None;
        }
        Some(Self {
            want_reply: b[4] & FLAG_WANT_REPLY != 0,
            port: u16::from_be_bytes([b[5], b[6]]),
            bucket: u32::from_be_bytes([b[7], b[8], b[9], b[10]]),
            tag: b[11..].try_into().ok()?,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Iface {
    pub ip: Ipv4Addr,
    pub broadcast: Option<Ipv4Addr>,
}

pub fn lan_ifaces() -> Vec<Iface> {
    let mut out: Vec<Iface> = Vec::new();
    for i in if_addrs::get_if_addrs().unwrap_or_default() {
        let if_addrs::IfAddr::V4(a) = &i.addr else { continue };

        let up = matches!(i.oper_status, if_addrs::IfOperStatus::Up | if_addrs::IfOperStatus::Unknown);
        if i.is_loopback() || i.is_p2p() || !up || !is_lan_ip(a.ip, false) {
            continue;
        }
        let broadcast =
            a.broadcast.or_else(|| (a.prefixlen < 31).then(|| Ipv4Addr::from(u32::from(a.ip) | !u32::from(a.netmask))));
        if !out.iter().any(|o| o.ip == a.ip) {
            out.push(Iface { ip: a.ip, broadcast });
        }
    }
    out
}

pub struct Discovery {
    sock: UdpSocket,
    joined: Mutex<Vec<Ipv4Addr>>,
}

impl Discovery {
    pub fn bind(port: u16, loopback_only: bool) -> io::Result<Self> {
        let s = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP))?;
        s.set_reuse_address(true)?;
        s.set_broadcast(true)?;
        s.set_multicast_loop_v4(false)?;
        s.set_nonblocking(true)?;
        let ip = if loopback_only { Ipv4Addr::LOCALHOST } else { Ipv4Addr::UNSPECIFIED };
        s.bind(&SocketAddr::V4(SocketAddrV4::new(ip, port)).into())?;
        Ok(Self { sock: UdpSocket::from_std(s.into())?, joined: Mutex::new(Vec::new()) })
    }

    pub fn port(&self) -> u16 {
        self.sock.local_addr().map(|a| a.port()).unwrap_or(0)
    }

    pub fn join(&self, ifaces: &[Iface]) {
        let mut joined = self.joined.lock().unwrap_or_else(|p| p.into_inner());
        for i in ifaces {
            if !joined.contains(&i.ip) && self.sock.join_multicast_v4(MULTICAST, i.ip).is_ok() {
                joined.push(i.ip);
            }
        }
    }

    pub async fn announce(&self, beacon: &[u8], ifaces: &[Iface], port: u16) {
        for i in ifaces {
            if let Some(bc) = i.broadcast {
                let _ = self.sock.send_to(beacon, SocketAddr::new(IpAddr::V4(bc), port)).await;
            }
            if SockRef::from(&self.sock).set_multicast_if_v4(&i.ip).is_ok() {
                let _ = self.sock.send_to(beacon, SocketAddr::new(IpAddr::V4(MULTICAST), port)).await;
            }
        }
    }

    pub async fn send_to(&self, beacon: &[u8], to: SocketAddr) {
        let _ = self.sock.send_to(beacon, to).await;
    }

    pub async fn recv(&self, buf: &mut [u8]) -> io::Result<(usize, SocketAddr)> {
        self.sock.recv_from(buf).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn beacon_roundtrip() {
        let b = Beacon { want_reply: true, port: 47821, bucket: 2_901_234, tag: [7u8; 16] };
        let bytes = b.encode();
        assert_eq!(Beacon::decode(&bytes), Some(b));
        assert_eq!(Beacon::decode(&bytes[..BEACON_LEN - 1]), None);
        let mut bad = bytes;
        bad[0] = b'X';
        assert_eq!(Beacon::decode(&bad), None);
    }

    #[test]
    fn ifaces_are_private_ipv4() {
        for i in lan_ifaces() {
            assert!(is_lan_ip(i.ip, false), "{i:?}");
        }
    }

    #[tokio::test]
    async fn unicast_exchange() {
        let a = Discovery::bind(0, true).unwrap();
        let b = Discovery::bind(0, true).unwrap();
        let beacon = Beacon { want_reply: false, port: 1234, bucket: 1, tag: [9u8; 16] };
        a.send_to(&beacon.encode(), SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), b.port())).await;
        let mut buf = [0u8; 64];
        let (n, from) = b.recv(&mut buf).await.unwrap();
        assert_eq!(Beacon::decode(&buf[..n]), Some(beacon));
        assert_eq!(from.port(), a.port());
    }
}
