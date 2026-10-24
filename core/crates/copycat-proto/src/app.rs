use prost::alloc::{string::String, vec::Vec};

#[derive(Clone, PartialEq, ::prost::Message)]
pub struct AppMessage {
    #[prost(oneof = "app_message::Body", tags = "1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18")]
    pub body: ::core::option::Option<app_message::Body>,
}

pub mod app_message {
    use super::*;

    #[derive(Clone, PartialEq, ::prost::Oneof)]
    pub enum Body {
        #[prost(message, tag = "1")]
        Hello(Hello),
        #[prost(message, tag = "2")]
        SyncState(SyncState),
        #[prost(message, tag = "3")]
        ClipBatch(ClipBatch),
        #[prost(message, tag = "4")]
        ClipFetch(ClipFetch),
        #[prost(message, tag = "5")]
        ClipData(ClipData),
        #[prost(message, tag = "6")]
        RosterOps(RosterOps),
        #[prost(message, tag = "7")]
        LanHint(LanHint),
        #[prost(message, tag = "8")]
        Ping(Ping),
        #[prost(message, tag = "9")]
        Pong(Ping),
        #[prost(message, tag = "10")]
        FileOffer(FileOffer),
        #[prost(message, tag = "11")]
        FileAccept(FileAccept),
        #[prost(message, tag = "12")]
        FileChunk(FileChunk),
        #[prost(message, tag = "13")]
        FileChunkAck(FileChunkAck),
        #[prost(message, tag = "14")]
        FileCancel(FileCancel),

        #[prost(message, tag = "15")]
        Notice(Notice),
        #[prost(message, tag = "16")]
        FileDone(FileDone),
        #[prost(message, tag = "17")]
        HistoryClear(HistoryClear),
        #[prost(message, tag = "18")]
        RelayConfig(RelayConfig),
    }
}

impl AppMessage {
    pub fn new(body: app_message::Body) -> Self {
        Self { body: Some(body) }
    }
}

#[derive(Clone, PartialEq, ::prost::Message)]
pub struct Hello {
    #[prost(uint32, tag = "1")]
    pub protocol_version: u32,
    #[prost(string, tag = "2")]
    pub device_name: String,

    #[prost(bytes = "vec", tag = "3")]
    pub roster_digest: Vec<u8>,
    #[prost(string, repeated, tag = "4")]
    pub capabilities: Vec<String>,
}

#[derive(Clone, PartialEq, ::prost::Message)]
pub struct Hlc {
    #[prost(uint64, tag = "1")]
    pub wall_ms: u64,
    #[prost(uint32, tag = "2")]
    pub counter: u32,
    #[prost(bytes = "vec", tag = "3")]
    pub node: Vec<u8>,
}

#[derive(Clone, PartialEq, ::prost::Message)]
pub struct Watermark {
    #[prost(bytes = "vec", tag = "1")]
    pub origin: Vec<u8>,

    #[prost(uint64, tag = "2")]
    pub contiguous: u64,

    #[prost(uint64, repeated, tag = "3")]
    pub extra: Vec<u64>,
}

#[derive(Clone, PartialEq, ::prost::Message)]
pub struct SyncState {
    #[prost(message, repeated, tag = "1")]
    pub marks: Vec<Watermark>,

    #[prost(bool, tag = "2")]
    pub reply: bool,
}

#[derive(Clone, PartialEq, ::prost::Message)]
pub struct Representation {
    #[prost(string, tag = "1")]
    pub mime: String,
    #[prost(uint64, tag = "2")]
    pub size: u64,

    #[prost(bytes = "vec", tag = "3")]
    pub hash: Vec<u8>,

    #[prost(bytes = "vec", optional, tag = "4")]
    pub inline: Option<Vec<u8>>,
}

#[derive(Clone, PartialEq, ::prost::Message)]
pub struct ClipItem {
    #[prost(bytes = "vec", tag = "1")]
    pub id: Vec<u8>,
    #[prost(bytes = "vec", tag = "2")]
    pub origin: Vec<u8>,
    #[prost(uint64, tag = "3")]
    pub seq: u64,
    #[prost(message, optional, tag = "4")]
    pub hlc: Option<Hlc>,
    #[prost(message, repeated, tag = "5")]
    pub reps: Vec<Representation>,
}

#[derive(Clone, PartialEq, ::prost::Message)]
pub struct Advance {
    #[prost(bytes = "vec", tag = "1")]
    pub origin: Vec<u8>,
    #[prost(uint64, tag = "2")]
    pub upto: u64,
}

#[derive(Clone, PartialEq, ::prost::Message)]
pub struct ClipBatch {
    #[prost(message, repeated, tag = "1")]
    pub items: Vec<ClipItem>,
    #[prost(message, repeated, tag = "2")]
    pub advances: Vec<Advance>,
}

#[derive(Clone, PartialEq, ::prost::Message)]
pub struct ClipFetch {
    #[prost(bytes = "vec", tag = "1")]
    pub item_id: Vec<u8>,

    #[prost(uint32, tag = "2")]
    pub rep_index: u32,
    #[prost(uint64, tag = "3")]
    pub offset: u64,

    #[prost(uint32, repeated, tag = "4")]
    pub reps: Vec<u32>,
}

#[derive(Clone, PartialEq, ::prost::Message)]
pub struct ClipData {
    #[prost(bytes = "vec", tag = "1")]
    pub item_id: Vec<u8>,
    #[prost(uint32, tag = "2")]
    pub rep_index: u32,
    #[prost(uint64, tag = "3")]
    pub offset: u64,
    #[prost(bytes = "vec", tag = "4")]
    pub data: Vec<u8>,
    #[prost(bool, tag = "5")]
    pub last: bool,
}

#[derive(Clone, PartialEq, ::prost::Message)]
pub struct RosterOps {
    #[prost(bytes = "vec", repeated, tag = "1")]
    pub ops: Vec<Vec<u8>>,
}

#[derive(Clone, PartialEq, ::prost::Message)]
pub struct RelayConfig {
    #[prost(string, tag = "1")]
    pub addr: String,

    #[prost(bytes = "vec", tag = "2")]
    pub key: Vec<u8>,
    #[prost(uint64, tag = "3")]
    pub version: u64,

    #[prost(bytes = "vec", tag = "4")]
    pub author: Vec<u8>,
}

#[derive(Clone, PartialEq, ::prost::Message)]
pub struct LanHint {
    #[prost(string, repeated, tag = "1")]
    pub addrs: Vec<String>,
    #[prost(uint32, tag = "2")]
    pub port: u32,
}

#[derive(Clone, PartialEq, ::prost::Message)]
pub struct Ping {
    #[prost(uint64, tag = "1")]
    pub nonce: u64,
}

#[derive(Clone, PartialEq, ::prost::Message)]
pub struct FileEntry {
    #[prost(string, tag = "1")]
    pub name: String,
    #[prost(uint64, tag = "2")]
    pub size: u64,

    #[prost(bytes = "vec", tag = "3")]
    pub hash: Vec<u8>,
}

#[derive(Clone, PartialEq, ::prost::Message)]
pub struct FileOffer {
    #[prost(bytes = "vec", tag = "1")]
    pub transfer_id: Vec<u8>,
    #[prost(message, repeated, tag = "2")]
    pub files: Vec<FileEntry>,
    #[prost(uint32, tag = "3")]
    pub chunk_size: u32,

    #[prost(string, optional, tag = "4")]
    pub text: Option<String>,

    #[prost(uint32, tag = "5")]
    pub kind: u32,

    #[prost(bytes = "vec", tag = "6")]
    pub clip_item: Vec<u8>,
}

pub const FILE_KIND_FILES: u32 = 0;

pub const FILE_KIND_SCREENSHOT: u32 = 1;

pub const FILE_KIND_CLIP: u32 = 2;

#[derive(Clone, PartialEq, ::prost::Message)]
pub struct FileAccept {
    #[prost(bytes = "vec", tag = "1")]
    pub transfer_id: Vec<u8>,

    #[prost(bytes = "vec", repeated, tag = "2")]
    pub have: Vec<Vec<u8>>,
}

#[derive(Clone, PartialEq, ::prost::Message)]
pub struct FileChunk {
    #[prost(bytes = "vec", tag = "1")]
    pub transfer_id: Vec<u8>,
    #[prost(uint32, tag = "2")]
    pub file_index: u32,
    #[prost(uint64, tag = "3")]
    pub chunk_index: u64,
    #[prost(bytes = "vec", tag = "4")]
    pub data: Vec<u8>,

    #[prost(bytes = "vec", tag = "5")]
    pub hash: Vec<u8>,
}

#[derive(Clone, PartialEq, ::prost::Message)]
pub struct FileChunkAck {
    #[prost(bytes = "vec", tag = "1")]
    pub transfer_id: Vec<u8>,
    #[prost(uint32, tag = "2")]
    pub file_index: u32,
    #[prost(uint64, repeated, tag = "3")]
    pub chunk_indices: Vec<u64>,
}

#[derive(Clone, PartialEq, ::prost::Message)]
pub struct FileCancel {
    #[prost(bytes = "vec", tag = "1")]
    pub transfer_id: Vec<u8>,
    #[prost(string, tag = "2")]
    pub reason: String,
}

#[derive(Clone, PartialEq, ::prost::Message)]
pub struct FileDone {
    #[prost(bytes = "vec", tag = "1")]
    pub transfer_id: Vec<u8>,
}

#[derive(Clone, PartialEq, ::prost::Message)]
pub struct HistoryClear {
    #[prost(bool, tag = "1")]
    pub all: bool,

    #[prost(bytes = "vec", repeated, tag = "2")]
    pub items: Vec<Vec<u8>>,
}

#[derive(Clone, PartialEq, ::prost::Message)]
pub struct Notice {
    #[prost(string, tag = "1")]
    pub kind: String,
    #[prost(bytes = "vec", tag = "2")]
    pub payload: Vec<u8>,
}
