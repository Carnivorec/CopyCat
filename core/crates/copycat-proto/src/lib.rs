pub mod app;
pub mod frame;
pub mod peer;
pub mod relay;

pub use prost::Message;

pub const PROTOCOL_VERSION: u32 = 1;

pub const DEVICE_ID_LEN: usize = 16;
