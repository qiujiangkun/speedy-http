use crate::stat::{ConnectionStatistics, ConnectionStatisticsEntry};
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

pub struct HttpClientPool<Channel, Buf = bytes::Bytes, T = ()> {
    clients: Vec<HttpClient<Channel, Buf, T>>,
    last_client: usize,
    connecting: Vec<BoxFuture<'static, std::io::Result<Channel>>>,
    builder: Box<dyn Fn() -> BoxFuture<'static, std::io::Result<Channel>> + Send>,
    pending_requests: std::collections::VecDeque<(RequestHandle<T>, http::Request<Buf>)>,
    config: HttpClientPoolConfig,
    stat: ConnectionStatistics,
    stats: Vec<ConnectionStatisticsEntry>,
}

impl<Channel: AsyncRead + AsyncWrite + Send + Unpin + 'static, Buf: bytes::Buf, T: Clone>
    HttpClientPool<Channel, Buf, T>
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
            stat: Default::default(),
            stats: vec![],
        }
    }

    fn record_status(&mut self) {
        self.stat.connection_connecting_count = self.connecting.len() as i64;
        self.stat.connection_living_count = self.clients.len() as i64;
        self.stat.request_pending_count = self.pending_requests.len() as i64;
        match self.stats.last() {
            Some(x) if x.stat == self.stat => {}
            _ => self.stats.push(ConnectionStatisticsEntry {
                time: std::time::SystemTime::now(),
                stat: self.stat.clone(),
            }),
        }
    }

    fn get_client_mut(&mut self) -> Option<&mut HttpClient<Channel, Buf, T>> {
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
    fn make_connection(&mut self) {
        self.stat.connection_new_count += 1;
        self.connecting.push((self.builder)());
    }
    fn poll_maintain(&mut self) {
        while self.clients.len() + self.connecting.len() < self.config.maintain_size.unwrap_or(0) {
            self.make_connection()
        }
    }
    pub fn request(&mut self, request: http::Request<Buf>, data: T) -> RequestHandle<T> {
        let handle = RequestHandle::unique(data.clone());
        if let Some(client) = self.get_client_mut() {
            if let Err(req) = client.request(request, data) {
                warn!("Client to be removed, request pending");
                self.pending_requests.push_back((handle.clone(), req));
            }
        } else {
            warn!("No available clients, request pending");
            self.pending_requests.push_back((handle.clone(), request));
            if self.clients.len() + self.connecting.len()
                < self.config.maintain_size.unwrap_or(usize::MAX)
            {
                self.make_connection()
            }
        }
        self.stat.request_sent_count += 1;
        self.record_status();
        handle
    }
    pub fn poll_response(
        &mut self,
        cx: &mut Context,
    ) -> Poll<(RequestHandle<T>, Response<bytes::Bytes>)> {
        for _ in 0..10 {
            if let Some((handle, request)) = self.pending_requests.pop_front() {
                if let Some(client) = self.get_client_mut() {
                    if let Err(req) = client.request(request, handle.data.clone()) {
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
            let result = client.poll_response(cx);
            // match &result {
            //     Poll::Pending => {}
            //     Poll::Ready(result) => {
            //         info!("Poll response result {:?}", result);
            //     }
            // }

            match result {
                Poll::Ready(Some(Ok(r))) => {
                    resp = Poll::Ready(r);
                    self.stat.response_ok_count += 1;
                    break;
                }
                Poll::Ready(Some(Err(err))) => {
                    warn!("Encountered error: {:?}", err);
                    self.clients.swap_remove(i);
                    self.stat.response_bad_count += 1;
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
        self.record_status();
        resp
    }
    pub fn get_status_records(&self) -> &Vec<ConnectionStatisticsEntry> {
        &self.stats
    }
}
