use crate::Result;
use crate::identity::{self, DeviceIdentity};
use crate::kdf::{self, sig_context};

pub const EXPORTER_LABEL: &[u8] = b"EXPORTER-copycat-relay-v1";
pub const EXPORTER_LEN: usize = 32;

fn transcript(nonce: &[u8], relay_id: &[u8], channel_binding: &[u8]) -> Vec<u8> {
    let mut t = Vec::with_capacity(12 + nonce.len() + relay_id.len() + channel_binding.len());
    for part in [nonce, relay_id, channel_binding] {
        t.extend_from_slice(&(part.len() as u32).to_be_bytes());
        t.extend_from_slice(part);
    }
    t
}

pub fn sign(identity: &DeviceIdentity, nonce: &[u8], relay_id: &[u8], channel_binding: &[u8]) -> [u8; 64] {
    identity.sign(sig_context::RELAY_AUTH, &transcript(nonce, relay_id, channel_binding))
}

pub fn verify(sign_pub: &[u8; 32], nonce: &[u8], relay_id: &[u8], channel_binding: &[u8], sig: &[u8]) -> Result<()> {
    identity::verify(sign_pub, sig_context::RELAY_AUTH, &transcript(nonce, relay_id, channel_binding), sig)
}

pub fn pair_join_proof(relay_auth: &[u8; 32], nonce: &[u8], channel_binding: &[u8]) -> [u8; 32] {
    kdf::keyed_mac(relay_auth, &transcript(nonce, b"pair join", channel_binding))
}

pub fn verify_pair_join_proof(relay_auth: &[u8; 32], nonce: &[u8], channel_binding: &[u8], proof: &[u8]) -> bool {
    kdf::ct_eq(&pair_join_proof(relay_auth, nonce, channel_binding), proof)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn binding_matters() {
        let a = DeviceIdentity::generate().unwrap();
        let sig = sign(&a, b"nonce", b"relay", b"tls1");
        verify(&a.sign_pub(), b"nonce", b"relay", b"tls1", &sig).unwrap();
        assert!(verify(&a.sign_pub(), b"nonce", b"relay", b"tls2", &sig).is_err());
        assert!(verify(&a.sign_pub(), b"nonce2", b"relay", b"tls1", &sig).is_err());
        assert!(verify(&a.sign_pub(), b"nonce", b"relay2", b"tls1", &sig).is_err());

        assert!(verify(&a.sign_pub(), b"nonc", b"erelay", b"tls1", &sig).is_err());
    }

    #[test]
    fn pair_proof() {
        let k = [3u8; 32];
        let p = pair_join_proof(&k, b"n", b"b");
        assert!(verify_pair_join_proof(&k, b"n", b"b", &p));
        assert!(!verify_pair_join_proof(&k, b"n", b"c", &p));
        assert!(!verify_pair_join_proof(&[4u8; 32], b"n", b"b", &p));
    }
}
