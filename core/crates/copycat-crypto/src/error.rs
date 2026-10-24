use thiserror::Error;

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum CryptoError {
    #[error("签名无效")]
    BadSignature,
    #[error("密钥格式无效")]
    BadKey,
    #[error("Noise 协议错误: {0}")]
    Noise(String),
    #[error("解密或认证失败")]
    Decrypt,
    #[error("数据格式错误: {0}")]
    Malformed(&'static str),
    #[error("消息过大")]
    TooLarge,
    #[error("设备不在名册中")]
    NotMember,
    #[error("设备已被吊销")]
    Revoked,
    #[error("名册操作无效: {0}")]
    InvalidOp(&'static str),
    #[error("配对邀请已过期")]
    Expired,
    #[error("系统随机数源不可用")]
    Rng,
}

impl From<snow::Error> for CryptoError {
    fn from(e: snow::Error) -> Self {
        match e {
            snow::Error::Decrypt => CryptoError::Decrypt,
            other => CryptoError::Noise(other.to_string()),
        }
    }
}

impl From<prost::DecodeError> for CryptoError {
    fn from(_: prost::DecodeError) -> Self {
        CryptoError::Malformed("protobuf 解码失败")
    }
}
