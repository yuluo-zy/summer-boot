use crate::Endpoint;
use bytes::{Bytes, BytesMut};
use futures_core::ready;
use h2::{
    server::{Connection, SendResponse},
    Ping, PingPong,
};
use http::{Extensions, Response};
use log::{error, trace, warn};
use pin_project_lite::pin_project;
use std::time::Instant;
use std::{
    cmp,
    error::Error as StdError,
    future::Future,
    marker::PhantomData,
    net,
    pin::Pin,
    rc::Rc,
    task::{Context, Poll},
};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::time::{sleep, Sleep};

use crate::web2::http2::{Payload, Service2Options};

const CHUNK_SIZE: usize = 16_384;
/// A collection of services that describe an HTTP request flow.
// pub(super) struct HttpFlow<S, X, U> {
//     pub(super) service: S,
//     pub(super) expect: X,
//     pub(super) upgrade: Option<U>,
// }

// impl<S, X, U> HttpFlow<S, X, U> {
//     pub(super) fn new(service: S, expect: X, upgrade: Option<U>) -> Rc<Self> {
//         Rc::new(Self {
//             service,
//             expect,
//             upgrade,
//         })
//     }
// }

struct H2PingPong {
    timer: Pin<Box<Sleep>>,
    on_flight: bool,
    ping_pong: PingPong,
}

pin_project! {
    /// Dispatcher for HTTP/2 protocol.
    pub struct Dispatcher<T, S, Fut, X, U> {
        // flow: Rc<HttpFlow<S, X, U>>,
        connection: Connection<T, Bytes>,
        conn_data: Option<Rc<Extensions>>,
        config: Service2Options,
        peer_addr: Option<net::SocketAddr>,
        ping_pong: Option<H2PingPong>,
        _phantom: PhantomData<Fut>
    }
}

impl<T, S, B, X, U> Dispatcher<T, S, B, X, U>
where
    T: AsyncRead + AsyncWrite + Unpin,
{
    pub(crate) fn new(
        mut conn: Connection<T, Bytes>,
        // flow: Rc<HttpFlow<S, X, U>>,
        config: Service2Options,
        peer_addr: Option<net::SocketAddr>,
        conn_data: Option<Extensions>,
        timer: Option<Pin<Box<Sleep>>>,
    ) -> Self {
        let ping_pong = config.keep_alive().duration().map(|dur| H2PingPong {
            timer: timer
                .map(|mut timer| {
                    // 如果已为握手初始化，则重用计时器
                    timer.as_mut().reset((Instant::now() + dur).into());
                    timer
                })
                .unwrap_or_else(|| Box::pin(sleep(dur))),
            on_flight: false,
            ping_pong: conn.ping_pong().unwrap(),
        });

        Self {
            // flow,
            config,
            peer_addr,
            connection: conn,
            conn_data: conn_data.0.map(Rc::new),
            ping_pong,
            _phantom: PhantomData,
        }
    }
}

enum DispatchError {
    SendResponse(h2::Error),
    SendData(h2::Error),
    ResponseBody(Box<dyn StdError>),
}

impl<T, S, B, X, U> Future for Dispatcher<T, S, B, X, U>
where
    T: AsyncRead + AsyncWrite + Unpin,
    // S: Service<Request>,
    // S::Error: Into<Response>,
    // S::Future: 'static,
    // S::Response: Into<Response>,
    //
    // B: MessageBody,
{
    type Output = Result<(), crate::web2::http2::error::DispatchError>;

    #[inline]
    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();

        loop {
            match Pin::new(&mut this.connection).poll_accept(cx)? {
                Poll::Ready(Some((req, tx))) => {
                    let (parts, body) = req.into_parts();
                    let payload = Payload::new(body);
                    // let mut req = Request::with_payload(pl);
                    // let heads = req.headers();
                    // let mut request = Request::new(req.method().into(), req.uri());
                    // for (key, value) in heads.iter() {
                    //     request.append_header(key, value);
                    // }
                    //
                    // // req.conn_data = this.conn_data.as_ref().map(Rc::clone);

                    // let fut = this.flow.service.call(request);
                    let config = this.config.clone();

                    // multiplex request handling with spawn task
                    tokio::task::spawn_local(
                        (async move {
                            // resolve service call and send response.
                            let res = match fut.await {
                                Ok(res) => handle_response(request.into(), tx, config).await,
                                Err(err) => {
                                    // 将错误转化为 http 类型
                                    let res: Response = err.into();
                                    handle_response(res, tx, config).await
                                }
                            };

                            // log error.
                            if let Err(err) = res {
                                match err {
                                    DispatchError::SendResponse(err) => {
                                        trace!("Error sending HTTP/2 response: {:?}", err)
                                    }
                                    DispatchError::SendData(err) => warn!("{:?}", err),
                                    DispatchError::ResponseBody(err) => {
                                        error!("Response payload stream error: {:?}", err)
                                    }
                                }
                            }
                        }),
                    )
                }
                Poll::Ready(None) => return Poll::Ready(Ok(())),
                Poll::Pending => match this.ping_pong.as_mut() {
                    Some(ping_pong) => loop {
                        if ping_pong.on_flight {
                            // When have on flight ping pong. poll pong and and keep alive timer.
                            // on success pong received update keep alive timer to determine the next timing of
                            // ping pong.
                            match ping_pong.ping_pong.poll_pong(cx)? {
                                Poll::Ready(_) => {
                                    ping_pong.on_flight = false;

                                    let dead_line = this.config.keep_alive_deadline().unwrap();
                                    ping_pong.timer.as_mut().reset(dead_line.into());
                                }
                                Poll::Pending => {
                                    return ping_pong.timer.as_mut().poll(cx).map(|_| Ok(()))
                                }
                            }
                        } else {
                            // When there is no on flight ping pong. keep alive timer is used to wait for next
                            // timing of ping pong. Therefore at this point it serves as an interval instead.
                            ready!(ping_pong.timer.as_mut().poll(cx));

                            ping_pong.ping_pong.send_ping(Ping::opaque())?;

                            let dead_line = this.config.keep_alive_deadline().unwrap();
                            ping_pong.timer.as_mut().reset(dead_line.into());

                            ping_pong.on_flight = true;
                        }
                    },
                    None => return Poll::Pending,
                },
            }
        }
    }
}

// async fn handle_response(
//     mut res: Response,
//     mut tx: SendResponse<Bytes>,
//     config: Service2Config,
// ) -> Result<(), DispatchError> {
//     // 转换成 http::Response
//     let body = res.replace_body(());
//
//
//     // prepare response.
//     let mut size = body.len();
//     let res = prepare_response(config, res, size);
//     let eof = size.is_eof();
//
//     // send response head and return on eof.
//     let mut stream = tx
//         .send_response(res, eof)
//         .map_err(DispatchError::SendResponse)?;
//
//     if eof {
//         return Ok(());
//     }
//
//     // poll response body and send chunks to client
//     actix_rt::pin!(body);
//
//     while let Some(res) = poll_fn(|cx| body.as_mut().poll_next(cx)).await {
//         let mut chunk = res.map_err(|err| DispatchError::ResponseBody(err.into()))?;
//
//         'send: loop {
//             let chunk_size = cmp::min(chunk.len(), CHUNK_SIZE);
//
//             // reserve enough space and wait for stream ready.
//             stream.reserve_capacity(chunk_size);
//
//             match poll_fn(|cx| stream.poll_capacity(cx)).await {
//                 // No capacity left. drop body and return.
//                 None => return Ok(()),
//
//                 Some(Err(err)) => return Err(DispatchError::SendData(err)),
//
//                 Some(Ok(cap)) => {
//                     // split chunk to writeable size and send to client
//                     let len = chunk.len();
//                     let bytes = chunk.split_to(cmp::min(len, cap));
//
//                     stream
//                         .send_data(bytes, false)
//                         .map_err(DispatchError::SendData)?;
//
//                     // Current chuck completely sent. break send loop and poll next one.
//                     if chunk.is_empty() {
//                         break 'send;
//                     }
//                 }
//             }
//         }
//     }
//
//     // response body streaming finished. send end of stream and return.
//     stream
//         .send_data(Bytes::new(), true)
//         .map_err(DispatchError::SendData)?;
//
//     Ok(())
// }
//
// fn prepare_response(config: Service2Config, req: Response, size: &mut Option<usize>) ->  {
//     let mut has_date = false;
//     let mut skip_len = size.is_none();
//
//     let mut res = Response::new(req.status());
//     res.set_version(Some(Http2_0));
//
//     match req.status() {
//         StatusCode::NoContent | StatusCode::Continue => *size = None,
//         // todo
//         StatusCode::SwitchingProtocols => {
//             skip_len = true;
//             *size = None;
//         }
//         _ => {}
//     }
//
//     let _ = match size {
//         BodySize::None | BodySize::Stream => None,
//
//         BodySize::Sized(0) => {
//             #[allow(clippy::declare_interior_mutable_const)]
//             const HV_ZERO: HeaderValue = HeaderValue::from_static("0");
//             res.headers_mut().insert(CONTENT_LENGTH, HV_ZERO)
//         }
//
//         BodySize::Sized(len) => {
//             let mut buf = itoa::Buffer::new();
//
//             res.headers_mut().insert(
//                 CONTENT_LENGTH,
//                 HeaderValue::from_str(buf.format(*len)).unwrap(),
//             )
//         }
//     };
//
//     // copy headers
//     for (key, value) in head.headers.iter() {
//         match key {
//             // omit HTTP/1.x only headers according to:
//             // https://datatracker.ietf.org/doc/html/rfc7540#section-8.1.2.2
//             &CONNECTION | &TRANSFER_ENCODING | &UPGRADE => continue,
//
//             &CONTENT_LENGTH if skip_len => continue,
//             &DATE => has_date = true,
//
//             // omit HTTP/1.x only headers according to:
//             // https://datatracker.ietf.org/doc/html/rfc7540#section-8.1.2.2
//             hdr if hdr == HeaderName::from_static("keep-alive")
//                 || hdr == HeaderName::from_static("proxy-connection") =>
//             {
//                 continue
//             }
//
//             _ => {}
//         }
//
//         res.headers_mut().append(key, value.clone());
//     }
//
//     // set date header
//     if !has_date {
//         let mut bytes = BytesMut::with_capacity(29);
//         config.write_date_header_value(&mut bytes);
//         res.headers_mut().insert(
//             DATE,
//             // SAFETY: serialized date-times are known ASCII strings
//             unsafe { HeaderValue::from_maybe_shared_unchecked(bytes.freeze()) },
//         );
//     }
//
//     res
// }
