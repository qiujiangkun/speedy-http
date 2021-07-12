use bytes::{Bytes, BytesMut};
use http::{Request, Response};
use hyper::body::{Buf, DecodedLength};
use hyper::proto::h1::ClientTransaction;
use hyper::proto::{Conn, RequestHead, RequestLine};
use std::io::{Error, ErrorKind};
use std::pin::Pin;
use std::task::{Context, Poll};
use tokio::io::{AsyncRead, AsyncWrite};
macro_rules! ensure {
    ($e: expr, $err: expr) => {
        $e.ok_or_else(|| std::io::Error::new(std::io::ErrorKind::Other, $err))?
    };
}
#[derive(Copy, Clone)]
pub struct RequestHandle(usize);

struct Receiving {
    handle: RequestHandle,
    resp: Response<BytesMut>,
    length: Option<DecodedLength>,
}

impl Receiving {
    pub fn new(handle: RequestHandle) -> Self {
        Self {
            handle,
            resp: Default::default(),
            length: None,
        }
    }
}

pub struct HttpClient<Channel, Buf = Bytes> {
    conn: Conn<Channel, Buf, ClientTransaction>,
    handle_cnt: usize,
    queue: std::collections::VecDeque<Receiving>,
}

impl<Channel: AsyncRead + AsyncWrite + Unpin, Buf: self::Buf> HttpClient<Channel, Buf> {
    pub fn new(io: Channel) -> Self {
        Self {
            conn: hyper::proto::Conn::new(io),
            handle_cnt: 0,
            queue: Default::default(),
        }
    }
    fn new_handle(&mut self) -> RequestHandle {
        let handle = RequestHandle(self.handle_cnt);
        self.handle_cnt += 1;
        handle
    }
    pub fn request(&mut self, req: Request<Buf>) -> RequestHandle {
        let (parts, body) = req.into_parts();
        let head = RequestHead {
            version: parts.version,
            subject: RequestLine(parts.method, parts.uri),
            headers: parts.headers,
            extensions: parts.extensions,
        };
        self.conn.write_full_msg(head, body);
        let handle = self.new_handle();
        self.queue.push_back(Receiving::new(handle));
        handle
    }
    pub fn poll_body(
        &mut self,
        cx: &mut Context<'_>,
    ) -> Poll<Option<std::io::Result<(RequestHandle, Response<Bytes>)>>> {
        let _ = self.conn.poll_flush(cx)?;
        if self.queue.is_empty() {
            return Poll::Ready(None);
        }
        if self.conn.can_read_head() {
            match futures::ready!(self.conn.poll_read_head(cx)) {
                Some(Ok((head, length, _))) => {
                    let response = ensure!(self.queue.front_mut(), "No available request in queue");
                    *response.resp.version_mut() = head.version;
                    *response.resp.status_mut() = head.subject;
                    *response.resp.headers_mut() = head.headers;
                    *response.resp.extensions_mut() = head.extensions;
                    response.length = Some(length);
                    Poll::Pending
                }
                Some(Err(err)) => {
                    Poll::Ready(Some(Err(std::io::Error::new(ErrorKind::Other, err))))
                }
                None => {
                    panic!("I don't know what to do")
                }
            }
        } else if self.conn.can_read_body() {
            match futures::ready!(self.conn.poll_read_body(cx)) {
                Some(Ok(chunk)) => {
                    let response = ensure!(self.queue.front_mut(), "No available request in queue");
                    let length = ensure!(response.length.as_mut(), "Does not receive a length");
                    length.sub_if(chunk.len() as _);
                    response.resp.body_mut().extend_from_slice(chunk.as_ref());
                    if length.into_opt() == Some(0) {
                        Poll::Ready(Some(Ok(ensure!(
                            self.queue
                                .pop_front()
                                .map(|x| (x.handle, x.resp.map(|body| body.freeze()))),
                            "No available request in queue"
                        ))))
                    } else {
                        Poll::Pending
                    }
                }
                None => Poll::Ready(Some(Ok(ensure!(
                    self.queue
                        .pop_front()
                        .map(|x| (x.handle, x.resp.map(|body| body.freeze()))),
                    "No available request in queue"
                )))),
                Some(Err(err)) => {
                    Poll::Ready(Some(Err(std::io::Error::new(ErrorKind::Other, err))))
                }
            }
        } else {
            Poll::Pending
        }
    }
}

impl<Channel: AsyncRead + AsyncWrite + Unpin, Buf: self::Buf> AsyncWrite
    for HttpClient<Channel, Buf>
{
    fn poll_write(
        self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        _buf: &[u8],
    ) -> Poll<Result<usize, Error>> {
        panic!("Does not support writing directly");
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Error>> {
        self.conn.poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Error>> {
        self.conn.poll_shutdown(cx)
    }
}

// impl<Channel: AsyncRead + AsyncWrite + Unpin, Buf: self::Buf> AsyncRead for HttpClient<Channel, Buf> {
//     fn poll_read(self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &mut ReadBuf<'_>) -> Poll<std::io::Result<()>> {
//
//     }
// }
