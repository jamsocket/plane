use std::fmt::{Debug, Display};
use trust_dns_server::client::op::ResponseCode;

#[derive(Debug)]
pub struct DnsError {
    pub code: ResponseCode,
    pub message: String,
}

impl Display for DnsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}", self)
    }
}

pub type Result<T> = std::result::Result<T, DnsError>;

impl std::error::Error for DnsError {}

pub trait OrDnsError<T, F>
where
    F: FnOnce() -> String,
{
    fn or_dns_error(self, code: ResponseCode, message: F) -> Result<T>;
}

impl<T, F> OrDnsError<T, F> for Option<T>
where
    F: FnOnce() -> String,
{
    fn or_dns_error(self, code: ResponseCode, message: F) -> Result<T> {
        match self {
            Some(v) => Ok(v),
            None => Err(DnsError {
                code,
                message: message(),
            }),
        }
    }
}

impl<T, E, F> OrDnsError<T, F> for std::result::Result<T, E>
where
    E: Debug,
    F: FnOnce() -> String,
{
    fn or_dns_error(self, code: ResponseCode, message: F) -> Result<T> {
        match self {
            Ok(v) => Ok(v),
            Err(_) => Err(DnsError {
                code,
                message: message(),
            }),
        }
    }
}
