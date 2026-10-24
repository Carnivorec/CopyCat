pub mod purpose {
    pub const PAIRING_PSK: &str = "pairing psk";
    pub const PAIRING_TICKET: &str = "pairing ticket";
    pub const PAIRING_RELAY_AUTH: &str = "pairing relay";
    pub const PAIRING_SAS: &str = "pairing sas";
    pub const DISCOVERY: &str = "lan discovery";
    pub const STORE_FIELD: &str = "store field";
}

pub mod sig_context {
    pub const DEVICE_RECORD: &str = "device record";
    pub const ROSTER_OP: &str = "roster op";
    pub const RELAY_AUTH: &str = "relay auth";
}

pub mod prologue {
    pub const SESSION: &[u8] = b"copycat v1 session";
    pub const PAIRING: &[u8] = b"copycat v1 pair";
    pub const MAILBOX: &[u8] = b"copycat v1 mailbox";
}

pub fn derive_key(purpose: &str, material: &[u8]) -> [u8; 32] {
    blake3::derive_key(&format!("copycat v1 {purpose}"), material)
}

pub fn hash(data: &[u8]) -> [u8; 32] {
    *blake3::hash(data).as_bytes()
}

pub fn keyed_mac(key: &[u8; 32], data: &[u8]) -> [u8; 32] {
    *blake3::keyed_hash(key, data).as_bytes()
}

pub fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

pub(crate) fn signing_message(context: &str, data: &[u8]) -> Vec<u8> {
    let mut m = Vec::with_capacity(16 + context.len() + 1 + data.len());
    m.extend_from_slice(b"copycat v1 sig ");
    m.extend_from_slice(context.as_bytes());
    m.push(0);
    m.extend_from_slice(data);
    m
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn purposes_are_independent() {
        let m = [42u8; 32];
        assert_ne!(derive_key(purpose::PAIRING_PSK, &m), derive_key(purpose::PAIRING_TICKET, &m));
        assert_ne!(derive_key(purpose::PAIRING_PSK, &m), derive_key(purpose::PAIRING_RELAY_AUTH, &m));
    }

    #[test]
    fn ct_eq_works() {
        assert!(ct_eq(b"abc", b"abc"));
        assert!(!ct_eq(b"abc", b"abd"));
        assert!(!ct_eq(b"abc", b"ab"));
    }
}
