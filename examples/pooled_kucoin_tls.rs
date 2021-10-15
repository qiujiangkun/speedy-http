use http::Request;
use hyper::body::Bytes;
use hyper::Uri;
use rustls::RootCertStore;
use speedy_http::stat::ConnectionStatisticsEntry;
use speedy_http::{HttpClientPool, HttpClientPoolConfig};
use std::io::Write;
use std::net::ToSocketAddrs;
use std::sync::Arc;
use std::task::Poll;
use std::time::Duration;
use tokio::net::TcpStream;
use tokio_rustls::TlsConnector;
use tracing::level_filters::LevelFilter;
use tracing::*;

mod logging;

async fn connect_to_server(
) -> std::io::Result<tokio_rustls::client::TlsStream<tokio::net::TcpStream>> {
    let domain = "api.kucoin.com";
    let addr = (domain, 443)
        .to_socket_addrs()?
        .next()
        .ok_or_else(|| std::io::Error::from(std::io::ErrorKind::NotFound))?;

    let mut root_store = RootCertStore::empty();
    root_store.add_server_trust_anchors(&webpki_roots::TLS_SERVER_ROOTS);

    let mut config = rustls::ClientConfig::new();
    config.root_store = root_store;

    let connector = TlsConnector::from(Arc::new(config));

    let stream = TcpStream::connect(&addr).await?;

    let domain = webpki::DNSNameRef::try_from_ascii_str(domain)
        .map_err(|_| std::io::Error::new(std::io::ErrorKind::InvalidInput, "invalid dnsname"))?;

    let stream = connector.connect(domain, stream).await?;
    Ok(stream)
}
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    logging::setup_logs(LevelFilter::DEBUG)?;
    let builder = || connect_to_server();
    let mut client = HttpClientPool::new(
        builder,
        HttpClientPoolConfig {
            maintain_size: Some(100),
            max_conv_per_channel: 10,
        },
    );
    let connection_num = 100;
    client.poll_maintain_connection();
    for _ in 0..500 {
        tokio::time::sleep(Duration::from_millis(1)).await;
        futures::future::poll_fn(|cx| Poll::Ready(client.poll_connecting(cx))).await;
    }
    let begin = std::time::Instant::now();
    for _ in 0..connection_num {
        let req =
            Request::get(Uri::from_static("https://api.kucoin.com/timestamp")).body(Bytes::new())?;
        client.request(req, std::time::Instant::now());
        client.poll_send_request();
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
    info!(
        "Writing {} records",
        client.get_status_records().history_stats.len()
    );
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
    let mut on_channels = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open("request_on_channels.csv")?;
    writeln!(on_channels, "channel")?;
    for line in &client.get_status_records().request_on_channel {
        writeln!(on_channels, "{}", line)?;
    }
    on_channels.flush()?;

    Ok(())
}
