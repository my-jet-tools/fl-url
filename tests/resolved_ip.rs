//! `scheme://server-name@ip/...` against a real socket: the connection goes to the
//! ip with no DNS lookup, while the server name is what the server sees — in the
//! Host header and in the TLS SNI.
//!
//! The server name is under `.invalid` (RFC 2606), which is guaranteed never to
//! resolve, so reaching the server at all proves that no lookup happened.
#![cfg(not(target_arch = "wasm32"))]

use flurl::{FlUrl, FlUrlError, FlUrlMode};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpListener;

const SERVER_NAME: &str = "my-service.invalid";

/// Accepts one connection, reads the request head, answers `200 ok`.
async fn serve_one_request(listener: TcpListener) -> String {
    let (socket, _) = listener.accept().await.unwrap();
    let (read_half, mut write_half) = socket.into_split();
    let mut reader = BufReader::new(read_half);

    let mut head = String::new();
    loop {
        let mut line = String::new();
        let read = reader.read_line(&mut line).await.unwrap();
        if read == 0 || line == "\r\n" || line == "\n" {
            break;
        }
        head.push_str(&line);
    }

    write_half
        .write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 2\r\nconnection: close\r\n\r\nok")
        .await
        .unwrap();
    write_half.flush().await.unwrap();

    head
}

fn header<'s>(head: &'s str, name: &str) -> Option<&'s str> {
    head.lines().find_map(|line| {
        let (key, value) = line.split_once(':')?;
        if key.trim().eq_ignore_ascii_case(name) {
            Some(value.trim())
        } else {
            None
        }
    })
}

async fn http_request_goes_to_the_ip_with_the_server_name_as_host(mode: FlUrlMode) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();

    let server = tokio::spawn(serve_one_request(listener));

    let mut response = FlUrl::new(format!("http://{}@127.0.0.1:{}/api", SERVER_NAME, port))
        .update_mode(mode)
        .append_path_segment("items")
        .append_query_param("id", Some("1"))
        .get()
        .await
        .unwrap();

    assert_eq!(response.get_status_code(), 200);
    assert_eq!(response.get_body().unwrap().into_vec(usize::MAX).await.unwrap(), b"ok");

    let head = server.await.unwrap();

    assert!(
        head.starts_with("GET /api/items?id=1 HTTP/1.1\r\n"),
        "request line: {head}"
    );
    assert_eq!(
        header(&head, "host"),
        Some(format!("{}:{}", SERVER_NAME, port).as_str())
    );
}

#[tokio::test]
async fn http1_hyper_connects_to_the_ip_and_sends_the_server_name() {
    http_request_goes_to_the_ip_with_the_server_name_as_host(FlUrlMode::Http1Hyper).await;
}

#[tokio::test]
async fn http1_no_hyper_connects_to_the_ip_and_sends_the_server_name() {
    http_request_goes_to_the_ip_with_the_server_name_as_host(FlUrlMode::Http1NoHyper).await;
}

#[tokio::test]
async fn a_refused_connection_names_the_pinned_ip() {
    // Bind to learn a free port, then close it: connecting there is refused.
    let port = {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        listener.local_addr().unwrap().port()
    };

    let err = FlUrl::new(format!("http://{}@127.0.0.1:{}/api", SERVER_NAME, port))
        .get()
        .await
        .expect_err("nothing listens on the port");

    let err = format!("{:?}", err);
    assert!(
        err.contains(&format!(
            "127.0.0.1:{} (pinned ip for {}:{})",
            port, SERVER_NAME, port
        )),
        "{err}"
    );
}

#[test]
fn a_host_name_after_the_at_sign_is_rejected() {
    // The part after '@' is where the socket goes, so it must be an ip — a host
    // name there would need exactly the DNS lookup this form exists to skip.
    let fl_url = FlUrl::new("https://domain.com@backend.internal/xxx");
    assert!(matches!(
        fl_url.get_error(),
        Some(FlUrlError::InvalidUrl(_))
    ));
}

/// Reads the TLS ClientHello off the socket and returns the SNI host name in it.
/// The handshake is never completed — the server name is all this test is after.
#[cfg(any(feature = "with-ring-tls", feature = "with-rust-tls"))]
async fn read_sni(listener: TcpListener) -> Option<String> {
    use tokio::io::AsyncReadExt;

    let (mut socket, _) = listener.accept().await.unwrap();

    let mut record_header = [0u8; 5];
    socket.read_exact(&mut record_header).await.unwrap();
    assert_eq!(record_header[0], 0x16, "expected a TLS handshake record");

    let record_len = u16::from_be_bytes([record_header[3], record_header[4]]) as usize;
    let mut record = vec![0u8; record_len];
    socket.read_exact(&mut record).await.unwrap();

    let mut reader = ByteReader(&record);
    assert_eq!(reader.u8()?, 0x01, "expected a ClientHello");
    reader.skip(3)?; // handshake length
    reader.skip(2 + 32)?; // client version + random
    let session_id_len = reader.u8()? as usize;
    reader.skip(session_id_len)?;
    let cipher_suites_len = reader.u16()? as usize;
    reader.skip(cipher_suites_len)?;
    let compression_len = reader.u8()? as usize;
    reader.skip(compression_len)?;

    let extensions_len = reader.u16()? as usize;
    let mut extensions = ByteReader(reader.take(extensions_len)?);
    while !extensions.0.is_empty() {
        let extension_type = extensions.u16()?;
        let data_len = extensions.u16()? as usize;
        let mut data = ByteReader(extensions.take(data_len)?);

        if extension_type == 0 {
            data.skip(2)?; // server name list length
            assert_eq!(data.u8()?, 0, "expected a host_name entry");
            let name_len = data.u16()? as usize;
            return Some(String::from_utf8(data.take(name_len)?.to_vec()).unwrap());
        }
    }

    None
}

#[cfg(any(feature = "with-ring-tls", feature = "with-rust-tls"))]
struct ByteReader<'s>(&'s [u8]);

#[cfg(any(feature = "with-ring-tls", feature = "with-rust-tls"))]
impl<'s> ByteReader<'s> {
    fn take(&mut self, len: usize) -> Option<&'s [u8]> {
        if self.0.len() < len {
            return None;
        }
        let (result, rest) = self.0.split_at(len);
        self.0 = rest;
        Some(result)
    }

    fn skip(&mut self, len: usize) -> Option<()> {
        self.take(len).map(|_| ())
    }

    fn u8(&mut self) -> Option<u8> {
        Some(self.take(1)?[0])
    }

    fn u16(&mut self) -> Option<u16> {
        let bytes = self.take(2)?;
        Some(u16::from_be_bytes([bytes[0], bytes[1]]))
    }
}

#[cfg(any(feature = "with-ring-tls", feature = "with-rust-tls"))]
#[tokio::test]
async fn https_connects_to_the_ip_and_uses_the_server_name_as_sni() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();

    let server = tokio::spawn(read_sni(listener));

    // The test server walks away after the ClientHello, so the request itself
    // fails — what matters is where it went and what it introduced itself as.
    let err = FlUrl::new(format!("https://{}@127.0.0.1:{}/api", SERVER_NAME, port))
        .do_not_reuse_connection()
        .get()
        .await
        .expect_err("the server walks away mid-handshake");

    assert_eq!(server.await.unwrap().as_deref(), Some(SERVER_NAME));

    // The error names the ip the socket went to and the phase that failed —
    // not just the server name, which was never resolved.
    let err = format!("{:?}", err);
    assert!(
        err.contains(&format!(
            "127.0.0.1:{} (pinned ip for {}:{}). TLS handshake failed",
            port, SERVER_NAME, port
        )),
        "{err}"
    );
}
