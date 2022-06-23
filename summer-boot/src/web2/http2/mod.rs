//!
//! http2 处理模块
//!
pub mod config;
mod dispatcher;
pub mod error;
mod service;
mod service_;

use std::{
    future::Future,
    pin::Pin,
    task::{Context, Poll},
};

use crate::web2::http2::config::Service2Options;
use crate::web2::http2::error::{DispatchError, PayloadError};
use bytes::Bytes;
use futures_core::{ready, Stream};
use h2::{
    server::{handshake, Connection, Handshake},
    RecvStream,
};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::time::{sleep_until, Sleep};

/// HTTP/2 peer stream.
pub struct Payload {
    stream: RecvStream,
}

impl Payload {
    pub(crate) fn new(stream: RecvStream) -> Self {
        Self { stream }
    }
}

impl Stream for Payload {
    type Item = Result<Bytes, PayloadError>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();

        match ready!(Pin::new(&mut this.stream).poll_data(cx)) {
            Some(Ok(chunk)) => {
                let len = chunk.len();

                match this.stream.flow_control().release_capacity(len) {
                    Ok(()) => Poll::Ready(Some(Ok(chunk))),
                    Err(err) => Poll::Ready(Some(Err(err.into()))),
                }
            }
            Some(Err(err)) => Poll::Ready(Some(Err(err.into()))),
            None => Poll::Ready(None),
        }
    }
}

pub(crate) fn handshake_with_timeout<T>(io: T, config: &Service2Options) -> HandshakeWithTimeout<T>
where
    T: AsyncRead + AsyncWrite + Unpin,
{
    HandshakeWithTimeout {
        handshake: handshake(io),
        timer: config
            .client_request_deadline()
            .map(|deadline| Box::pin(sleep_until(deadline.into()))),
    }
}

pub(crate) struct HandshakeWithTimeout<T: AsyncRead + AsyncWrite + Unpin> {
    handshake: Handshake<T>,
    timer: Option<Pin<Box<Sleep>>>,
}

impl<T> Future for HandshakeWithTimeout<T>
where
    T: AsyncRead + AsyncWrite + Unpin,
{
    type Output = Result<(Connection<T, Bytes>, Option<Pin<Box<Sleep>>>), DispatchError>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();

        match Pin::new(&mut this.handshake).poll(cx)? {
            //返回成功握手的计时器；它可以重新用于 h2 ping-pong
            Poll::Ready(conn) => Poll::Ready(Ok((conn, this.timer.take()))),
            Poll::Pending => match this.timer.as_mut() {
                Some(timer) => {
                    ready!(timer.as_mut().poll(cx));
                    Poll::Ready(Err(DispatchError::SlowRequestTimeout))
                }
                None => Poll::Pending,
            },
        }
    }
}
