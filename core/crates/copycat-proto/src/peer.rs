#[derive(Clone, PartialEq, ::prost::Message)]
pub struct PeerFrame {
    #[prost(fixed64, tag = "1")]
    pub session_id: u64,
    #[prost(oneof = "peer_frame::Kind", tags = "2, 3, 4, 5, 6, 7")]
    pub kind: ::core::option::Option<peer_frame::Kind>,
}

pub mod peer_frame {
    #[derive(Clone, PartialEq, ::prost::Oneof)]
    pub enum Kind {
        #[prost(bytes, tag = "2")]
        HandshakeInit(::prost::alloc::vec::Vec<u8>),

        #[prost(bytes, tag = "3")]
        HandshakeResp(::prost::alloc::vec::Vec<u8>),

        #[prost(bytes, tag = "4")]
        Data(::prost::alloc::vec::Vec<u8>),

        #[prost(bool, tag = "5")]
        Close(bool),

        #[prost(bytes, tag = "6")]
        PairInit(::prost::alloc::vec::Vec<u8>),

        #[prost(bytes, tag = "7")]
        PairResp(::prost::alloc::vec::Vec<u8>),
    }
}
