# FLUrl

A fluent, async HTTP client library for Rust, inspired by the .NET Flurl library (https://flurl.dev/).

FLUrl is a Hyper-based HTTP client that provides a fluent API for building and executing HTTP requests with connection pooling, retry logic, and comprehensive body type support.

This page is everything an ordinary request needs: installation, building and sending requests, bodies, responses and errors. The rest — WASM, streamed bodies, request models, connections, TLS, SSH, unix sockets, retries, timeouts and limits, compression, debugging — are topics, listed at the end. Read a topic: `resource://flurl-usage-guide/{topic}`, or `get_flurl_usage_guide` with `topic`.

## Features

- **Fluent API**: Chain methods to build requests naturally
- **Connection Reuse**: Automatic connection pooling and reuse for HTTP/1.1 and HTTP/2
- **Multiple HTTP Modes**: Support for HTTP/2, HTTP/1.1 with Hyper, and HTTP/1.1 without Hyper
- **Body Types**: JSON, URL-encoded, multipart/form-data, and raw data
- **SSL/TLS**: Opt-in via one of two provider features — `with-ring-tls` (ring) or `with-rust-tls` (pure Rust, no C toolchain). Client certificate support and invalid certificate acceptance. With neither, the crate never links rustls and an `https://` request fails with `FlUrlError::UnsupportedScheme`
- **SSH Tunneling**: Optional SSH tunnel support via `with-ssh` feature
- **Unix Socket Support**: Native Unix socket support (Unix systems only)
- **Known IP, no DNS**: `https://domain.com@15.0.0.5/path` connects to the ip while the Host header and TLS SNI stay `domain.com` (native only) — see [Connecting to a Known IP](known-ip.md)
- **Retry Logic**: `with_retries(n)` replays a request that got no response; `with_retry(delay, n)` also replays a 5xx, `delay` apart, to ride out a restart of the service — see [Retry Logic](retries.md)
- **Request Compression**: `compress()` gzips a request body of 64 bytes and up; `compress_with_level(CompressionLevel::Fast | Default | Best)` picks how hard. Under wasm both come with the `with-wasm-compression` feature — see [Compression](compression.md#request-compression-under-wasm)
- **Response Body Reader**: `get_body()` gives the body as a reader — read it piece by piece, or as a whole within a size limit, as bytes, a string or JSON — native and wasm
- **Passing a Response On**: `into_hyper_response()` hands the response on with its body still streaming (native only)
- **Streaming Request Bodies**: Send a body of any size at constant memory, framed with `Content-Length` or chunked (native only) — see [Streamed Body](streamed-body.md)
- **Debug Support**: Built-in request debugging capabilities
- **WASM Support**: The same API compiles to `wasm32-unknown-unknown` (browser / web-worker) on top of the `fetch` API — see [WebAssembly (WASM) Support](wasm.md)

## Installation

`flurl` is not published on crates.io — it is pulled from git, pinned to a release
tag. Add to your `Cargo.toml`:

```toml
[dependencies]
flurl = { tag = "0.7.0", git = "https://github.com/MyJetTools/fl-url.git" }
```

**`https://` needs a TLS provider feature.** Both are off by default, so a
project doing plain HTTP (or unix sockets, or SSH tunnels) does not pay for the
rustls stack — `my-tls`, `rustls`, `tokio-rustls` and the provider itself all
leave the dependency tree. In a build with neither, a request to an `https://` url
returns `FlUrlError::UnsupportedScheme` — `FlUrl does not support https: it is
compiled without a TLS provider feature` — before a socket is opened.

TLS is decided once for the whole application — a provider feature turned on
anywhere in the build turns TLS on for every user of `flurl` in it. There are
three answers: no provider feature at all, `with-ring-tls`, or `with-rust-tls`.
None of them is a default — ask the user which of the three it is to be:

```toml
[dependencies]
# ring — mature, very widely deployed; costs a bundled C/assembly build.
flurl = { tag = "0.7.0", git = "https://github.com/MyJetTools/fl-url.git", features = [
    "with-ring-tls",
] }
```

```toml
[dependencies]
# pure Rust — no C toolchain anywhere, via rustls-graviola.
# x86_64 and aarch64 only; a younger, less deployed crypto implementation.
flurl = { tag = "0.7.0", git = "https://github.com/MyJetTools/fl-url.git", features = [
    "with-rust-tls",
] }
```

Enabling both is not an error (`--all-features` does it): my-tls resolves the
conflict in favour of ring.

For SSH tunneling support:

```toml
[dependencies]
flurl = { tag = "0.7.0", git = "https://github.com/MyJetTools/fl-url.git", features = [
    "with-ssh",
] }
```

### Feature flags

| Feature | Default | What it does |
| --- | --- | --- |
| `with-ring-tls` | off | TLS on the **ring** provider. Enables `https://` plus [`with_client_certificate`](tls.md#client-certificate). Mature and widely deployed; costs a bundled C/assembly build. No `aws-lc-sys` either way. |
| `with-rust-tls` | off | The same, on a **pure-Rust** provider (`rustls-graviola`) — no C toolchain at all. Builds only on x86_64 and aarch64, and the implementation is far younger than ring. |
| `dangerous-tls` | off | A modifier, not a TLS switch: it makes [`accept_invalid_certificate()`](tls.md#accept-invalid-certificates) actually skip server-cert verification. Combine it with a provider feature — on its own the TLS code compiles but no provider is installed, so https fails at connect time. |
| `with-ssh` | off | [SSH tunneling](ssh.md) (`ssh://…->http://…`, `ssh://…->https://…` and `ssh://…->/path/to.sock` urls). Unix only. Built on `my-ssh`, which is `russh` underneath — no `libssh2` or OpenSSL, but this is the one feature that links `aws-lc-sys` (a bundled C build). |
| `with-wasm-compression` | off | wasm only: compiles [`compress()` and `compress_with_level()`](compression.md#request-compression-under-wasm) and flate2 into the bundle; without it a call under wasm is a compile error. Whether a body is compressed is decided at run time, so the linker can not drop the gzip code, and a client that never compresses would pay for it: 20 KB over the wire, measured on a trading terminal. The native backend always has both and does not depend on it. |

On `wasm32` TLS is the browser's job, so neither provider feature matters there —
the `fetch` backend handles `https://` with or without them.

## Basic Usage

### Simple GET Request

```rust
use flurl::FlUrl;

let response = FlUrl::new("http://mywebsite.com")
    .append_path_segment("api")
    .append_path_segment("users")
    .append_query_param("page", Some("1"))
    .append_query_param("limit", Some("10"))
    .get()
    .await?;
```

### Errors Come From the Request

`FlUrl` is a builder: `FlUrl::new` and every method chained after it return the
builder, never a `Result`. What one of them can not use — the url, a header, a client
certificate — is remembered instead: from the first such error on every step is
skipped, and the call that sends the request returns that error without sending
anything. So one `?`, or one `match`, at the end covers the whole chain:

```rust
use flurl::{FlUrl, FlUrlError};

match FlUrl::new(url_from_settings)
    .append_path_segment("api")
    .with_header("X-Api-Key", api_key)
    .get()
    .await
{
    Ok(response) => {
        // A response arrived
    }
    Err(FlUrlError::InvalidUrl(reason)) => {
        // The url from settings can not be used
        eprintln!("Invalid URL: {}", reason);
    }
    Err(err) => {
        eprintln!("Error: {}", err);
    }
}
```

`FlUrl::new` does not panic on a url it can not use.

To check a url without sending anything — at start-up, say — ask the builder for the
error it is carrying. `get_url_builder()` gives the url as it stands, and once there
is an error it gives that error instead, so `get_url_builder().unwrap()` panics with
what was wrong with the url:

```rust
use flurl::FlUrl;

if let Some(err) = FlUrl::new(url_from_settings).get_error() {
    eprintln!("Invalid url in settings: {}", err);
}
```

The request fails with `FlUrlError::InvalidUrl` for a url that can not be used at all:

| url | why |
| --- | --- |
| `""`, `" "`, `http://`, `http://:8080`, `http:///path`, `http+unix://` | it names no host (for a unix socket — no socket file) |
| a host, or a socket path, that does not fit into 255 bytes together with its port | it is too long to be a host — usually something else pasted into the url's place |
| `ftp://host` | the scheme is not one FlUrl knows |
| `https://domain.com@backend.internal` | a malformed [`name@ip`](known-ip.md) form |
| `http://user@host:22->…`, `ssh://user@host:22x->…` | a malformed [ssh tunnel](ssh.md) part |

A url with no scheme is fine — `localhost:8080`, `10.0.0.1:5123/api` — and is taken
as `http://`. That is the native backend; under wasm a url that does not start with
`http://` or `https://` is resolved against the page origin instead — see
[Relative URLs](wasm.md#relative-urls-origin-resolution-wasm-only).

A url that parses but can not be put on the wire fails the request as well: a new
line or another control byte in the host, a space in the path (a value read from a
file together with its trailing new line is the usual source).

### Using String Literals (IntoFlUrl Trait)

```rust
use flurl::IntoFlUrl;

let response = "http://mywebsite.com"
    .append_path_segment("Row")
    .append_query_param("tableName", Some(table_name))
    .append_query_param("partitionKey", Some(partition_key))
    .get()
    .await?;
```

## HTTP Methods

Each of them has a `*_with_debug` twin that also dumps the request into a `&mut String`
— see [Request Debug String](debugging.md#request-debug-string) for the full list.

### GET

```rust
let response = FlUrl::new("https://api.example.com/data")
    .get()
    .await?;
```

### GET with Debug

```rust
let mut debug_string = String::new();
let response = FlUrl::new("https://api.example.com/data")
    .get_with_debug(&mut debug_string)
    .await?;
println!("Request: {}", debug_string);
```

### POST

```rust
use flurl::body::HttpRequestBody;

let body = HttpRequestBody::as_json(&my_data);
let response = FlUrl::new("https://api.example.com/users")
    .post(body)
    .await?;
```

### POST with Debug

```rust
let mut debug_string = String::new();
let body = HttpRequestBody::as_json(&my_data);
let response = FlUrl::new("https://api.example.com/users")
    .post_with_debug(body, &mut debug_string)
    .await?;
```

### PUT

```rust
let body = HttpRequestBody::as_json(&update_data);
let response = FlUrl::new("https://api.example.com/users/123")
    .put(body)
    .await?;
```

### PATCH

```rust
let body = HttpRequestBody::as_json(&patch_data);
let response = FlUrl::new("https://api.example.com/users/123")
    .patch(body)
    .await?;
```

### DELETE

```rust
let response = FlUrl::new("https://api.example.com/users/123")
    .delete()
    .await?;
```

### DELETE with Debug

```rust
let mut debug_string = String::new();
let response = FlUrl::new("https://api.example.com/users/123")
    .delete_with_debug(&mut debug_string)
    .await?;
```

### HEAD

```rust
let response = FlUrl::new("https://api.example.com/resource")
    .head()
    .await?;
```

## URL Building

### Append Path Segments

```rust
let response = FlUrl::new("https://api.example.com")
    .append_path_segment("api")
    .append_path_segment("v1")
    .append_path_segment("users")
    .get()
    .await?;
// Results in: https://api.example.com/api/v1/users
```

### Append Query Parameters

```rust
let response = FlUrl::new("https://api.example.com/search")
    .append_query_param("q", Some("rust"))
    .append_query_param("page", Some("1"))
    .append_query_param("sort", None::<&str>) // Adds parameter without value
    .get()
    .await?;
// Results in: https://api.example.com/search?q=rust&page=1&sort
```

### Append Raw URL Ending

```rust
let response = FlUrl::new("https://api.example.com")
    .append_raw_ending_to_url("/custom/path?param=value")
    .get()
    .await?;
```

## Headers

### Add Custom Headers

```rust
let response = FlUrl::new("https://api.example.com/data")
    .with_header("Authorization", "Bearer token123")
    .with_header("X-Custom-Header", "value")
    .get()
    .await?;
```

A header that must not be put on the wire fails the request with
`FlUrlError::RequestBuild` — nothing is sent, and the header is not silently dropped:

- a value with a CR, LF or NUL in it — typically an api key read from a file together
  with its trailing new line;
- a name that is empty or is not an HTTP token (a space, a `:`, a non-ASCII letter).

`with_header` itself can not report this (it returns the builder), so the error comes
from the call that sends the request. The message names the header and the byte, never
the value. The same applies to the header fields of a
[request model](model-driven-requests.md).

That check is the native backend's. Under wasm a header goes to the browser as it is,
and one the browser refuses fails the request with `FlUrlError::FetchError`.

## Request Bodies

### JSON Body

```rust
use flurl::body::HttpRequestBody;
use serde::Serialize;

#[derive(Serialize)]
struct User {
    name: String,
    email: String,
}

let user = User {
    name: "John Doe".to_string(),
    email: "john@example.com".to_string(),
};

let response = FlUrl::new("https://api.example.com/users")
    .post(HttpRequestBody::as_json(&user))
    .await?;
```

### URL-Encoded Body

```rust
use flurl::body::UrlEncodedBody;

let body = UrlEncodedBody::new()
    .append("username", "john")
    .append("password", "secret123")
    .append("remember", "true");

let response = FlUrl::new("https://api.example.com/login")
    .post(body)
    .await?;
```

### Multipart Form Data

```rust
use flurl::body::new_form_data;

// Form fields (`new_form_data()` generates a random multipart boundary)
let form_data = new_form_data()
    .append_form_data_field("username", "john")
    .append_form_data_field("email", "john@example.com");

let response = FlUrl::new("https://api.example.com/profile")
    .post(form_data)
    .await?;

// Form with file upload
let form_data = new_form_data()
    .append_form_data_field("title", "My Document")
    .append_form_data_file("file", "document.pdf", "application/pdf", &file_bytes);

let response = FlUrl::new("https://api.example.com/upload")
    .post(form_data)
    .await?;
```

### Raw Body

```rust
use flurl::body::HttpRequestBody;

let raw_data = b"custom binary data";
let body = HttpRequestBody::from_raw_data(raw_data.to_vec(), Some("application/octet-stream"));

let response = FlUrl::new("https://api.example.com/upload")
    .post(body)
    .await?;
```

Every body above is held in memory as a whole. A body of any size at constant memory: topic [`streamed-body`](streamed-body.md). A request described by a `MyHttpInput` model: topic [`model-driven-requests`](model-driven-requests.md).

## Response Handling

### Get Status Code

```rust
let mut response = FlUrl::new("https://api.example.com/data")
    .get()
    .await?;

let status_code = response.get_status_code();
println!("Status: {}", status_code);
```

### Read the Body

A response arrives as soon as its head is read: the status and the headers are there,
the body is not read yet. `get_body()` takes the body out of the response as its
reader, and how the body is read is up to the reader.

```rust
let mut response = FlUrl::new("https://api.example.com/data")
    .get()
    .await?;

// the whole body, 1 MB at most
let body: Vec<u8> = response.get_body()?.into_vec(1024 * 1024).await?;
println!("Body length: {}", body.len());
```

As a string:

```rust
let body: String = response.get_body()?.into_string(1024 * 1024).await?;
println!("Body: {}", body);
```

As JSON:

```rust
use serde::Deserialize;

#[derive(Deserialize)]
struct ApiResponse {
    data: Vec<String>,
}

let api_response: ApiResponse = response.get_body()?.into_json(1024 * 1024).await?;
```

Piece by piece, as it comes over the network — a body of any size at constant memory,
or one which is read as it is sent, like an event stream:

```rust
let mut body = response.get_body()?;

// `None` is the end of the body
while let Some(piece) = body.next_item().await? {
    println!("Received {} bytes", piece.len());
}
```

| method of the reader | gives |
| --- | --- |
| `next_item()` | the next piece of the body (`&[u8]`), good until the next call; `None` at its end |
| `into_vec(max_size)` | the whole body as `Vec<u8>` — what is left of it, when a part is taken by `next_item` already |
| `into_string(max_size)` | the whole body as a `String` |
| `into_json::<T>(max_size)` | the whole body deserialized from JSON |
| `content_length()` | the size of the body when the response says it |
| `remains_to_read()` | how much of it is not read yet |

- **`max_size` is the limit of the body in memory, in bytes, and there is no other.**
  A body which says it is bigger is refused before a byte of it is read; one which does
  not say is cut off as soon as it grows past the limit. Either way the read fails with
  `FlUrlError::ResponseBodyTooLarge { limit }`. `usize::MAX` takes a body of any size.
  See [Response Body Size](timeouts-and-limits.md#response-body-size).
- **The body is taken once.** `get_body()` moves it out of the response, which keeps
  its status and its headers; a second call fails.
- A body which is cut short ends with an error and keeps answering with it, so a part
  of a body never passes for a whole one.
- **A piece is good until the next call** of `next_item`, which lets go of it: in
  `FlUrlMode::Http1NoHyper` a piece is a part of one of the two buffers the connection
  is read into, and the buffer is read into again while the next piece is waited for.
  What is needed for longer is copied out of it: `piece.to_vec()`.
- A reader dropped before the end of the body — and a response dropped with its body
  in it — ends the download: natively the connection is closed instead of going back to
  the pool (in `FlUrlMode::H2` only that stream is dropped), under wasm the `fetch` is
  aborted.
- With `accept_gzip()` the reader gives the body decoded, however it is read — see
  [Compression](compression.md#response-decompression).
- What bounds the read in time: [Timeouts](timeouts-and-limits.md#timeouts).

`FlUrlBodyReader` has the same methods on both backends. Natively it is the reader of
my-http-client with what a request of FlUrl adds to it — the connection of the pool,
the timeout, the decoding; under wasm it reads the stream of the `fetch` response.

Natively it is a `rust_extensions::AsyncBytesStream<FlUrlError>` as well, for code
which reads any stream of bytes. Its `get_next()` gives a piece of its own, a
`FlUrlBodyPiece` — a `&[u8]` through `Deref`, decoded when `accept_gzip()` asked for it.
Such a piece can be kept while the next one is asked for, and is let go of soon all the
same: a reader which keeps pieces of both read buffers of the connection holds the
reading back until it lets one go.

### Pass the Response On

Native only. `into_hyper_response()` turns the response into hyper's, its body still
streaming from the connection as it came — nothing is decoded, and the trailers of an
HTTP/2 response are passed on. It is what a proxy answers its own caller with:

```rust
let response = FlUrl::new("https://upstream.example.com/data")
    .get()
    .await?;

let response: hyper::Response<_> = response.into_hyper_response();
```

The connection goes back to the pool once that body is read to its end.

hyper's `collect()` keeps every frame of the body until its end, so in
`FlUrlMode::Http1NoHyper` a body bigger than the two read buffers of the connection
(64 KB each) never ends that way. A body is read into memory with
`get_body()?.into_vec(max_size)`.

### Get Headers

```rust
let mut response = FlUrl::new("https://api.example.com/data")
    .get()
    .await?;

// Get specific header
let content_type = response.get_header("Content-Type")?;

// Get header case-insensitive
let content_type = response.get_header_case_insensitive("content-type")?;

// Get all headers. The names come back lower-cased: `content-type`
let headers = response.get_headers();
for (key, value) in headers {
    println!("{}: {:?}", key, value);
}
```

### Redirects

FlUrl does not follow redirects. A `3xx` answer comes back as it is, with its
`Location` header, and no request goes to the address it names:

```rust
let mut response = FlUrl::new("https://api.example.com/old-path")
    .get()
    .await?;

if (300..400).contains(&response.get_status_code()) {
    let location = response.get_header("Location")?;
    println!("Moved to: {:?}", location);
}
```

To go where the redirect points, read `Location` and make the next request yourself.
A relative `Location` (`/other`) is resolved against the url of the request. When the
new address is on another host, do not carry over `Authorization`, `Cookie` and
other credentials that were meant for the first one.

Under wasm the request is made by the browser's `fetch`, which follows redirects
itself: the response you get is the one at the end of the redirects, not the
redirect.

## Error Handling

```rust
use flurl::{FlUrl, FlUrlError};

match FlUrl::new("https://api.example.com/data").get().await {
    Ok(response) => {
        // A response arrived — whatever its status code
    }
    Err(FlUrlError::Timeout) => {
        // No response within `set_timeout`
    }
    Err(FlUrlError::InvalidUrl(reason)) | Err(FlUrlError::UnsupportedScheme(reason)) => {
        // The url can not be used, or this build can not serve its scheme
    }
    Err(FlUrlError::RequestBuild(reason)) => {
        // The request could not be built — nothing was sent
    }
    Err(err) => {
        // Everything else, a failed connection included
        eprintln!("Error: {}", err);
    }
}
```

- **The errors of the builder come out here too** — a url that can not be used, a
  header that must not go on the wire, a client certificate for a url that is not
  `https://`. See [Errors Come From the Request](#errors-come-from-the-request).
- **A status code is not an error.** `404` and `500` come back as `Ok(response)`;
  read `response.get_status_code()`.
- **`FlUrlError` is `#[non_exhaustive]`**, so a `match` on it needs a catch-all arm.
- **A failed connection** — refused, reset, a TLS handshake that did not go through —
  is `FlUrlError::MyHttpClientError` on native (its payload is
  `flurl::my_http_client::MyHttpClientError`) and `FlUrlError::FetchError` under
  wasm. Each variant exists on its own backend only, so code meant for both leaves
  them to the catch-all arm.
- **`err.is_timeout()`** is `true` for a timeout however it was reported.
- **Reading the body has errors of its own**: `into_json` fails with
  `FlUrlError::SerializationError` on a body that is not the expected JSON,
  `into_string` with `FlUrlError::CanNotConvertToUtf8`, any read with
  `FlUrlError::Timeout` once [its time](timeouts-and-limits.md#timeouts) runs out, and
  a read of the whole body with `FlUrlError::ResponseBodyTooLarge { limit }` on a body
  over [the limit it was given](#read-the-body).

## Examples

### Complete Example: API Client

```rust
use flurl::{FlUrl, body::HttpRequestBody};
use serde::{Deserialize, Serialize};

#[derive(Serialize)]
struct CreateUser {
    name: String,
    email: String,
}

#[derive(Deserialize)]
struct User {
    id: u64,
    name: String,
    email: String,
}

async fn create_user(name: &str, email: &str) -> Result<User, Box<dyn std::error::Error>> {
    let user_data = CreateUser {
        name: name.to_string(),
        email: email.to_string(),
    };
    
    let mut response = FlUrl::new("https://api.example.com")
        .append_path_segment("users")
        .with_header("Authorization", "Bearer token123")
        .post(HttpRequestBody::as_json(&user_data))
        .await?;
    
    let user: User = response.get_body()?.into_json(1024 * 1024).await?;
    Ok(user)
}

async fn get_user(id: u64) -> Result<User, Box<dyn std::error::Error>> {
    let mut response = FlUrl::new("https://api.example.com")
        .append_path_segment("users")
        .append_path_segment(id.to_string())
        .with_header("Authorization", "Bearer token123")
        .get()
        .await?;
    
    let user: User = response.get_body()?.into_json(1024 * 1024).await?;
    Ok(user)
}
```

## Thread Safety

- A `FlUrl` is one request: every builder method and every method that sends takes it by value, so an instance is never shared — build a new one per request
- On native `FlUrl` is `Send + Sync` and the futures of its requests are `Send`, so a request can be built in one task and sent from another, or handed to `tokio::spawn`. Under wasm the futures are `!Send`
- Connection cache (`FlUrlHttpConnectionsCache`) is thread-safe
- Multiple async tasks can safely use different `FlUrl` instances concurrently

## Topics

| Topic | What is inside |
| --- | --- |
| [`wasm`](wasm.md) | The same API on `wasm32` over the browser `fetch`: what is a no-op there, what is native-only, `with-wasm-compression`, `!Send` futures, root-relative urls resolved against the page origin |
| [`streamed-body`](streamed-body.md) | A request body of any size at constant memory (native only): `*_request_streamed` with `RequestBodyStream`, `Content-Length` vs chunked framing, what does not apply to a streamed body, a model whose body is a stream |
| [`model-driven-requests`](model-driven-requests.md) | `execute_request` with a `MyHttpInput` model filling the path, query, headers and body; `EmptyRequestModel` for a request without a model |
| [`connections`](connections.md) | Connection reuse and the pool, when a connection is dropped, the idle timeout, custom connection caches, HTTP/2 vs HTTP/1.1 modes |
| [`tls`](tls.md) | Accepting invalid certificates (`dangerous-tls`) and client certificates from PKCS#12; what a TLS provider feature is needed for |
| [`ssh`](ssh.md) | SSH tunnels (`with-ssh`): `ssh://…->http(s)://…` urls, password, private key, credentials resolver, a unix socket on the ssh server |
| [`unix-socket`](unix-socket.md) | Requests to a unix socket: every form the socket file can be named in |
| [`known-ip`](known-ip.md) | `https://name@ip/path`: connect to a known ip without DNS while the Host header, SNI and certificate check stay on the name (native only) |
| [`timeouts-and-limits`](timeouts-and-limits.md) | `set_timeout`, `set_response_body_timeout`, what each of them bounds, and how a response body is held to a size limit |
| [`retries`](retries.md) | `with_retries(n)` vs `with_retry(delay, n)`: which outcomes are replayed, idempotent methods only, what the last attempt returns, how long it can take |
| [`compression`](compression.md) | `compress()` / `compress_with_level()` of a request body, `with-wasm-compression` for them under wasm, and `accept_gzip()` for the response |
| [`debugging`](debugging.md) | `print_input_request()` and the `*_with_debug` twin of every request method |
