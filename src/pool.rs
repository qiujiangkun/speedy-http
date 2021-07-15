use crate::{HttpClient, RequestHandle};
use futures::future::BoxFuture;
use futures::{Future, FutureExt};
use http::Response;
use std::task::{Context, Poll};
use tokio::io::{AsyncRead, AsyncWrite};
use tracing::*;

pub struct HttpClientPoolConfig {
    pub maintain_size: Option<usize>,
    pub max_conv_per_channel: usize,
}
pub struct HttpClientPool<Channel, Buf = bytes::Bytes> {
    clients: Vec<HttpClient<Channel, Buf>>,
    last_client: usize,
    connecting: Vec<BoxFuture<'static, std::io::Result<Channel>>>,
    builder: Box<dyn Fn() -> BoxFuture<'static, std::io::Result<Channel>> + Send>,
    pending_requests: std::collections::VecDeque<(RequestHandle, http::Request<Buf>)>,
    config: HttpClientPoolConfig,
}

impl<Channel: AsyncRead + AsyncWrite + Send + Unpin + 'static, Buf: bytes::Buf>
    HttpClientPool<Channel, Buf>
{
    pub fn new<Func, Fut>(builder: Func, config: HttpClientPoolConfig) -> Self
    where
        Func: Fn() -> Fut + Send + 'static,
        Fut: Future<Output = std::io::Result<Channel>> + Send + 'static,
    {
        Self {
            clients: vec![],
            last_client: 0,
            connecting: vec![],
            builder: Box::new(move || Box::pin(builder())),
            pending_requests: Default::default(),
            config,
        }
    }
    fn get_client_mut(&mut self) -> Option<&mut HttpClient<Channel, Buf>> {
        if self.last_client >= self.clients.len() {
            self.last_client = 0;
        }
        let mut client = None;
        let total_len = self.clients.len();

        while self.last_client < total_len {
            let id = self.last_client;
            self.last_client += 1;
            let c = &self.clients[id];
            if c.conn.can_write_head() && c.queue_len() < self.config.max_conv_per_channel {
                client = Some(id);
                break;
            }
        }
        if client.is_none() {
            client = self
                .clients
                .iter()
                .enumerate()
                .min_by_key(|x| x.1.queue_len())
                .map(|x| x.0);
        }
        let clients = &mut self.clients;
        client.map(move |i| &mut clients[i])
    }
    fn poll_connecting(&mut self, cx: &mut Context<'_>) {
        let mut i = 0;
        while i < self.connecting.len() {
            let connecting = &mut self.connecting[i];
            match connecting.poll_unpin(cx) {
                Poll::Ready(Ok(channel)) => {
                    self.clients.push(HttpClient::new(channel));
                    self.connecting.swap_remove(i);
                }
                Poll::Ready(Err(err)) => {
                    error!("Error while connecting {:?}", err);
                    self.connecting.swap_remove(i);
                }
                Poll::Pending => {
                    i += 1;
                }
            }
        }
    }
    fn poll_maintain(&mut self) {
        while self.clients.len() + self.connecting.len() < self.config.maintain_size.unwrap_or(0) {
            self.connecting.push((self.builder)());
        }
    }
    pub fn request(&mut self, request: http::Request<Buf>) -> RequestHandle {
        let handle = RequestHandle::unique();
        if let Some(client) = self.get_client_mut() {
            if let Err(req) = client.request(request) {
                warn!("Client to be removed, request pending");
                self.pending_requests.push_back((handle, req));
            }
        } else {
            warn!("No available clients, request pending");
            self.pending_requests.push_back((handle, request));
            if self.clients.len() + self.connecting.len()
                < self.config.maintain_size.unwrap_or(usize::MAX)
            {
                self.connecting.push((self.builder)());
            }
        }
        handle
    }
    pub fn poll_response(
        &mut self,
        cx: &mut Context,
    ) -> Poll<(RequestHandle, Response<bytes::Bytes>)> {
        for _ in 0..10 {
            if let Some((handle, request)) = self.pending_requests.pop_front() {
                if let Some(client) = self.get_client_mut() {
                    if let Err(req) = client.request(request) {
                        warn!("Client to be removed, request pending");
                        self.pending_requests.push_back((handle, req));
                    }
                } else {
                    self.pending_requests.push_front((handle, request));
                    break;
                }
            }
        }
        let mut resp = Poll::Pending;
        let mut i = 0;
        while i < self.clients.len() {
            let client = &mut self.clients[i];
            if !client.conn.can_write_head() {
                warn!("Remove closed client");
                self.clients.swap_remove(i);
                continue;
            }
            match client.poll_response(cx) {
                Poll::Ready(Some(Ok(r))) => {
                    resp = Poll::Ready(r);
                    break;
                }
                Poll::Ready(Some(Err(err))) => {
                    warn!("Encountered error: {:?}", err);
                    self.clients.swap_remove(i);
                }
                Poll::Ready(None) => {
                    self.clients.swap_remove(i);
                }
                Poll::Pending => {
                    i += 1;
                }
            }
        }
        self.poll_connecting(cx);
        self.poll_maintain();
        resp
    }
}
