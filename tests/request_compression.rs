//! `compress()` and `compress_with_level()` against a real server, in every mode: what
//! lands on the wire is a gzip of the body at the level asked for, marked
//! `Content-Encoding: gzip`, and a body too short to be worth it goes out as it is.
//!
//! The level is read off the gzip header itself: its XFL byte is `4` for the fastest
//! compression, `2` for the best one and `0` for anything between (RFC 1952).
#![cfg(not(target_arch = "wasm32"))]

use std::convert::Infallible;
use std::io::Read;
use std::sync::Arc;
use std::time::Duration;

use flurl::body::HttpRequestBody;
use flurl::{CompressionLevel, FlUrl, FlUrlHttpConnectionsCache, FlUrlMode};
use http_body_util::{BodyExt, Full};
use hyper::service::service_fn;
use hyper_util::rt::{TokioExecutor, TokioIo};
use tokio::net::TcpListener;

const MODES: [FlUrlMode; 3] = [FlUrlMode::Http1Hyper, FlUrlMode::Http1NoHyper, FlUrlMode::H2];

/// Where the XFL byte is in the gzip header.
const XFL: usize = 8;

fn json_body() -> Vec<u8> {
    let items: Vec<String> = (0..500)
        .map(|i| format!(r#"{{"id":{},"name":"user-{}","active":true}}"#, i, i % 37))
        .collect();
    format!("[{}]", items.join(",")).into_bytes()
}

fn gunzip(data: &[u8]) -> Vec<u8> {
    let mut result = Vec::new();
    flate2::read::GzDecoder::new(data)
        .read_to_end(&mut result)
        .unwrap();
    result
}

/// A request as the server got it.
struct Received {
    content_encoding: Option<String>,
    body: Vec<u8>,
}

struct Server {
    url: String,
    mode: FlUrlMode,
    // A pool of its own: what another test does with the shared one is not seen here.
    cache: Arc<FlUrlHttpConnectionsCache>,
}

impl Server {
    /// Serves on a free port until the test ends. Every request is answered with its
    /// own body as it came over the wire, and with its `Content-Encoding` in
    /// `x-request-content-encoding`.
    async fn start(mode: FlUrlMode) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());
        let h2 = mode.is_h2();

        tokio::spawn(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else {
                    return;
                };

                let io = TokioIo::new(stream);
                let service = service_fn(|request: hyper::Request<hyper::body::Incoming>| async move {
                    let content_encoding = request.headers().get("content-encoding").cloned();
                    let body = request.into_body().collect().await.unwrap().to_bytes();

                    let mut response = hyper::Response::new(Full::new(body));
                    if let Some(value) = content_encoding {
                        response
                            .headers_mut()
                            .insert("x-request-content-encoding", value);
                    }

                    Ok::<_, Infallible>(response)
                });

                if h2 {
                    // Prior-knowledge h2 (h2c): plain TCP has no ALPN.
                    tokio::spawn(async move {
                        let _ = hyper::server::conn::http2::Builder::new(TokioExecutor::new())
                            .serve_connection(io, service)
                            .await;
                    });
                } else {
                    tokio::spawn(async move {
                        let _ = hyper::server::conn::http1::Builder::new()
                            .serve_connection(io, service)
                            .await;
                    });
                }
            }
        });

        Self {
            url,
            mode,
            cache: Arc::new(FlUrlHttpConnectionsCache::new()),
        }
    }

    /// Posts `body` with what `build` adds to the request.
    async fn post(&self, build: impl FnOnce(FlUrl) -> FlUrl, body: &[u8]) -> Received {
        let fl_url = FlUrl::new(self.url.as_str())
            .update_mode(self.mode)
            .set_connections_cache(self.cache.clone())
            .set_timeout(Duration::from_secs(3));

        let mut response = build(fl_url)
            .post(HttpRequestBody::from_raw_data(
                body.to_vec(),
                Some("application/json"),
            ))
            .await
            .unwrap();
        assert_eq!(response.get_status_code(), 200, "{:?}", self.mode);

        let content_encoding = response
            .get_header("x-request-content-encoding")
            .unwrap()
            .map(|value| value.to_string());
        let body = response
            .get_body()
            .unwrap()
            .into_vec(usize::MAX)
            .await
            .unwrap();

        Received {
            content_encoding,
            body,
        }
    }
}

#[tokio::test]
async fn the_body_goes_out_gzipped_at_the_level_asked_for() {
    let body = json_body();

    for mode in MODES {
        let server = Server::start(mode).await;

        for (level, xfl) in [
            (CompressionLevel::Fast, 4),
            (CompressionLevel::Default, 0),
            (CompressionLevel::Best, 2),
        ] {
            let received = server
                .post(|fl_url| fl_url.compress_with_level(level), &body)
                .await;

            assert_eq!(
                received.content_encoding.as_deref(),
                Some("gzip"),
                "{mode:?} {level:?}"
            );
            assert_eq!(received.body[XFL], xfl, "{mode:?} {level:?}");
            assert_eq!(gunzip(&received.body), body, "{mode:?} {level:?}");
        }
    }
}

#[tokio::test]
async fn compress_is_the_default_level_and_the_later_call_wins() {
    let body = json_body();
    let server = Server::start(FlUrlMode::Http1Hyper).await;

    let received = server.post(|fl_url| fl_url.compress(), &body).await;
    assert_eq!(received.body[XFL], 0);

    let received = server
        .post(
            |fl_url| fl_url.compress_with_level(CompressionLevel::Best).compress(),
            &body,
        )
        .await;
    assert_eq!(received.body[XFL], 0);

    let received = server
        .post(
            |fl_url| fl_url.compress().compress_with_level(CompressionLevel::Fast),
            &body,
        )
        .await;
    assert_eq!(received.body[XFL], 4);
}

#[tokio::test]
async fn a_body_under_64_bytes_goes_out_as_it_is() {
    let body = br#"{"id":1}"#;
    let server = Server::start(FlUrlMode::Http1Hyper).await;

    let received = server
        .post(
            |fl_url| fl_url.compress_with_level(CompressionLevel::Best),
            body,
        )
        .await;

    assert_eq!(received.content_encoding, None);
    assert_eq!(received.body, body);
}
