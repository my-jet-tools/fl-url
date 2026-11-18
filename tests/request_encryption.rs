//! `encrypted()` against a real server, in every mode. The body on the wire is
//! `aes128gcm` (RFC 8188) of the body, marked `Content-Encoding: aes128gcm`, and it
//! opens with the key and with no other. Combined with `compress()` the body is gzipped
//! first and the header lists both codings in that order. A body with nothing in it,
//! and a streamed one, do not get encrypted.
#![cfg(all(not(target_arch = "wasm32"), feature = "encryption"))]

use std::convert::Infallible;
use std::io::Read;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use flurl::body::HttpRequestBody;
use flurl::my_http_utils::HttpBodyEncryptionKey;
use flurl::{FlUrl, FlUrlError, FlUrlHttpConnectionsCache, FlUrlMode};
use http_body_util::{BodyExt, Full};
use hyper::service::service_fn;
use hyper_util::rt::{TokioExecutor, TokioIo};
use tokio::net::TcpListener;

const MODES: [FlUrlMode; 3] = [
    FlUrlMode::Http1Hyper,
    FlUrlMode::Http1NoHyper,
    FlUrlMode::H2,
];

/// The 32-byte key of the vector in my-reverse-proxy's docs/encrypted-service-data.md.
const KEY: &str = "AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8=";

fn key() -> HttpBodyEncryptionKey {
    HttpBodyEncryptionKey::from_base64(KEY).unwrap()
}

fn another_key() -> HttpBodyEncryptionKey {
    HttpBodyEncryptionKey::from_bytes(&[7u8; 32]).unwrap()
}

fn json_body() -> Vec<u8> {
    let items: Vec<String> = (0..500)
        .map(|i| format!(r#"{{"id":{},"name":"user-{}","active":true}}"#, i, i % 37))
        .collect();
    format!(r#"{{"ts":1700000000,"items":[{}]}}"#, items.join(",")).into_bytes()
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
                let service = service_fn(
                    |request: hyper::Request<hyper::body::Incoming>| async move {
                        // Every line of the header, so a second line would show up as one.
                        let content_encoding: Vec<&str> = request
                            .headers()
                            .get_all("content-encoding")
                            .iter()
                            .map(|value| value.to_str().unwrap())
                            .collect();
                        let content_encoding = content_encoding.join(" | ");

                        let body = request.into_body().collect().await.unwrap().to_bytes();

                        let mut response = hyper::Response::new(Full::new(body));
                        if !content_encoding.is_empty() {
                            response.headers_mut().insert(
                                "x-request-content-encoding",
                                content_encoding.parse().unwrap(),
                            );
                        }

                        Ok::<_, Infallible>(response)
                    },
                );

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

    fn fl_url(&self) -> FlUrl {
        FlUrl::new(self.url.as_str())
            .update_mode(self.mode)
            .set_connections_cache(self.cache.clone())
            .set_timeout(Duration::from_secs(3))
    }

    /// Posts `body` with what `build` adds to the request.
    async fn post(&self, build: impl FnOnce(FlUrl) -> FlUrl, body: &[u8]) -> Received {
        let response = build(self.fl_url())
            .post(HttpRequestBody::from_raw_data(
                body.to_vec(),
                Some("application/json"),
            ))
            .await
            .unwrap();

        self.read(response).await
    }

    async fn read(&self, mut response: flurl::FlUrlResponse) -> Received {
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
async fn the_body_goes_out_encrypted_and_opens_with_the_key() {
    let body = json_body();

    for mode in MODES {
        let server = Server::start(mode).await;

        let received = server.post(|fl_url| fl_url.encrypted(&key()), &body).await;

        assert_eq!(
            received.content_encoding.as_deref(),
            Some("aes128gcm"),
            "{mode:?}"
        );
        assert_ne!(received.body, body, "{mode:?}");
        assert_eq!(key().decrypt(&received.body).unwrap(), body, "{mode:?}");
        assert!(another_key().decrypt(&received.body).is_err(), "{mode:?}");
    }
}

/// A fresh salt goes into every body.
#[tokio::test]
async fn the_same_body_never_looks_the_same_twice() {
    let body = br#"{"ts":1700000000}"#;
    let server = Server::start(FlUrlMode::Http1Hyper).await;

    let first = server.post(|fl_url| fl_url.encrypted(&key()), body).await;
    let second = server.post(|fl_url| fl_url.encrypted(&key()), body).await;

    assert_ne!(first.body, second.body);
    assert_eq!(key().decrypt(&first.body).unwrap(), body);
    assert_eq!(key().decrypt(&second.body).unwrap(), body);
}

/// Gzip first, then the encryption, whichever order the builder was told in. A body too
/// short to be gzipped is only encrypted.
#[tokio::test]
async fn a_compressed_body_is_gzipped_then_encrypted() {
    let body = json_body();

    for mode in MODES {
        let server = Server::start(mode).await;

        for build in [
            (|fl_url: FlUrl| fl_url.compress().encrypted(&key())) as fn(FlUrl) -> FlUrl,
            |fl_url: FlUrl| fl_url.encrypted(&key()).compress(),
        ] {
            let received = server.post(build, &body).await;

            assert_eq!(
                received.content_encoding.as_deref(),
                Some("gzip, aes128gcm"),
                "{mode:?}"
            );
            let gzipped = key().decrypt(&received.body).unwrap();
            assert_eq!(gunzip(&gzipped), body, "{mode:?}");
        }

        let short = br#"{"ts":1}"#;
        let received = server
            .post(|fl_url| fl_url.compress().encrypted(&key()), short)
            .await;
        assert_eq!(
            received.content_encoding.as_deref(),
            Some("aes128gcm"),
            "{mode:?}"
        );
        assert_eq!(key().decrypt(&received.body).unwrap(), short, "{mode:?}");
    }
}

/// The caller's own `Content-Encoding` stays, with `aes128gcm` after it on the same
/// line, whether the header was set before or after `encrypted()`.
#[tokio::test]
async fn the_callers_content_encoding_gets_aes128gcm_on_the_same_line() {
    let body = b"already encoded by the caller".to_vec();

    for mode in MODES {
        let server = Server::start(mode).await;

        for build in [
            (|fl_url: FlUrl| {
                fl_url
                    .with_header("Content-Encoding", "br")
                    .encrypted(&key())
            }) as fn(FlUrl) -> FlUrl,
            |fl_url: FlUrl| {
                fl_url
                    .encrypted(&key())
                    .with_header("Content-Encoding", "br")
            },
        ] {
            let received = server.post(build, &body).await;

            assert_eq!(
                received.content_encoding.as_deref(),
                Some("br, aes128gcm"),
                "{mode:?}"
            );
            assert_eq!(key().decrypt(&received.body).unwrap(), body, "{mode:?}");
        }
    }
}

/// A GET has no body: it goes out as one, with no `Content-Encoding`.
#[tokio::test]
async fn a_request_without_a_body_is_not_encrypted() {
    for mode in MODES {
        let server = Server::start(mode).await;

        let response = server.fl_url().encrypted(&key()).get().await.unwrap();
        let received = server.read(response).await;

        assert_eq!(received.content_encoding, None, "{mode:?}");
        assert!(received.body.is_empty(), "{mode:?}");
    }
}

#[tokio::test]
async fn a_streamed_body_is_refused() {
    let server = Server::start(FlUrlMode::Http1Hyper).await;

    let result = server
        .fl_url()
        .encrypted(&key())
        .post_request_streamed(Full::new(Bytes::from_static(b"secret")), Some(6))
        .await;

    assert!(
        matches!(result, Err(FlUrlError::StreamedBodyCanNotBeEncrypted)),
        "{:?}",
        result.err()
    );
}

/// The debug dump gets the length of an encrypted body, not its text.
#[tokio::test]
async fn the_debug_dump_does_not_carry_the_plain_text() {
    let body = br#"{"ts":1700000000,"password":"secret"}"#;
    let server = Server::start(FlUrlMode::Http1Hyper).await;

    let mut debug = String::new();
    let response = server
        .fl_url()
        .encrypted(&key())
        .post_with_debug(
            HttpRequestBody::from_raw_data(body.to_vec(), Some("application/json")),
            &mut debug,
        )
        .await
        .unwrap();
    let received = server.read(response).await;

    assert!(!debug.contains("secret"), "{debug}");
    assert!(
        debug.contains(&format!("Body: {} bytes, sent encrypted", body.len())),
        "{debug}"
    );
    assert_eq!(key().decrypt(&received.body).unwrap(), body);
}
