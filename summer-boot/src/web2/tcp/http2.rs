// use core::task;
// use std::future::Future;
// use std::pin::Pin;
// use std::task::Poll;
// use h2::{RecvStream, SendStream};
// use http::HeaderMap;
// use http::header::{HeaderName, CONNECTION, TE, TRAILER, TRANSFER_ENCODING, UPGRADE};
// use log::{info, warn, debug, trace};
// use pin_project_lite::pin_project;
// use bytes::{Buf, Bytes};
// use futures_core::ready;
// use std::io::{self, Cursor, IoSlice};
// use std::sync::{Arc, Mutex};
// use futures_util::future::Shared;
// use h2::server::{Connection, Handshake};
// use tokio::io::{AsyncRead, AsyncWrite};
//
// /// Default initial stream window size defined in HTTP2 spec.
// pub(crate) const SPEC_WINDOW_SIZE: u32 = 65_535;
//
// fn strip_connection_headers(headers: &mut HeaderMap, is_request: bool) {
//     // List of connection headers from:
//     // https://developer.mozilla.org/en-US/docs/Web/HTTP/Headers/Connection
//     //
//     // TE headers are allowed in HTTP/2 requests as long as the value is "trailers", so they're
//     // tested separately.
//     let connection_headers = [
//         HeaderName::from_lowercase(b"keep-alive").unwrap(),
//         HeaderName::from_lowercase(b"proxy-connection").unwrap(),
//         TRAILER,
//         TRANSFER_ENCODING,
//         UPGRADE,
//     ];
//
//     for header in connection_headers.iter() {
//         if headers.remove(header).is_some() {
//             warn!("Connection header illegal in HTTP/2: {}", header.as_str());
//         }
//     }
//
//     if is_request {
//         if headers
//             .get(TE)
//             .map(|te_header| te_header != "trailers")
//             .unwrap_or(false)
//         {
//             warn!("TE headers not set to \"trailers\" are illegal in HTTP/2 requests");
//             headers.remove(TE);
//         }
//     } else if headers.remove(TE).is_some() {
//         warn!("TE headers illegal in HTTP/2 responses");
//     }
//
//     if let Some(header) = headers.remove(CONNECTION) {
//         warn!(
//             "Connection header illegal in HTTP/2: {}",
//             CONNECTION.as_str()
//         );
//         let header_contents = header.to_str().unwrap();
//
//         // A `Connection` header may have a comma-separated list of names of other headers that
//         // are meant for only this specific connection.
//         //
//         // Iterate these names and remove them as headers. Connection-specific headers are
//         // forbidden in HTTP2, as that information has been moved into frame types of the h2
//         // protocol.
//         for name in header_contents.split(',') {
//             let name = name.trim();
//             headers.remove(name);
//         }
//     }
// }
//
// // body adapters used by both Client and Server
//
// pin_project! {
//     struct PipeToSendStream
//     {
//         body_tx: SendStream<Bytes>,
//         data_done: bool,
//         #[pin]
//         stream: S,
//     }
// }
//
// impl<S> PipeToSendStream
// {
//     fn new(stream: S, tx: SendStream<Bytes>) -> PipeToSendStream {
//         PipeToSendStream {
//             body_tx: tx,
//             data_done: false,
//             stream,
//         }
//     }
// }
//
// impl<S> Future for PipeToSendStream
// {
//     type Output = crate::Result<()>;
//
//     fn poll(self: Pin<&mut Self>, cx: &mut task::Context<'_>) -> Poll<Self::Output> {
//         let mut me = self.project();
//         loop {
//             if !*me.data_done {
//                 // we don't have the next chunk of data yet, so just reserve 1 byte to make
//                 // sure there's some capacity available. h2 will handle the capacity management
//                 // for the actual body chunk.
//                 me.body_tx.reserve_capacity(1);
//
//                 if me.body_tx.capacity() == 0 {
//                     loop {
//                         match ready!(me.body_tx.poll_capacity(cx)) {
//                             Some(Ok(0)) => {}
//                             Some(Ok(_)) => break,
//                             Some(Err(e)) => {
//                                 // return Poll::Ready(Err(crate::Error::new_body_write(e)))
//                                 return Poll::Ready()
//                             }
//                             None => {
//                                 // None means the stream is no longer in a
//                                 // streaming state, we either finished it
//                                 // somehow, or the remote reset us.
//                                 return Poll::Ready(Err(
//                                     "send stream capacity unexpectedly closed",
//                                 ));
//                             }
//                         }
//                     }
//                 }
//                 else if let Poll::Ready(reason) = me
//                     .body_tx
//                     .poll_reset(cx)
//                     .map_err(crate::Error::new_body_write)?
//                 {
//                     debug!("stream received RST_STREAM: {:?}", reason);
//                     return Poll::Ready(Err(crate::Error::new_body_write(::h2::Error::from(
//                         reason,
//                     ))));
//                 }
//
//                 match ready!(me.stream.as_mut().poll_data(cx)) {
//                     Some(Ok(chunk)) => {
//                         let is_eos = me.stream.is_end_stream();
//                         trace!(
//                             "send body chunk: {} bytes, eos={}",
//                             chunk.remaining(),
//                             is_eos,
//                         );
//
//                         let buf = SendBuf::Buf(chunk);
//                         me.body_tx
//                             .send_data(buf, is_eos)
//                             .map_err(crate::Error::new_body_write)?;
//
//                         if is_eos {
//                             return Poll::Ready(Ok(()));
//                         }
//                     }
//                     Some(Err(e)) => return Poll::Ready(Err(me.body_tx.on_user_err(e))),
//                     None => {
//                         me.body_tx.reserve_capacity(0);
//                         let is_eos = me.stream.is_end_stream();
//                         if is_eos {
//                             return Poll::Ready(me.body_tx.send_eos_frame());
//                         } else {
//                             *me.data_done = true;
//                             // loop again to poll_trailers
//                         }
//                     }
//                 }
//             } else {
//                 if let Poll::Ready(reason) = me
//                     .body_tx
//                     .poll_reset(cx)
//                     .map_err(crate::Error::new_body_write)?
//                 {
//                     debug!("stream received RST_STREAM: {:?}", reason);
//                     return Poll::Ready(Err(crate::Error::new_body_write(::h2::Error::from(
//                         reason,
//                     ))));
//                 }
//
//                 match ready!(me.stream.poll_trailers(cx)) {
//                     Ok(Some(trailers)) => {
//                         me.body_tx
//                             .send_trailers(trailers)
//                             .map_err(crate::Error::new_body_write)?;
//                         return Poll::Ready(Ok(()));
//                     }
//                     Ok(None) => {
//                         // There were no trailers, so send an empty DATA frame...
//                         return Poll::Ready(me.body_tx.send_eos_frame());
//                     }
//                     Err(e) => return Poll::Ready(Err(me.body_tx.on_user_err(e))),
//                 }
//             }
//         }
//     }
// }
//
// trait SendStreamExt {
//     fn on_user_err<E>(&mut self, err: E) -> crate::Error
//         where
//             E: Into<Box<dyn std::error::Error + Send + Sync>>;
//     fn send_eos_frame(&mut self) -> crate::Result<()>;
// }
//
// impl<B: Buf> SendStreamExt for SendStream<SendBuf<B>> {
//     fn on_user_err<E>(&mut self, err: E) -> crate::Error
//         where
//             E: Into<Box<dyn std::error::Error + Send + Sync>>,
//     {
//         let err = crate::Error::new_user_body(err);
//         debug!("send body user stream error: {}", err);
//         self.send_reset(err.h2_reason());
//         err
//     }
//
//     fn send_eos_frame(&mut self) -> crate::Result<()> {
//         trace!("send body eos");
//         self.send_data(SendBuf::None, true)
//             .map_err(crate::Error::new_body_write)
//     }
// }
//
// #[repr(usize)]
// enum SendBuf<B> {
//     Buf(B),
//     Cursor(Cursor<Box<[u8]>>),
//     None,
// }
//
// impl<B: Buf> Buf for SendBuf<B> {
//     #[inline]
//     fn remaining(&self) -> usize {
//         match *self {
//             Self::Buf(ref b) => b.remaining(),
//             Self::Cursor(ref c) => Buf::remaining(c),
//             Self::None => 0,
//         }
//     }
//
//     #[inline]
//     fn chunk(&self) -> &[u8] {
//         match *self {
//             Self::Buf(ref b) => b.chunk(),
//             Self::Cursor(ref c) => c.chunk(),
//             Self::None => &[],
//         }
//     }
//
//     #[inline]
//     fn advance(&mut self, cnt: usize) {
//         match *self {
//             Self::Buf(ref mut b) => b.advance(cnt),
//             Self::Cursor(ref mut c) => c.advance(cnt),
//             Self::None => {}
//         }
//     }
//
//     fn chunks_vectored<'a>(&'a self, dst: &mut [IoSlice<'a>]) -> usize {
//         match *self {
//             Self::Buf(ref b) => b.chunks_vectored(dst),
//             Self::Cursor(ref c) => c.chunks_vectored(dst),
//             Self::None => 0,
//         }
//     }
// }
// #[derive(Clone)]
// pub(crate) struct Recorder {
//     shared: Option<Arc<Mutex<Shared>>>,
// }
// struct H2Upgraded<B>
//     where
//         B: Buf,
// {
//     ping: Recorder,
//     send_stream: UpgradedSendStream<B>,
//     recv_stream: RecvStream,
//     buf: Bytes,
// }
//
// impl<B> AsyncRead for H2Upgraded<B>
//     where
//         B: Buf,
// {
//     fn poll_read(
//         mut self: Pin<&mut Self>,
//         cx: &mut Context<'_>,
//         read_buf: &mut ReadBuf<'_>,
//     ) -> Poll<Result<(), io::Error>> {
//         if self.buf.is_empty() {
//             self.buf = loop {
//                 match ready!(self.recv_stream.poll_data(cx)) {
//                     None => return Poll::Ready(Ok(())),
//                     Some(Ok(buf)) if buf.is_empty() && !self.recv_stream.is_end_stream() => {
//                         continue
//                     }
//                     Some(Ok(buf)) => {
//                         self.ping.record_data(buf.len());
//                         break buf;
//                     }
//                     Some(Err(e)) => {
//                         return Poll::Ready(match e.reason() {
//                             Some(Reason::NO_ERROR) | Some(Reason::CANCEL) => Ok(()),
//                             Some(Reason::STREAM_CLOSED) => {
//                                 Err(io::Error::new(io::ErrorKind::BrokenPipe, e))
//                             }
//                             _ => Err(h2_to_io_error(e)),
//                         })
//                     }
//                 }
//             };
//         }
//         let cnt = std::cmp::min(self.buf.len(), read_buf.remaining());
//         read_buf.put_slice(&self.buf[..cnt]);
//         self.buf.advance(cnt);
//         let _ = self.recv_stream.flow_control().release_capacity(cnt);
//         Poll::Ready(Ok(()))
//     }
// }
//
// impl<B> AsyncWrite for H2Upgraded<B>
//     where
//         B: Buf,
// {
//     fn poll_write(
//         mut self: Pin<&mut Self>,
//         cx: &mut Context<'_>,
//         buf: &[u8],
//     ) -> Poll<Result<usize, io::Error>> {
//         if buf.is_empty() {
//             return Poll::Ready(Ok(0));
//         }
//         self.send_stream.reserve_capacity(buf.len());
//
//         // We ignore all errors returned by `poll_capacity` and `write`, as we
//         // will get the correct from `poll_reset` anyway.
//         let cnt = match ready!(self.send_stream.poll_capacity(cx)) {
//             None => Some(0),
//             Some(Ok(cnt)) => self
//                 .send_stream
//                 .write(&buf[..cnt], false)
//                 .ok()
//                 .map(|()| cnt),
//             Some(Err(_)) => None,
//         };
//
//         if let Some(cnt) = cnt {
//             return Poll::Ready(Ok(cnt));
//         }
//
//         Poll::Ready(Err(h2_to_io_error(
//             match ready!(self.send_stream.poll_reset(cx)) {
//                 Ok(Reason::NO_ERROR) | Ok(Reason::CANCEL) | Ok(Reason::STREAM_CLOSED) => {
//                     return Poll::Ready(Err(io::ErrorKind::BrokenPipe.into()))
//                 }
//                 Ok(reason) => reason.into(),
//                 Err(e) => e,
//             },
//         )))
//     }
//
//     fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Result<(), io::Error>> {
//         Poll::Ready(Ok(()))
//     }
//
//     fn poll_shutdown(
//         mut self: Pin<&mut Self>,
//         cx: &mut Context<'_>,
//     ) -> Poll<Result<(), io::Error>> {
//         if self.send_stream.write(&[], true).is_ok() {
//             return Poll::Ready(Ok(()))
//         }
//
//         Poll::Ready(Err(h2_to_io_error(
//             match ready!(self.send_stream.poll_reset(cx)) {
//                 Ok(Reason::NO_ERROR) => {
//                     return Poll::Ready(Ok(()))
//                 }
//                 Ok(Reason::CANCEL) | Ok(Reason::STREAM_CLOSED) => {
//                     return Poll::Ready(Err(io::ErrorKind::BrokenPipe.into()))
//                 }
//                 Ok(reason) => reason.into(),
//                 Err(e) => e,
//             },
//         )))
//     }
// }
//
// fn h2_to_io_error(e: h2::Error) -> io::Error {
//     if e.is_io() {
//         e.into_io().unwrap()
//     } else {
//         io::Error::new(io::ErrorKind::Other, e)
//     }
// }
//
// struct UpgradedSendStream<B>(SendStream<SendBuf<Neutered<B>>>);
//
// impl<B> UpgradedSendStream<B>
//     where
//         B: Buf,
// {
//     unsafe fn new(inner: SendStream<SendBuf<B>>) -> Self {
//         assert_eq!(mem::size_of::<B>(), mem::size_of::<Neutered<B>>());
//         Self(mem::transmute(inner))
//     }
//
//     fn reserve_capacity(&mut self, cnt: usize) {
//         unsafe { self.as_inner_unchecked().reserve_capacity(cnt) }
//     }
//
//     fn poll_capacity(&mut self, cx: &mut Context<'_>) -> Poll<Option<Result<usize, h2::Error>>> {
//         unsafe { self.as_inner_unchecked().poll_capacity(cx) }
//     }
//
//     fn poll_reset(&mut self, cx: &mut Context<'_>) -> Poll<Result<h2::Reason, h2::Error>> {
//         unsafe { self.as_inner_unchecked().poll_reset(cx) }
//     }
//
//     fn write(&mut self, buf: &[u8], end_of_stream: bool) -> Result<(), io::Error> {
//         let send_buf = SendBuf::Cursor(Cursor::new(buf.into()));
//         unsafe {
//             self.as_inner_unchecked()
//                 .send_data(send_buf, end_of_stream)
//                 .map_err(h2_to_io_error)
//         }
//     }
//
//     unsafe fn as_inner_unchecked(&mut self) -> &mut SendStream<SendBuf<B>> {
//         &mut *(&mut self.0 as *mut _ as *mut _)
//     }
// }
//
// #[repr(transparent)]
// struct Neutered<B> {
//     _inner: B,
//     impossible: Impossible,
// }
//
// enum Impossible {}
//
// unsafe impl<B> Send for Neutered<B> {}
//
// impl<B> Buf for Neutered<B> {
//     fn remaining(&self) -> usize {
//         match self.impossible {}
//     }
//
//     fn chunk(&self) -> &[u8] {
//         match self.impossible {}
//     }
//
//     fn advance(&mut self, _cnt: usize) {
//         match self.impossible {}
//     }
// }
//
//
// const DEFAULT_CONN_WINDOW: u32 = 1024 * 1024; // 1mb
// const DEFAULT_STREAM_WINDOW: u32 = 1024 * 1024; // 1mb
// const DEFAULT_MAX_FRAME_SIZE: u32 = 1024 * 16; // 16kb
// const DEFAULT_MAX_SEND_BUF_SIZE: usize = 1024 * 400; // 400kb
// // 16 MB "sane default" taken from golang http2
// const DEFAULT_SETTINGS_MAX_HEADER_LIST_SIZE: u32 = 16 << 20;
//
// #[derive(Clone, Debug)]
// pub(crate) struct Config {
//     pub(crate) adaptive_window: bool,
//     pub(crate) initial_conn_window_size: u32,
//     pub(crate) initial_stream_window_size: u32,
//     pub(crate) max_frame_size: u32,
//     pub(crate) enable_connect_protocol: bool,
//     pub(crate) max_concurrent_streams: Option<u32>,
//     #[cfg(feature = "runtime")]
//     pub(crate) keep_alive_interval: Option<Duration>,
//     #[cfg(feature = "runtime")]
//     pub(crate) keep_alive_timeout: Duration,
//     pub(crate) max_send_buffer_size: usize,
//     pub(crate) max_header_list_size: u32,
// }
//
// impl Default for Config {
//     fn default() -> Config {
//         Config {
//             adaptive_window: false,
//             initial_conn_window_size: DEFAULT_CONN_WINDOW,
//             initial_stream_window_size: DEFAULT_STREAM_WINDOW,
//             max_frame_size: DEFAULT_MAX_FRAME_SIZE,
//             enable_connect_protocol: false,
//             max_concurrent_streams: None,
//             #[cfg(feature = "runtime")]
//             keep_alive_interval: None,
//             #[cfg(feature = "runtime")]
//             keep_alive_timeout: Duration::from_secs(20),
//             max_send_buffer_size: DEFAULT_MAX_SEND_BUF_SIZE,
//             max_header_list_size: DEFAULT_SETTINGS_MAX_HEADER_LIST_SIZE,
//         }
//     }
// }
//
// pin_project! {
//     pub(crate) struct Server<T, S, B, E>
//     where
//         S: HttpService<Body>,
//         B: HttpBody,
//     {
//         exec: E,
//         service: S,
//         state: State<T, B>,
//     }
// }
//
// enum State<T, B>
//     where
//         B: HttpBody,
// {
//     Handshaking {
//         ping_config: ping::Config,
//         hs: Handshake<T, SendBuf<B::Data>>,
//     },
//     Serving(Serving<T, B>),
//     Closed,
// }
//
// struct Serving<T, B>
//     where
//         B: HttpBody,
// {
//     ping: Option<(ping::Recorder, ping::Ponger)>,
//     conn: Connection<T, SendBuf<B::Data>>,
//     closing: Option<crate::Error>,
// }
//
// impl<T, S, B, E> Server<T, S, B, E>
//     where
//         T: AsyncRead + AsyncWrite + Unpin,
//         S: HttpService<Body, ResBody = B>,
//         S::Error: Into<Box<dyn StdError + Send + Sync>>,
//         B: HttpBody + 'static,
//         E: ConnStreamExec<S::Future, B>,
// {
//     pub(crate) fn new(io: T, service: S, config: &Config, exec: E) -> Server<T, S, B, E> {
//         let mut builder = h2::server::Builder::default();
//         builder
//             .initial_window_size(config.initial_stream_window_size)
//             .initial_connection_window_size(config.initial_conn_window_size)
//             .max_frame_size(config.max_frame_size)
//             .max_header_list_size(config.max_header_list_size)
//             .max_send_buffer_size(config.max_send_buffer_size);
//         if let Some(max) = config.max_concurrent_streams {
//             builder.max_concurrent_streams(max);
//         }
//         if config.enable_connect_protocol {
//             builder.enable_connect_protocol();
//         }
//         let handshake = builder.handshake(io);
//
//         let bdp = if config.adaptive_window {
//             Some(config.initial_stream_window_size)
//         } else {
//             None
//         };
//
//         let ping_config = ping::Config {
//             bdp_initial_window: bdp,
//             #[cfg(feature = "runtime")]
//             keep_alive_interval: config.keep_alive_interval,
//             #[cfg(feature = "runtime")]
//             keep_alive_timeout: config.keep_alive_timeout,
//             // If keep-alive is enabled for servers, always enabled while
//             // idle, so it can more aggressively close dead connections.
//             #[cfg(feature = "runtime")]
//             keep_alive_while_idle: true,
//         };
//
//         Server {
//             exec,
//             state: State::Handshaking {
//                 ping_config,
//                 hs: handshake,
//             },
//             service,
//         }
//     }
//
//     pub(crate) fn graceful_shutdown(&mut self) {
//         trace!("graceful_shutdown");
//         match self.state {
//             State::Handshaking { .. } => {
//                 // fall-through, to replace state with Closed
//             }
//             State::Serving(ref mut srv) => {
//                 if srv.closing.is_none() {
//                     srv.conn.graceful_shutdown();
//                 }
//                 return;
//             }
//             State::Closed => {
//                 return;
//             }
//         }
//         self.state = State::Closed;
//     }
// }
//
// impl<T, S, B, E> Future for Server<T, S, B, E>
//     where
//         T: AsyncRead + AsyncWrite + Unpin,
//         S: HttpService<Body, ResBody = B>,
//         S::Error: Into<Box<dyn StdError + Send + Sync>>,
//         B: HttpBody + 'static,
//         E: ConnStreamExec<S::Future, B>,
// {
//     type Output = crate::Result<Dispatched>;
//
//     fn poll(mut self: Pin<&mut Self>, cx: &mut task::Context<'_>) -> Poll<Self::Output> {
//         let me = &mut *self;
//         loop {
//             let next = match me.state {
//                 State::Handshaking {
//                     ref mut hs,
//                     ref ping_config,
//                 } => {
//                     let mut conn = ready!(Pin::new(hs).poll(cx).map_err(crate::Error::new_h2))?;
//                     let ping = if ping_config.is_enabled() {
//                         let pp = conn.ping_pong().expect("conn.ping_pong");
//                         Some(ping::channel(pp, ping_config.clone()))
//                     } else {
//                         None
//                     };
//                     State::Serving(Serving {
//                         ping,
//                         conn,
//                         closing: None,
//                     })
//                 }
//                 State::Serving(ref mut srv) => {
//                     ready!(srv.poll_server(cx, &mut me.service, &mut me.exec))?;
//                     return Poll::Ready(Ok(Dispatched::Shutdown));
//                 }
//                 State::Closed => {
//                     // graceful_shutdown was called before handshaking finished,
//                     // nothing to do here...
//                     return Poll::Ready(Ok(Dispatched::Shutdown));
//                 }
//             };
//             me.state = next;
//         }
//     }
// }
//
// impl<T, B> Serving<T, B>
//     where
//         T: AsyncRead + AsyncWrite + Unpin,
//         B: HttpBody + 'static,
// {
//     fn poll_server<S, E>(
//         &mut self,
//         cx: &mut task::Context<'_>,
//         service: &mut S,
//         exec: &mut E,
//     ) -> Poll<crate::Result<()>>
//         where
//             S: HttpService<Body, ResBody = B>,
//             S::Error: Into<Box<dyn StdError + Send + Sync>>,
//             E: ConnStreamExec<S::Future, B>,
//     {
//         if self.closing.is_none() {
//             loop {
//                 self.poll_ping(cx);
//
//                 // Check that the service is ready to accept a new request.
//                 //
//                 // - If not, just drive the connection some.
//                 // - If ready, try to accept a new request from the connection.
//                 match service.poll_ready(cx) {
//                     Poll::Ready(Ok(())) => (),
//                     Poll::Pending => {
//                         // use `poll_closed` instead of `poll_accept`,
//                         // in order to avoid accepting a request.
//                         ready!(self.conn.poll_closed(cx).map_err(crate::Error::new_h2))?;
//                         trace!("incoming connection complete");
//                         return Poll::Ready(Ok(()));
//                     }
//                     Poll::Ready(Err(err)) => {
//                         let err = crate::Error::new_user_service(err);
//                         debug!("service closed: {}", err);
//
//                         let reason = err.h2_reason();
//                         if reason == Reason::NO_ERROR {
//                             // NO_ERROR is only used for graceful shutdowns...
//                             trace!("interpreting NO_ERROR user error as graceful_shutdown");
//                             self.conn.graceful_shutdown();
//                         } else {
//                             trace!("abruptly shutting down with {:?}", reason);
//                             self.conn.abrupt_shutdown(reason);
//                         }
//                         self.closing = Some(err);
//                         break;
//                     }
//                 }
//
//                 // When the service is ready, accepts an incoming request.
//                 match ready!(self.conn.poll_accept(cx)) {
//                     Some(Ok((req, mut respond))) => {
//                         trace!("incoming request");
//                         let content_length = headers::content_length_parse_all(req.headers());
//                         let ping = self
//                             .ping
//                             .as_ref()
//                             .map(|ping| ping.0.clone())
//                             .unwrap_or_else(ping::disabled);
//
//                         // Record the headers received
//                         ping.record_non_data();
//
//                         let is_connect = req.method() == Method::CONNECT;
//                         let (mut parts, stream) = req.into_parts();
//                         let (mut req, connect_parts) = if !is_connect {
//                             (
//                                 Request::from_parts(
//                                     parts,
//                                     crate::Body::h2(stream, content_length.into(), ping),
//                                 ),
//                                 None,
//                             )
//                         } else {
//                             if content_length.map_or(false, |len| len != 0) {
//                                 warn!("h2 connect request with non-zero body not supported");
//                                 respond.send_reset(h2::Reason::INTERNAL_ERROR);
//                                 return Poll::Ready(Ok(()));
//                             }
//                             let (pending, upgrade) = crate::upgrade::pending();
//                             debug_assert!(parts.extensions.get::<OnUpgrade>().is_none());
//                             parts.extensions.insert(upgrade);
//                             (
//                                 Request::from_parts(parts, crate::Body::empty()),
//                                 Some(ConnectParts {
//                                     pending,
//                                     ping,
//                                     recv_stream: stream,
//                                 }),
//                             )
//                         };
//
//                         if let Some(protocol) = req.extensions_mut().remove::<h2::ext::Protocol>() {
//                             req.extensions_mut().insert(Protocol::from_inner(protocol));
//                         }
//
//                         let fut = H2Stream::new(service.call(req), connect_parts, respond);
//                         exec.execute_h2stream(fut);
//                     }
//                     Some(Err(e)) => {
//                         return Poll::Ready(Err(crate::Error::new_h2(e)));
//                     }
//                     None => {
//                         // no more incoming streams...
//                         if let Some((ref ping, _)) = self.ping {
//                             ping.ensure_not_timed_out()?;
//                         }
//
//                         trace!("incoming connection complete");
//                         return Poll::Ready(Ok(()));
//                     }
//                 }
//             }
//         }
//
//         debug_assert!(
//             self.closing.is_some(),
//             "poll_server broke loop without closing"
//         );
//
//         ready!(self.conn.poll_closed(cx).map_err(crate::Error::new_h2))?;
//
//         Poll::Ready(Err(self.closing.take().expect("polled after error")))
//     }
//
//     fn poll_ping(&mut self, cx: &mut task::Context<'_>) {
//         if let Some((_, ref mut estimator)) = self.ping {
//             match estimator.poll(cx) {
//                 Poll::Ready(ping::Ponged::SizeUpdate(wnd)) => {
//                     self.conn.set_target_window_size(wnd);
//                     let _ = self.conn.set_initial_window_size(wnd);
//                 }
//                 #[cfg(feature = "runtime")]
//                 Poll::Ready(ping::Ponged::KeepAliveTimedOut) => {
//                     debug!("keep-alive timed out, closing connection");
//                     self.conn.abrupt_shutdown(h2::Reason::NO_ERROR);
//                 }
//                 Poll::Pending => {}
//             }
//         }
//     }
// }
//
// pin_project! {
//     #[allow(missing_debug_implementations)]
//     pub struct H2Stream<F, B>
//     where
//         B: HttpBody,
//     {
//         reply: SendResponse<SendBuf<B::Data>>,
//         #[pin]
//         state: H2StreamState<F, B>,
//     }
// }
//
// pin_project! {
//     #[project = H2StreamStateProj]
//     enum H2StreamState<F, B>
//     where
//         B: HttpBody,
//     {
//         Service {
//             #[pin]
//             fut: F,
//             connect_parts: Option<ConnectParts>,
//         },
//         Body {
//             #[pin]
//             pipe: PipeToSendStream<B>,
//         },
//     }
// }
//
// struct ConnectParts {
//     pending: Pending,
//     ping: Recorder,
//     recv_stream: RecvStream,
// }
//
// impl<F, B> H2Stream<F, B>
//     where
//         B: HttpBody,
// {
//     fn new(
//         fut: F,
//         connect_parts: Option<ConnectParts>,
//         respond: SendResponse<SendBuf<B::Data>>,
//     ) -> H2Stream<F, B> {
//         H2Stream {
//             reply: respond,
//             state: H2StreamState::Service { fut, connect_parts },
//         }
//     }
// }
//
// macro_rules! reply {
//     ($me:expr, $res:expr, $eos:expr) => {{
//         match $me.reply.send_response($res, $eos) {
//             Ok(tx) => tx,
//             Err(e) => {
//                 debug!("send response error: {}", e);
//                 $me.reply.send_reset(Reason::INTERNAL_ERROR);
//                 return Poll::Ready(Err(crate::Error::new_h2(e)));
//             }
//         }
//     }};
// }
//
// impl<F, B, E> H2Stream<F, B>
//     where
//         F: Future<Output = Result<Response<B>, E>>,
//         B: HttpBody,
//         B::Data: 'static,
//         B::Error: Into<Box<dyn StdError + Send + Sync>>,
//         E: Into<Box<dyn StdError + Send + Sync>>,
// {
//     fn poll2(self: Pin<&mut Self>, cx: &mut task::Context<'_>) -> Poll<crate::Result<()>> {
//         let mut me = self.project();
//         loop {
//             let next = match me.state.as_mut().project() {
//                 H2StreamStateProj::Service {
//                     fut: h,
//                     connect_parts,
//                 } => {
//                     let res = match h.poll(cx) {
//                         Poll::Ready(Ok(r)) => r,
//                         Poll::Pending => {
//                             // Response is not yet ready, so we want to check if the client has sent a
//                             // RST_STREAM frame which would cancel the current request.
//                             if let Poll::Ready(reason) =
//                             me.reply.poll_reset(cx).map_err(crate::Error::new_h2)?
//                             {
//                                 debug!("stream received RST_STREAM: {:?}", reason);
//                                 return Poll::Ready(Err(crate::Error::new_h2(reason.into())));
//                             }
//                             return Poll::Pending;
//                         }
//                         Poll::Ready(Err(e)) => {
//                             let err = crate::Error::new_user_service(e);
//                             warn!("http2 service errored: {}", err);
//                             me.reply.send_reset(err.h2_reason());
//                             return Poll::Ready(Err(err));
//                         }
//                     };
//
//                     let (head, body) = res.into_parts();
//                     let mut res = ::http::Response::from_parts(head, ());
//                     super::strip_connection_headers(res.headers_mut(), false);
//
//                     // set Date header if it isn't already set...
//                     res.headers_mut()
//                         .entry(::http::header::DATE)
//                         .or_insert_with(date::update_and_header_value);
//
//                     if let Some(connect_parts) = connect_parts.take() {
//                         if res.status().is_success() {
//                             if headers::content_length_parse_all(res.headers())
//                                 .map_or(false, |len| len != 0)
//                             {
//                                 warn!("h2 successful response to CONNECT request with body not supported");
//                                 me.reply.send_reset(h2::Reason::INTERNAL_ERROR);
//                                 return Poll::Ready(Err(crate::Error::new_user_header()));
//                             }
//                             let send_stream = reply!(me, res, false);
//                             connect_parts.pending.fulfill(Upgraded::new(
//                                 H2Upgraded {
//                                     ping: connect_parts.ping,
//                                     recv_stream: connect_parts.recv_stream,
//                                     send_stream: unsafe { UpgradedSendStream::new(send_stream) },
//                                     buf: Bytes::new(),
//                                 },
//                                 Bytes::new(),
//                             ));
//                             return Poll::Ready(Ok(()));
//                         }
//                     }
//
//
//                     if !body.is_end_stream() {
//                         // automatically set Content-Length from body...
//                         if let Some(len) = body.size_hint().exact() {
//                             headers::set_content_length_if_missing(res.headers_mut(), len);
//                         }
//
//                         let body_tx = reply!(me, res, false);
//                         H2StreamState::Body {
//                             pipe: PipeToSendStream::new(body, body_tx),
//                         }
//                     } else {
//                         reply!(me, res, true);
//                         return Poll::Ready(Ok(()));
//                     }
//                 }
//                 H2StreamStateProj::Body { pipe } => {
//                     return pipe.poll(cx);
//                 }
//             };
//             me.state.set(next);
//         }
//     }
// }
//
// impl<F, B, E> Future for H2Stream<F, B>
//     where
//         F: Future<Output = Result<Response<B>, E>>,
//         B: HttpBody,
//         B::Data: 'static,
//         B::Error: Into<Box<dyn StdError + Send + Sync>>,
//         E: Into<Box<dyn StdError + Send + Sync>>,
// {
//     type Output = ();
//
//     fn poll(self: Pin<&mut Self>, cx: &mut task::Context<'_>) -> Poll<Self::Output> {
//         self.poll2(cx).map(|res| {
//             if let Err(e) = res {
//                 debug!("stream error: {}", e);
//             }
//         })
//     }
// }
//
