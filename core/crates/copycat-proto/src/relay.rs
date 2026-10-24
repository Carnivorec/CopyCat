use prost::alloc::{string::String, vec::Vec};

#[derive(Clone, PartialEq, ::prost::Message)]
pub struct ClientMessage {
    #[prost(oneof = "client_message::Body", tags = "1, 2, 3, 4, 5, 6, 7, 8, 9")]
    pub body: ::core::option::Option<client_message::Body>,
}

pub mod client_message {
    use super::*;

    #[derive(Clone, PartialEq, ::prost::Oneof)]
    pub enum Body {
        #[prost(message, tag = "1")]
        Auth(Auth),
        #[prost(message, tag = "2")]
        Send(Send),
        #[prost(message, tag = "3")]
        MailboxPut(MailboxPut),
        #[prost(message, tag = "4")]
        MailboxAck(MailboxAck),
        #[prost(message, tag = "5")]
        RosterPush(RosterPush),
        #[prost(message, tag = "6")]
        PairRegister(PairRegister),
        #[prost(message, tag = "7")]
        PairJoin(PairJoin),
        #[prost(message, tag = "8")]
        Ping(Ping),
        #[prost(message, tag = "9")]
        MailboxFetch(MailboxFetch),
    }
}

#[derive(Clone, PartialEq, ::prost::Message)]
pub struct ServerMessage {
    #[prost(oneof = "server_message::Body", tags = "1, 2, 3, 4, 5, 6, 7, 8, 9")]
    pub body: ::core::option::Option<server_message::Body>,
}

pub mod server_message {
    use super::*;

    #[derive(Clone, PartialEq, ::prost::Oneof)]
    pub enum Body {
        #[prost(message, tag = "1")]
        Challenge(Challenge),
        #[prost(message, tag = "2")]
        AuthResult(AuthResult),
        #[prost(message, tag = "3")]
        Deliver(Deliver),
        #[prost(message, tag = "4")]
        Presence(Presence),
        #[prost(message, tag = "5")]
        MailboxItems(MailboxItems),
        #[prost(message, tag = "6")]
        RosterState(RosterState),
        #[prost(message, tag = "7")]
        PairAccepted(PairAccepted),
        #[prost(message, tag = "8")]
        Error(Error),
        #[prost(message, tag = "9")]
        Pong(Ping),
    }
}

impl ClientMessage {
    pub fn new(body: client_message::Body) -> Self {
        Self { body: Some(body) }
    }
}

impl ServerMessage {
    pub fn new(body: server_message::Body) -> Self {
        Self { body: Some(body) }
    }
}

#[derive(Clone, PartialEq, ::prost::Message)]
pub struct Challenge {
    #[prost(bytes = "vec", tag = "1")]
    pub nonce: Vec<u8>,

    #[prost(bytes = "vec", tag = "2")]
    pub relay_id: Vec<u8>,
}

#[derive(Clone, PartialEq, ::prost::Message)]
pub struct Auth {
    #[prost(bytes = "vec", tag = "1")]
    pub device_id: Vec<u8>,
    #[prost(bytes = "vec", tag = "2")]
    pub signature: Vec<u8>,

    #[prost(bytes = "vec", repeated, tag = "4")]
    pub roster_ops: Vec<Vec<u8>>,
}

#[derive(Clone, PartialEq, ::prost::Message)]
pub struct AuthResult {
    #[prost(bool, tag = "1")]
    pub ok: bool,
    #[prost(string, tag = "2")]
    pub reason: String,

    #[prost(bytes = "vec", repeated, tag = "3")]
    pub online: Vec<Vec<u8>>,
}

#[derive(Clone, PartialEq, ::prost::Message)]
pub struct Send {
    #[prost(bytes = "vec", tag = "1")]
    pub to: Vec<u8>,
    #[prost(bytes = "vec", tag = "2")]
    pub payload: Vec<u8>,
}

#[derive(Clone, PartialEq, ::prost::Message)]
pub struct Deliver {
    #[prost(bytes = "vec", tag = "1")]
    pub from: Vec<u8>,
    #[prost(bytes = "vec", tag = "2")]
    pub payload: Vec<u8>,
}

#[derive(Clone, PartialEq, ::prost::Message)]
pub struct Presence {
    #[prost(bytes = "vec", tag = "1")]
    pub device: Vec<u8>,
    #[prost(bool, tag = "2")]
    pub online: bool,
}

#[derive(Clone, PartialEq, ::prost::Message)]
pub struct MailboxPut {
    #[prost(bytes = "vec", tag = "1")]
    pub to: Vec<u8>,
    #[prost(bytes = "vec", tag = "2")]
    pub msg_id: Vec<u8>,
    #[prost(bytes = "vec", tag = "3")]
    pub payload: Vec<u8>,
    #[prost(uint32, tag = "4")]
    pub ttl_secs: u32,
}

#[derive(Clone, PartialEq, ::prost::Message)]
pub struct MailboxFetch {}

#[derive(Clone, PartialEq, ::prost::Message)]
pub struct MailboxItem {
    #[prost(bytes = "vec", tag = "1")]
    pub from: Vec<u8>,
    #[prost(bytes = "vec", tag = "2")]
    pub msg_id: Vec<u8>,
    #[prost(bytes = "vec", tag = "3")]
    pub payload: Vec<u8>,
}

#[derive(Clone, PartialEq, ::prost::Message)]
pub struct MailboxItems {
    #[prost(message, repeated, tag = "1")]
    pub items: Vec<MailboxItem>,
}

#[derive(Clone, PartialEq, ::prost::Message)]
pub struct MailboxAck {
    #[prost(bytes = "vec", repeated, tag = "1")]
    pub msg_ids: Vec<Vec<u8>>,
}

#[derive(Clone, PartialEq, ::prost::Message)]
pub struct RosterPush {
    #[prost(bytes = "vec", repeated, tag = "1")]
    pub ops: Vec<Vec<u8>>,
}

#[derive(Clone, PartialEq, ::prost::Message)]
pub struct RosterState {
    #[prost(bytes = "vec", repeated, tag = "1")]
    pub ops: Vec<Vec<u8>>,
}

#[derive(Clone, PartialEq, ::prost::Message)]
pub struct PairRegister {
    #[prost(bytes = "vec", tag = "1")]
    pub ticket_id: Vec<u8>,

    #[prost(bytes = "vec", tag = "2")]
    pub relay_auth: Vec<u8>,
    #[prost(uint32, tag = "3")]
    pub ttl_secs: u32,
}

#[derive(Clone, PartialEq, ::prost::Message)]
pub struct PairJoin {
    #[prost(bytes = "vec", tag = "1")]
    pub ticket_id: Vec<u8>,

    #[prost(bytes = "vec", tag = "2")]
    pub proof: Vec<u8>,
}

#[derive(Clone, PartialEq, ::prost::Message)]
pub struct PairAccepted {
    #[prost(bytes = "vec", tag = "1")]
    pub peer: Vec<u8>,

    #[prost(bytes = "vec", tag = "2")]
    pub temp_id: Vec<u8>,
}

#[derive(Clone, PartialEq, ::prost::Message)]
pub struct Error {
    #[prost(uint32, tag = "1")]
    pub code: u32,
    #[prost(string, tag = "2")]
    pub message: String,

    #[prost(bytes = "vec", tag = "3")]
    pub context: Vec<u8>,
}

#[derive(Clone, PartialEq, ::prost::Message)]
pub struct Ping {
    #[prost(uint64, tag = "1")]
    pub nonce: u64,
}

pub mod error_code {
    pub const BAD_REQUEST: u32 = 1;
    pub const UNAUTHORIZED: u32 = 2;
    pub const RATE_LIMITED: u32 = 3;
    pub const PEER_OFFLINE: u32 = 4;
    pub const QUOTA_EXCEEDED: u32 = 5;
    pub const REVOKED: u32 = 6;
}
