//! 表示HTTP传输和绑定的类型
use crate::Server;

mod concurrent;
mod failover;
mod parsed;
mod tcp_listener;
mod to_listener;
mod to_listener_impls;
mod unix;
mod http2;
mod h2;

use std::fmt::{write, Debug, Display, Formatter};

use async_std::io;
use async_trait::async_trait;

pub use concurrent::ConcurrentListener;
pub use failover::FailoverListener;
pub use to_listener::ToListener;

pub(crate) use parsed::ParsedListener;
pub(crate) use tcp_listener::TcpListener;
pub(crate) use unix::UnixListener;

#[macro_export]
macro_rules! read_to_end {
    ($expr:expr) => {
        match $expr {
            Poll::Ready(Ok(0)) => (),
            other => return other,
        }
    };
}

#[async_trait]
pub trait Listener<State>: Debug + Display + Send + Sync + 'static
where
    State: Send + Sync + 'static,
{
    async fn bind(&mut self, app: Server<State>) -> io::Result<()>;

    async fn accept(&mut self) -> io::Result<()>;

    fn info(&self) -> Vec<ListenInfo>;
}

#[async_trait]
impl<L, State> Listener<State> for Box<L>
where
    L: Listener<State>,
    State: Send + Sync + 'static,
{
    async fn bind(&mut self, app: Server<State>) -> io::Result<()> {
        self.as_mut().bind(app).await
    }

    async fn accept(&mut self) -> io::Result<()> {
        self.as_mut().accept().await
    }

    fn info(&self) -> Vec<ListenInfo> {
        self.as_ref().info()
    }
}

/// tcp和unix侦听器使用的crate内部共享逻辑
/// io::Error 触发是否需要回退延迟
/// types不需要延迟
pub(crate) fn is_transient_error(e: &io::Error) -> bool {
    use io::ErrorKind::*;

    matches!(
        e.kind(),
        ConnectionRefused | ConnectionAborted | ConnectionReset
    )
}

#[derive(Debug, Clone)]
pub struct ListenInfo {
    conn_string: String,
    transport: String,
    tls: bool,
    conn_model: ConnectionMode,
}

impl ListenInfo {
    pub fn new(
        conn_string: String,
        transport: String,
        tls: bool,
        conn_model: ConnectionMode,
    ) -> Self {
        Self {
            conn_string,
            transport,
            tls,
            conn_model,
        }
    }

    pub fn connection(&self) -> &str {
        self.conn_string.as_str()
    }

    pub fn transport(&self) -> &str {
        self.transport.as_str()
    }

    pub fn is_encrypted(&self) -> bool {
        self.tls
    }
}

impl Display for ListenInfo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.conn_string)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum ConnectionMode {
    /// 始终使用 HTTP/1，并且在发生解析错误时不要升级
    H1Only,
    /// 始终使用 HTTP/2
    H2Only,
    /// 使用 HTTP/1, 并且允许进行 HTTP/2 升级
    Fallback,
}
impl Default for ConnectionMode {
    fn default() -> Self {
        ConnectionMode::H1Only
    }
}

impl Display for ConnectionMode {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            ConnectionMode::Fallback => write!(f, "Fallback"),
            ConnectionMode::H2Only => write!(f, "H2Only"),
            ConnectionMode::H1Only => write!(f, "H1Only"),
        }
    }
}
