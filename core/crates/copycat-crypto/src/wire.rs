use prost::alloc::{string::String, vec::Vec};

#[derive(Clone, PartialEq, ::prost::Message)]
pub struct RecordBody {
    #[prost(bytes = "vec", tag = "1")]
    pub sign_pub: Vec<u8>,
    #[prost(bytes = "vec", tag = "2")]
    pub dh_pub: Vec<u8>,
    #[prost(string, tag = "3")]
    pub name: String,
    #[prost(uint32, tag = "4")]
    pub platform: u32,
    #[prost(uint64, tag = "5")]
    pub created_at_ms: u64,
}

#[derive(Clone, PartialEq, ::prost::Message)]
pub struct SignedRecord {
    #[prost(bytes = "vec", tag = "1")]
    pub body: Vec<u8>,
    #[prost(bytes = "vec", tag = "2")]
    pub sig: Vec<u8>,
}

#[derive(Clone, PartialEq, ::prost::Message)]
pub struct OpBody {
    #[prost(bytes = "vec", tag = "1")]
    pub group: Vec<u8>,
    #[prost(uint64, tag = "2")]
    pub issued_at_ms: u64,
    #[prost(oneof = "op_body::Kind", tags = "3, 4")]
    pub kind: Option<op_body::Kind>,
}

pub mod op_body {
    use super::*;

    #[derive(Clone, PartialEq, ::prost::Oneof)]
    pub enum Kind {
        #[prost(bytes, tag = "3")]
        Add(Vec<u8>),
        #[prost(message, tag = "4")]
        Remove(RemoveOp),
    }
}

#[derive(Clone, PartialEq, ::prost::Message)]
pub struct RemoveOp {
    #[prost(bytes = "vec", tag = "1")]
    pub target: Vec<u8>,

    #[prost(bytes = "vec", repeated, tag = "2")]
    pub grandfathered: Vec<Vec<u8>>,
}

#[derive(Clone, PartialEq, ::prost::Message)]
pub struct SignedOp {
    #[prost(bytes = "vec", tag = "1")]
    pub body: Vec<u8>,
    #[prost(bytes = "vec", tag = "2")]
    pub signer: Vec<u8>,
    #[prost(bytes = "vec", tag = "3")]
    pub sig: Vec<u8>,
}
