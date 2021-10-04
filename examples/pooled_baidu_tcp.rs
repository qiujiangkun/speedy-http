use http::Request;
use hyper::body::Bytes;
use hyper::Uri;
use speedy_http::stat::ConnectionStatisticsEntry;
use speedy_http::{HttpClientPool, HttpClientPoolConfig};
use std::io::Write;
use tracing::level_filters::LevelFilter;
use tracing::*;

mod logging;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    logging::setup_logs(LevelFilter::DEBUG)?;
    let begin = std::time::Instant::now();
    let builder = || tokio::net::TcpStream::connect("www.baidu.com:80");
    let mut client = HttpClientPool::new(
        builder,
        HttpClientPoolConfig {
            maintain_size: Some(10),
            max_conv_per_channel: 10,
        },
    );
    let connection_num = 1000;
    for _ in 0..connection_num {
        let req = Request::get(Uri::from_static("http://www.baidu.com")).body(Bytes::new())?;
        client.request(req, std::time::Instant::now());
    }
    let mut sum_time = 0;
    for _ in 0..connection_num {
        let (handle, _response) = futures::future::poll_fn(|cx| {
            cx.waker().wake_by_ref();
            client.poll_response(cx)
        })
        .await;
        sum_time += handle.into_data().elapsed().as_micros();
    }
    let elapsed = begin.elapsed();

    info!(
        "Finished {} requests in {:?}({} ms/req). Average RTT(req) {} ms.",
        connection_num,
        elapsed,
        elapsed.as_micros() as f64 / connection_num as f64 / 1000.0,
        sum_time as f64 / connection_num as f64 / 1000.0,
    );
    info!("Writing {} records", client.get_status_records().len());
    let mut csv = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open("result.csv")?;
    ConnectionStatisticsEntry::write_csv_headers(&mut csv)?;
    for status in &client.get_status_records().history_stats {
        status.write_csv_line(&mut csv)?;
    }
    csv.flush()?;
    Ok(())
}
