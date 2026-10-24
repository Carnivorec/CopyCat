pub mod aead;
mod error;
pub mod identity;
pub mod kdf;
pub mod mailbox;
pub mod pairing;
pub mod random;
pub mod record;
pub mod relay_auth;
pub mod roster;
pub mod session;
mod wire;

pub use error::CryptoError;
pub use identity::{DeviceId, DeviceIdentity};
pub use record::{DeviceRecord, Platform};
pub use roster::Roster;
pub use session::Session;

pub type Result<T, E = CryptoError> = core::result::Result<T, E>;
