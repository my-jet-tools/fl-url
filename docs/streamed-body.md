# Streamed Body (native only)

Every body above is a `Vec<u8>`: the payload exists in memory as a whole, and a large
upload costs its own size in RSS — twice, if the caller also read a file to build it.
For a body that must not be materialized, `post_request_streamed` /
`put_request_streamed` / `patch_request_streamed` take anything implementing
`hyper::body::Body<Data = Bytes>` and write it to the socket as it is produced. Peak
memory is one chunk plus whatever the producer buffers, whatever the size of the body.

`RequestBodyStream` is the usual producer — a body over an mpsc channel, where the
channel is the backpressure: `publish` waits once `buffer` chunks are queued for the
socket. **Dropping the publisher is what ends the body.** It comes from my-http-client,
which is re-exported as `flurl::my_http_client` (native only) — no line for it in
`Cargo.toml`.

```rust
use flurl::my_http_client::RequestBodyStream;

let (publisher, body) = RequestBodyStream::new(4);

tokio::spawn(async move {
    while let Some(chunk) = source.next().await {
        // Err means the request is over — nothing else can be published
        if publisher.publish(chunk).await.is_err() {
            break;
        }
    }
    // dropping the publisher ends the body
});

let response = FlUrl::new("https://api.example.com")
    .append_path_segment("upload")
    .set_timeout(Duration::from_secs(600))
    // None: the size is unknown, so the body goes out chunked
    .post_request_streamed(body, None)
    .await?;
```

A proxied `hyper::body::Incoming`, a `StreamBody` over a file reader, or any other
`Body` implementation works just as well. `hyper` itself is re-exported as
`flurl::hyper`.

## Framing: the `content_length` argument

HTTP/1.1 offers exactly two ways to delimit a request body, and the last argument
picks between them:

| argument | framing | when |
| --- | --- | --- |
| `None` | `Transfer-Encoding: chunked` | the size is genuinely unknown. Every HTTP/1.1 recipient is required to understand chunked, so this is the right default |
| `Some(n)` | `Content-Length: n` | the size is known, or the endpoint refuses a chunked request body |

```rust
let response = FlUrl::new("https://api.example.com")
    .append_path_segment("files")
    .append_path_segment("archive.tar")
    .set_timeout(Duration::from_secs(600))
    .put_request_streamed(body, Some(len))
    .await?;
```

What lands on the wire is then plain length framing:

```
PUT /files/archive.tar HTTP/1.1
content-length: 524288
host: api.example.com
```

With `Some(n)` the body must then deliver **exactly** `n` bytes. That is not an fl-url
rule but the protocol's: a short body makes the message incomplete, and extra bytes
would be read as the start of the next request on the same connection. A stream that
ends early therefore fails the request with `"user body write aborted"` instead of
leaving a truncated payload behind — so `n` and the producer have to come from one
source (a file's metadata and that same file), never be computed twice.

The argument is the single source of the framing, so it overrides a `Content-Length`
added with `with_header` in **both** directions: `Some(n)` replaces such a header
(never emits a second one, which would be a protocol violation), and `None` removes
it — a body of unknown size must not claim a length it may not deliver.

## What does not apply to a streamed body

| knob | what happens |
| --- | --- |
| `compress()`, `compress_with_level(..)` | `FlUrlError::StreamedBodyCanNotBeCompressed` — gzip needs the whole body in one buffer, which is exactly what streaming avoids |
| `with_retries(n)`, `with_retry(d, n)` | ignored: the payload is consumed as it is sent, so the request is attempted **exactly once**. Rebuilding the stream and calling again is the caller's decision — it owns the source data |
| `set_timeout(d)` | now covers the **whole** call, upload included, not just the wait for the response head. The 10s default is far too short for a real upload |
| `update_mode(..)` | ignored: the mode is pinned to `Http1Hyper`, since the own HTTP/1.1 implementation serializes a request into one buffer and the h2 client has no streaming entry point |

These methods are native-only — the browser `fetch` API cannot stream a request body
without HTTP/2 duplex, so there is no wasm counterpart.

## A Model That Streams Its Body (native only)

A model whose body field is marked `#[http_body_as_stream]` is sent the streamed way
by the very same `execute_request` — the chunks the application writes into the
stream go to the socket as they arrive, and the payload is never materialized. It is
the same model the server parses the incoming body with, used from the other end.

```rust
use flurl::my_http_utils;
use flurl::{FlUrl, HttpVerb};
use my_http_utils::http_input::HttpBodyAsStream;
use my_http_utils::macros::MyHttpInput;

#[derive(MyHttpInput)]
struct UploadHttpInput {
    #[http_path(name = "fileName", description = "File name")]
    file_name: String,
    #[http_header(name = "X-Api-Key", description = "Api key")]
    api_key: String,
    #[http_body_as_stream(description = "File content")]
    body: HttpBodyAsStream,
}

// `4` is the channel capacity — the back-pressure knob; `None` = the size is not
// known up front, so the body goes out chunked.
let (sender, stream) = HttpBodyAsStream::create(4, None);

tokio::spawn(async move {
    while let Some(chunk) = source.next().await {
        // false means the transport is gone — nothing left to write into
        if !sender.send_chunk(chunk).await {
            return;
        }
    }
    // Marks the body complete. WITHOUT it a dropped sender reads as a producer that
    // died half-way, and the request fails instead of sending a truncated payload.
    sender.finish();
});

let response = FlUrl::new("https://api.example.com")
    .append_path_segment("upload")
    .with_header("Content-Type", "application/octet-stream")
    .set_timeout(Duration::from_secs(600))
    .execute_request(HttpVerb::Post, UploadHttpInput {
        file_name: "archive.tar".to_string(),
        api_key: "secret".to_string(),
        body: stream,
    })
    .await?;
```

The framing is not a separate argument here — it comes from the stream, so it can
not drift out of step with the payload: the `content_length` given to
`HttpBodyAsStream::create` becomes `Content-Length: n`, and `None` goes out chunked.
Everything else listed under [What does not apply to a streamed
body](#what-does-not-apply-to-a-streamed-body) applies unchanged — no `compress()`,
no retries, and `set_timeout` covers the whole upload.

Two cases fail the request rather than quietly sending something else:

| case | why |
| --- | --- |
| `Get` / `Delete` / `Head` | a materialized body is merely dropped for these verbs, but dropping a *stream* would leave the application writing into something nothing will ever read |
| `HttpBodyAsStream::empty()` | what a model carries when it is only ever parsed by a server; sending it would produce a request with no body at all |

**wasm**: the browser `fetch` API has no portable streamed request body — a
`ReadableStream` body needs Chromium 105+ over HTTP/2+, and Firefox and Safari do not
support it at all — so `execute_request` with such a model returns
`FlUrlError::RequestBuild` there instead of sending an empty body. Where the payload
is a file the user picked, streaming it by hand is not needed anyway: handing the
`File`/`Blob` straight to `fetch` makes the browser stream it from disk itself, at
constant memory, in every browser.
