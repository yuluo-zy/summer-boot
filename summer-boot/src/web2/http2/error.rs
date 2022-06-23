use std::{error::Error, fmt, io, str::Utf8Error, string::FromUtf8Error};
use std::fmt::{Display, Formatter};
use http_types::Response;
use http_types::url::ParseError;
use derive_more::{Display, Error, From};

#[derive(Debug, Display)]
#[non_exhaustive]
pub enum PayloadError {
    /// A payload reached EOF, but is not complete.
    #[display(
    fmt = "A payload reached EOF, but is not complete. Inner error: {:?}",
    _0
    )]
    Incomplete(Option<io::Error>),

    /// Content encoding stream corruption.
    #[display(fmt = "Can not decode content-encoding.")]
    EncodingCorrupted,

    /// Payload reached size limit.
    #[display(fmt = "Payload reached size limit.")]
    Overflow,

    /// Payload length is unknown.
    #[display(fmt = "Payload length is unknown.")]
    UnknownLength,

    /// HTTP/2 payload error.
    #[display(fmt = "{}", _0)]
    Http2Payload(::h2::Error),

    /// Generic I/O error.
    #[display(fmt = "{}", _0)]
    Io(io::Error),
}

impl std::error::Error for PayloadError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            PayloadError::Incomplete(None) => None,
            PayloadError::Incomplete(Some(err)) => Some(err),
            PayloadError::EncodingCorrupted => None,
            PayloadError::Overflow => None,
            PayloadError::UnknownLength => None,
            PayloadError::Http2Payload(err) => Some(err),
            PayloadError::Io(err) => Some(err),
        }
    }
}

impl From<h2::Error> for PayloadError {
    fn from(err: h2::Error) -> Self {
        PayloadError::Http2Payload(err)
    }
}

impl From<Option<io::Error>> for PayloadError {
    fn from(err: Option<io::Error>) -> Self {
        PayloadError::Incomplete(err)
    }
}

impl From<io::Error> for PayloadError {
    fn from(err: io::Error) -> Self {
        PayloadError::Incomplete(Some(err))
    }
}


#[derive(Debug, Display, From)]
#[non_exhaustive]
pub enum DispatchError {
    /// Service error.
    #[display(fmt = "Service Error")]
    Service(Response),

    /// Body streaming error.
    #[display(fmt = "Body error: {}", _0)]
    Body(Box<dyn std::error::Error>),

    /// Upgrade service error.
    Upgrade,

    /// An `io::Error` that occurred while trying to read or write to a network stream.
    #[display(fmt = "IO error: {}", _0)]
    Io(io::Error),

    /// Request parse error.
    #[display(fmt = "Request parse error: {}", _0)]
    Parse(ParseError),

    /// HTTP/2 error.
    #[display(fmt = "{}", _0)]
    H2(h2::Error),

    /// The first request did not complete within the specified timeout.
    #[display(fmt = "The first request did not complete within the specified timeout")]
    SlowRequestTimeout,

    /// Disconnect timeout. Makes sense for ssl streams.
    #[display(fmt = "Connection shutdown timeout")]
    DisconnectTimeout,

    /// Handler dropped payload before reading EOF.
    #[display(fmt = "Handler dropped payload before reading EOF")]
    HandlerDroppedPayload,

    /// Internal error.
    #[display(fmt = "Internal error")]
    InternalError,
}


impl std::error::Error for DispatchError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            DispatchError::Service(_res) => None,
            DispatchError::Body(err) => Some(&**err),
            DispatchError::Io(err) => Some(err),
            DispatchError::Parse(err) => Some(err),
            DispatchError::H2(err) => Some(err),

            _ => None,
        }
    }
}