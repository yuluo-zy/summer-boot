use super::{is_transient_error, ListenInfo};

use super::Listener;
use crate::{log, Server};

use std::fmt::{self, Debug, Display, Formatter};

use crate::tcp::ConnectionMode;
use async_std::net::{self, SocketAddr, TcpStream};
use async_std::prelude::*;
use async_std::{io, task};
use h2::server as http2server;
use tokio::net::{TcpListener as TokioTcpListener, TcpStream as TokioTcpStream};

pub struct TcpListener<State, T>
where T: From<std::net::TcpListener>
{
    addrs: Option<Vec<SocketAddr>>,
    listener: Option<net::TcpListener>,
    server: Option<Server<State>>,
    info: Option<ListenInfo>,
}

impl<State, T> TcpListener<State, T> where T: From<std::net::TcpListener>{
    pub fn from_addrs(addrs: Vec<SocketAddr>) -> Self {
        Self {
            addrs: Some(addrs),
            listener: None,
            server: None,
            info: None,
        }
    }

    pub fn from_listener(tcp_listener: impl Into<net::TcpListener>) -> Self {
        Self {
            addrs: None,
            listener: Some(tcp_listener.into()),
            server: None,
            info: None,
        }
    }
}

fn handle_h1_tcp<State: Clone + Send + Sync + 'static>(app: Server<State>, stream: TcpStream) {
    task::spawn(async move {
        let local_addr = stream.local_addr().ok();
        let peer_addr = stream.peer_addr().ok();

        let fut = async_h1::accept(stream, |mut req| async {
            req.set_local_addr(local_addr);
            req.set_peer_addr(peer_addr);
            app.respond(req).await
        });

        if let Err(error) = fut.await {
            log::error!("async-h1 error", { error: error.to_string() });
        }
    });
}

fn handle_h2_tcp<State: Clone + Send + Sync + 'static>(app: Server<State>, stream: std::net::TcpStream) {
    tokio::spawn(async {
        let mut h2 = http2server::handshake(TokioTcpStream::from_std(stream)).await.unwrap();
        while let Some(req) = h2.accept().await {
            app.respond(req).await
        }
    });
}

#[async_trait::async_trait]
impl<State,T> Listener<State> for TcpListener<State, T>
where
    State: Clone + Send + Sync + 'static,
    T: From<std::net::TcpListener>
{
    async fn bind(&mut self, server: Server<State>) -> io::Result<()> {
        assert!(self.server.is_none(), "`bind`只能调用一次");
        self.server = Some(server);

        // Format the listen information.
        let conn_string = format!("{}", self);
        let transport = "tcp".to_owned();
        let tls = false;
        let conn_model = ConnectionMode::H1Only;
        self.info = Some(ListenInfo::new(conn_string, transport, tls, conn_model));
        if self.listener.is_none() {
            let addrs = self.addrs.take().expect("`bind` 只能调用一次");
            let listener =  match conn_model {
                ConnectionMode::H1Only => AsyncTcpListener::bind(addrs.as_slice()).await?,
                _ => TokioTcpListener::bind(addrs.as_slice()).await?
            };
            self.listener = Some(listener);
        }
        Ok(())
    }

    async fn accept(&mut self) -> io::Result<()> {
        let server = self
            .server
            .take()
            .expect("`Listener::bind` 必须在之前调用 `Listener::accept`");
        let listener = self
            .listener
            .take()
            .expect("`Listener::bind` 必须在之前调用 `Listener::accept`");

        let mut incoming = listener.incoming();

        let info = self
            .info
            .take()
            .expect("`Listener::bind` 必须在之前设置 `TcpListener::Info`");

        while let Some(stream) = incoming.next().await {
            match stream {
                Err(ref e) if is_transient_error(e) => continue,
                Err(error) => {
                    let delay = std::time::Duration::from_millis(500);
                    crate::log::error!("Error: {}. for {:?}.", error, delay);
                    task::sleep(delay).await;
                    continue;
                }

                Ok(stream) => match info.conn_model {
                    ConnectionMode::H1Only => handle_h1_tcp(server.clone(), stream),
                    ConnectionMode::H2Only => panic!("尚未实现"),
                    ConnectionMode::Fallback => panic!("尚未实现"),
                },
            };
        }
        Ok(())
    }

    fn info(&self) -> Vec<ListenInfo> {
        match &self.info {
            Some(info) => vec![info.clone()],
            None => vec![],
        }
    }
}

impl<State, T> fmt::Debug for TcpListener<State, T>where  T: From<std::net::TcpListener> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("TcpListener")
            .field("listener", &self.listener)
            .field("addrs", &self.addrs)
            .field(
                "server",
                if self.server.is_some() {
                    &"Some(Server<State>)"
                } else {
                    &"None"
                },
            )
            .finish()
    }
}

impl<State, T> Display for TcpListener<State, T> where T: From<std::net::TcpListener> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        let http_fmt = |a| format!("http://{}", a);
        match &self.listener {
            Some(listener) => {
                let addr = listener.local_addr().expect("无法获取本地地址");
                write!(f, "{}", http_fmt(&addr))
            }
            None => match &self.addrs {
                Some(addrs) => {
                    let addrs = addrs.iter().map(http_fmt).collect::<Vec<_>>().join(", ");
                    write!(f, "{}", addrs)
                }
                None => write!(f, "没有监听，请检查是否成功调用了 `Listener::bind`?"),
            },
        }
    }
}
