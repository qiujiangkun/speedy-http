use anyhow::Context;
use http::Request;
use hyper::body::Bytes;
use hyper::Uri;
use speedy_http::HttpClient;
use tracing::level_filters::LevelFilter;
use tracing_log::LogTracer;
use tracing_subscriber::fmt;

pub fn setup_logs(log_level: LevelFilter) -> anyhow::Result<()> {
    let default_level = format!("[]={}", log_level);
    LogTracer::init().context("Cannot setup_logs")?;
    let subscriber = fmt()
        .with_thread_names(true)
        .with_env_filter(default_level)
        .finish();
    tracing::subscriber::set_global_default(subscriber).context("Cannot setup_logs")?;
    Ok(())
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    setup_logs(LevelFilter::OFF)?;

    let begin = std::time::Instant::now();
    let io = tokio::net::TcpStream::connect("www.baidu.com:80").await?;
    let mut client = HttpClient::new(io);
    let connection_num = 100;
    for _ in 0..connection_num {
        let req = Request::get(Uri::from_static("http://www.baidu.com")).body(Bytes::new())?;
        client.request(req);
    }
    for _ in 0..connection_num {
        let (_handle, _req) = futures::future::poll_fn(|cx| {
            cx.waker().wake_by_ref();
            client.poll_body(cx)
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
