use http::Request;
use hyper::body::Bytes;
use hyper::Uri;
use speedy_http::HttpClient;
use tracing::level_filters::LevelFilter;
mod logging;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    logging::setup_logs(LevelFilter::OFF)?;

    let begin = std::time::Instant::now();
    let io = tokio::net::TcpStream::connect("www.baidu.com:80").await?;
    let mut client = HttpClient::new(io);
    let connection_num = 100;
    for _ in 0..connection_num {
        let req = Request::get(Uri::from_static("http://www.baidu.com")).body(Bytes::new())?;
        client.request(req).unwrap();
    }
    for _ in 0..connection_num {
        let (_handle, _response) = futures::future::poll_fn(|cx| {
            cx.waker().wake_by_ref();
            client.poll_response(cx)
        })
        .await
        .unwrap()?;
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
