use http::Request;
use hyper::body::Bytes;
use hyper::Uri;
use speedy_http::{HttpClientPool, HttpClientPoolConfig};
use tracing::level_filters::LevelFilter;
mod logging;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    logging::setup_logs(LevelFilter::TRACE)?;

    let begin = std::time::Instant::now();
    let builder = || tokio::net::TcpStream::connect("www.baidu.com:80");
    let mut client = HttpClientPool::new(
        builder,
        HttpClientPoolConfig {
            maintain_size: Some(10),
            max_conv_per_channel: 10,
        },
    );
    let connection_num = 100;
    for _ in 0..connection_num {
        let req = Request::get(Uri::from_static("http://www.baidu.com")).body(Bytes::new())?;
        client.request(req);
    }
    for _ in 0..connection_num {
        let (_handle, _response) = futures::future::poll_fn(|cx| {
            cx.waker().wake_by_ref();
            client.poll_response(cx)
        })
        .await;
        // let body = req.into_body();
        // println!("Read {} bytes", body.len());
    }
    println!(
        "Finished {} connections in {:?}",
        connection_num,
        begin.elapsed()
    );
    Ok(())
}
