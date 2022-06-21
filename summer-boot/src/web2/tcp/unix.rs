use super::{is_transient_error, ListenInfo};

use super::Listener;
use crate::{log, Server, http};

use std::fmt::{self, Display, Formatter};

use crate::tcp::ConnectionMode;
use async_std::os::unix::net::{self, SocketAddr, UnixStream};
use async_std::path::PathBuf;
use async_std::prelude::*;
use async_std::{io, task};

pub struct UnixListener<State> {
    path: Option<PathBuf>,
    listener: Option<net::UnixListener>,
    server: Option<Server<State>>,
    info: Option<ListenInfo>,
}

impl<State> UnixListener<State> {
    pub fn from_path(path: impl Into<PathBuf>) -> Self {
        Self {
            path: Some(path.into()),
            listener: None,
            server: None,
            info: None,
        }
    }

    pub fn from_listener(unix_listener: impl Into<net::UnixListener>) -> Self {
        Self {
            path: None,
            listener: Some(unix_listener.into()),
            server: None,
            info: None,
        }
    }
}

fn handle_h1_unix<State: Clone + Send + Sync + 'static>(app: Server<State>, stream: UnixStream) {
    task::spawn(async move {
        let local_addr = unix_socket_addr_to_string(stream.local_addr());
        let peer_addr = unix_socket_addr_to_string(stream.peer_addr());

        let fut = http::accept(stream, |mut req| async {
            req.set_local_addr(local_addr.as_ref());
            req.set_peer_addr(peer_addr.as_ref());
            app.respond(req).await
        });

        if let Err(error) = fut.await {
            log::error!("async-h1 error", { error: error.to_string() });
        }
    });
}

#[async_trait::async_trait]
impl<State> Listener<State> for UnixListener<State>
where
    State: Clone + Send + Sync + 'static,
{
    async fn bind(&mut self, server: Server<State>) -> io::Result<()> {
        assert!(self.server.is_none(), "`bind` 只能调用一次");
        self.server = Some(server);

        if self.listener.is_none() {
            let path = self.path.take().expect("`bind` 只能调用一次");
            let listener = net::UnixListener::bind(path).await?;
            self.listener = Some(listener);
        }

        // Format the listen information.
        let conn_string = format!("{}", self);
        let transport = "uds".to_owned();
        let tls = false;
        let conn_model = ConnectionMode::H1Only;
        self.info = Some(ListenInfo::new(conn_string, transport, tls, conn_model));

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
            .expect("`Listener::bind` 必须在之前设置 `UnixListener::Info`");

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
                    ConnectionMode::H1Only => handle_h1_unix(server.clone(), stream),
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

impl<State> fmt::Debug for UnixListener<State> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("UnixListener")
            .field("listener", &self.listener)
            .field("path", &self.path)
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

impl<State> Display for UnixListener<State> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match &self.listener {
            Some(listener) => {
                let path = listener.local_addr().expect("无法获取本地路径目录");
                let pathname = path
                    .as_pathname()
                    .and_then(|p| p.canonicalize().ok())
                    .expect("无法格式化路径目录");
                write!(f, "http+unix://{}", pathname.display())
            }
            None => match &self.path {
                Some(path) => write!(f, "http+unix://{}", path.display()),
                None => write!(f, "没有监听，请检查是否成功调用了 `Listener::bind`?"),
            },
        }
    }
}

fn unix_socket_addr_to_string(result: io::Result<SocketAddr>) -> Option<String> {
    result
        .ok()
        .as_ref()
        .and_then(SocketAddr::as_pathname)
        .and_then(|p| p.canonicalize().ok())
        .map(|pathname| format!("http+unix://{}", pathname.display()))
}
