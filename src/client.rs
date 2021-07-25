use crate::ensure;
use crate::RequestHandle;
use bytes::{Bytes, BytesMut};
use http::header::HOST;
use http::{HeaderValue, Request, Response};
use hyper::body::{Buf, DecodedLength};
use hyper::proto::h1::ClientTransaction;
use hyper::proto::{Conn, RequestHead, RequestLine};
use std::io::ErrorKind;
use std::task::{Context, Poll};
use tokio::io::{AsyncRead, AsyncWrite};

struct Receiving<T = ()> {
    handle: RequestHandle<T>,
    resp: Response<BytesMut>,
    length: Option<DecodedLength>,
}

impl<T> Receiving<T> {
    pub fn new(handle: RequestHandle<T>) -> Self {
        Self {
            handle,
            resp: Default::default(),
            length: None,
        }
    }
}
pub struct HttpClient<Channel, Buf = Bytes, T = ()> {
    pub(crate) conn: Conn<Channel, Buf, ClientTransaction>,
    queue: std::collections::VecDeque<Receiving<T>>,
}

impl<Channel: AsyncRead + AsyncWrite + Unpin, Buf: self::Buf, T: Clone>
    HttpClient<Channel, Buf, T>
{
    pub fn new(io: Channel) -> Self {
        Self {
            conn: hyper::proto::Conn::new(io),
            queue: Default::default(),
        }
    }

    pub fn request(
        &mut self,
        mut req: Request<Buf>,
        data: T,
    ) -> Result<RequestHandle<T>, Request<Buf>> {
        if self.conn.can_write_head() {
            let aur = req
                .uri()
                .authority()
                .expect("No valid authority")
                .to_string();
            req.headers_mut().insert(
                HOST,
                HeaderValue::from_str(&aur).expect("Host name invalid"),
            );
            let (parts, body) = req.into_parts();
            let head = RequestHead {
                version: parts.version,
                subject: RequestLine(parts.method, parts.uri),
                headers: parts.headers,
                extensions: parts.extensions,
            };
            self.conn.write_full_msg(head, body);
            let handle = RequestHandle::unique(data);
            self.queue.push_back(Receiving::new(handle.clone()));
            Ok(handle)
        } else {
            Err(req)
        }
    }
    pub fn poll_response(
        &mut self,
        cx: &mut Context<'_>,
    ) -> Poll<Option<std::io::Result<(RequestHandle<T>, Response<Bytes>)>>> {
        let _ = self.conn.poll_flush(cx)?;
        if self.conn.can_read_head() {
            match futures::ready!(self.conn.poll_read_head(cx)) {
                Some(Ok((head, length, _))) => {
                    let response = ensure!(self.queue.front_mut(), "No available request in queue");
                    *response.resp.version_mut() = head.version;
                    *response.resp.status_mut() = head.subject;
                    *response.resp.headers_mut() = head.headers;
                    *response.resp.extensions_mut() = head.extensions;
                    response.length = Some(length);
                }
                Some(Err(err)) => {
                    return Poll::Ready(Some(Err(std::io::Error::new(ErrorKind::Other, err))))
                }
                None => return Poll::Ready(None),
            }
        }
        if self.conn.can_read_body() {
            match futures::ready!(self.conn.poll_read_body(cx)) {
                Some(Ok(chunk)) => {
                    let response = ensure!(self.queue.front_mut(), "No available request in queue");
                    let length = ensure!(response.length.as_mut(), "Does not receive a length");
                    length.sub_if(chunk.len() as _);
                    response.resp.body_mut().extend_from_slice(chunk.as_ref());
                    if length.into_opt() == Some(0) {
                        return Poll::Ready(Some(Ok(ensure!(
                            self.queue
                                .pop_front()
                                .map(|x| (x.handle, x.resp.map(|body| body.freeze()))),
                            "No available request in queue"
                        ))));
                    }
                }
                None => {
                    return Poll::Ready(Some(Ok(ensure!(
                        self.queue
                            .pop_front()
                            .map(|x| (x.handle, x.resp.map(|body| body.freeze()))),
                        "No available request in queue"
                    ))))
                }
                Some(Err(err)) => {
                    return Poll::Ready(Some(Err(std::io::Error::new(ErrorKind::Other, err))))
                }
            }
        }
        Poll::Pending
    }
    pub fn queue_len(&self) -> usize {
        self.queue.len()
    }
}
