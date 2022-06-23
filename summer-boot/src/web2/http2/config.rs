use std::{
    net,
    rc::Rc,
    time::{Duration, Instant},
};

use bytes::BytesMut;
/// HTTP service configuration.
#[derive(Debug, Clone)]
pub struct Service2Options(Rc<Inner>);

#[derive(Debug)]
struct Inner {
    keep_alive: KeepAlive,
    client_request_timeout: Duration,
    client_disconnect_timeout: Duration,
}

impl Default for Service2Options {
    fn default() -> Self {
        Self::new(KeepAlive::default(), Duration::from_secs(5), Duration::ZERO)
    }
}

impl Service2Options {
    /// Create instance of `ServiceConfig`
    pub fn new(
        keep_alive: KeepAlive,
        client_request_timeout: Duration,
        client_disconnect_timeout: Duration,
    ) -> Service2Options {
        Service2Options(Rc::new(Inner {
            keep_alive: keep_alive.normalize(),
            client_request_timeout,
            client_disconnect_timeout,
        }))
    }

    /// 连接保持活动设置。
    #[inline]
    pub fn keep_alive(&self) -> KeepAlive {
        self.0.keep_alive
    }

    ///创建一个时间对象，表示此连接的保活期的截止日期，如果启用了 [`KeepAlive::Os`] 或 [`KeepAlive::Disabled`] 时，这将返回 `None`。
    pub fn keep_alive_deadline(&self) -> Option<Instant> {
        match self.keep_alive() {
            KeepAlive::Timeout(dur) => Some(Instant::now() + dur),
            KeepAlive::Os => None,
            KeepAlive::Disabled => None,
        }
    }

    /// 创建一个时间对象，表示客户端完成发送其第一个请求的头部的截止日期。
    ///
    /// 如果此 `ServiceConfig 是用`client_request_timeout: 0`构造的，则返回`None`。
    pub fn client_request_deadline(&self) -> Option<Instant> {
        let timeout = self.0.client_request_timeout;
        (timeout != Duration::ZERO).then(|| Instant::now() + timeout)
    }

    ///创建一个时间对象，表示客户端断开连接的截止日期。
    pub fn client_disconnect_deadline(&self) -> Option<Instant> {
        let timeout = self.0.client_disconnect_timeout;
        (timeout != Duration::ZERO).then(|| Instant::now() + timeout)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeepAlive {
    /// Keep-alive duration.
    ///
    /// `KeepAlive::Timeout(Duration::ZERO)` is mapped to `KeepAlive::Disabled`.
    Timeout(Duration),

    /// Rely on OS to shutdown TCP connection.
    ///
    /// Some defaults can be very long, check your OS documentation.
    Os,

    /// Keep-alive is disabled.
    ///
    /// Connections will be closed immediately.
    Disabled,
}

impl KeepAlive {
    pub(crate) fn enabled(&self) -> bool {
        !matches!(self, Self::Disabled)
    }

    pub(crate) fn duration(&self) -> Option<Duration> {
        match self {
            KeepAlive::Timeout(dur) => Some(*dur),
            _ => None,
        }
    }

    /// Map zero duration to disabled.
    pub(crate) fn normalize(self) -> KeepAlive {
        match self {
            KeepAlive::Timeout(Duration::ZERO) => KeepAlive::Disabled,
            ka => ka,
        }
    }
}

impl Default for KeepAlive {
    fn default() -> Self {
        Self::Timeout(Duration::from_secs(5))
    }
}

impl From<Duration> for KeepAlive {
    fn from(dur: Duration) -> Self {
        KeepAlive::Timeout(dur).normalize()
    }
}

impl From<Option<Duration>> for KeepAlive {
    fn from(ka_dur: Option<Duration>) -> Self {
        match ka_dur {
            Some(dur) => KeepAlive::from(dur),
            None => KeepAlive::Disabled,
        }
        .normalize()
    }
}
