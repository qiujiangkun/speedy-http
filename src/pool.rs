use crate::stat::{ConnectionStatistics, ConnectionStatisticsEntry};
use crate::{HttpClient, RequestHandle};
use futures::future::BoxFuture;
use futures::{Future, FutureExt};
use http::Response;
use std::task::{Context, Poll};
use tokio::io::{AsyncRead, AsyncWrite};
use tracing::*;
#[derive(Clone)]
pub struct HttpClientPoolConfig {
    pub maintain_size: Option<usize>,
    pub max_conv_per_channel: usize,
}
type PendingQueue<T, Buf> = std::collections::VecDeque<(RequestHandle<T>, http::Request<Buf>)>;
struct ClientSection<Channel, Buf, T> {
    clients: Vec<HttpClient<Channel, Buf, T>>,
    last_client: usize,
    config: HttpClientPoolConfig,
}
impl<Channel: AsyncRead + AsyncWrite + Send + Unpin + 'static, Buf: bytes::Buf, T: Clone>
    ClientSection<Channel, Buf, T>
{
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
}
pub struct HttpClientPoolStats {
    pub current_stat: ConnectionStatistics,
    pub history_stats: Vec<ConnectionStatisticsEntry>,
    pub request_on_channel: Vec<usize>,
}

pub struct HttpClientPool<Channel, Buf = bytes::Bytes, T = ()> {
    client_section: ClientSection<Channel, Buf, T>,
    connecting: Vec<BoxFuture<'static, std::io::Result<Channel>>>,
    builder: Box<dyn Fn() -> BoxFuture<'static, std::io::Result<Channel>> + Send>,
    pending_requests: PendingQueue<T, Buf>,
    config: HttpClientPoolConfig,
    stats: HttpClientPoolStats,
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
            client_section: ClientSection {
                clients: vec![],
                last_client: 0,
                config: config.clone(),
            },
            connecting: vec![],
            builder: Box::new(move || Box::pin(builder())),
            pending_requests: Default::default(),
            config,

            stats: HttpClientPoolStats {
                current_stat: Default::default(),
                history_stats: vec![],
                request_on_channel: vec![],
            },
        }
    }

    fn record_status(&mut self) {
        self.stats.current_stat.connection_connecting_count = self.connecting.len() as i64;
        self.stats.current_stat.connection_living_count = self.client_section.clients.len() as i64;
        self.stats.current_stat.request_pending_count = self.pending_requests.len() as i64;
        match self.stats.history_stats.last() {
            Some(x) if x.stat == self.stats.current_stat => {}
            _ => self.stats.history_stats.push(ConnectionStatisticsEntry {
                time: std::time::SystemTime::now(),
                stat: self.stats.current_stat.clone(),
            }),
        }
    }

    pub fn poll_connecting(&mut self, cx: &mut Context<'_>) {
        let mut i = 0;
        while i < self.connecting.len() {
            let connecting = &mut self.connecting[i];
            match connecting.poll_unpin(cx) {
                Poll::Ready(Ok(channel)) => {
                    self.client_section.clients.push(HttpClient::new(channel));
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
        self.stats.current_stat.connection_new_count += 1;
        self.connecting.push((self.builder)());
    }
    pub fn poll_maintain_connection(&mut self) {
        while self.client_section.clients.len() + self.connecting.len()
            < self.config.maintain_size.unwrap_or(0)
        {
            self.make_connection()
        }
    }
    fn try_make_request(
        client: &mut HttpClient<Channel, Buf, T>,
        handle: RequestHandle<T>,
        pending: &mut PendingQueue<T, Buf>,
        request: http::Request<Buf>,
        stats: &mut HttpClientPoolStats,
    ) {
        match client.request_with_handle(request, handle.clone()) {
            Ok(..) => stats.request_on_channel.push(client.get_client_id()),
            Err(req) => {
                warn!("Client to be removed, request pending");
                pending.push_back((handle, req));
            }
        }
    }
    pub fn request(&mut self, request: http::Request<Buf>, data: T) -> RequestHandle<T> {
        let handle = RequestHandle::unique(data.clone());
        if let Some(client) = self.client_section.get_client_mut() {
            Self::try_make_request(
                client,
                handle.clone(),
                &mut self.pending_requests,
                request,
                &mut self.stats,
            );
        } else {
            warn!("No available clients, request pending");
            self.pending_requests.push_back((handle.clone(), request));
            if self.client_section.clients.len() + self.connecting.len()
                < self.config.maintain_size.unwrap_or(usize::MAX)
            {
                self.make_connection()
            }
        }
        self.stats.current_stat.request_sent_count += 1;
        self.record_status();
        handle
    }
    pub fn poll_send_request(&mut self) {
        for _ in 0..10 {
            if let Some((handle, request)) = self.pending_requests.pop_front() {
                if let Some(client) = self.client_section.get_client_mut() {
                    Self::try_make_request(
                        client,
                        handle.clone(),
                        &mut self.pending_requests,
                        request,
                        &mut self.stats,
                    );
                } else {
                    self.pending_requests.push_front((handle, request));
                    break;
                }
            }
        }
    }
    pub fn poll_response(
        &mut self,
        cx: &mut Context,
    ) -> Poll<(RequestHandle<T>, Response<bytes::Bytes>)> {
        self.poll_send_request();
        let mut resp = Poll::Pending;
        let mut i = 0;
        while i < self.client_section.clients.len() {
            let client = &mut self.client_section.clients[i];
            if !client.conn.can_write_head() {
                warn!("Remove closed client");
                self.client_section.clients.swap_remove(i);
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
                    self.stats.current_stat.response_ok_count += 1;
                    break;
                }
                Poll::Ready(Some(Err(err))) => {
                    warn!("Encountered error: {:?}", err);
                    self.client_section.clients.swap_remove(i);
                    self.stats.current_stat.response_bad_count += 1;
                }
                Poll::Ready(None) => {
                    self.client_section.clients.swap_remove(i);
                }
                Poll::Pending => {
                    i += 1;
                }
            }
        }
        self.poll_connecting(cx);
        self.poll_maintain_connection();
        self.record_status();
        resp
    }
    pub fn get_status_records(&self) -> &HttpClientPoolStats {
        &self.stats
    }
}
