# FLUrl

A fluent, async HTTP client library for Rust, inspired by the .NET Flurl library (https://flurl.dev/).

FLUrl is a Hyper-based HTTP client that provides a fluent API for building and executing HTTP requests with connection pooling, retry logic, and comprehensive body type support. The same API compiles to `wasm32-unknown-unknown` on top of the browser `fetch`.

## Install

`flurl` is not published on crates.io — it is pulled from git, pinned to a release tag:

```toml
[dependencies]
flurl = { tag = "0.7.0", git = "https://github.com/MyJetTools/fl-url.git" }
```

`https://` needs a TLS provider feature — `with-ring-tls` or `with-rust-tls`; with neither, an `https://` request fails with `FlUrlError::UnsupportedScheme`. TLS is decided once for the whole application. The features and how to choose between them: [Installation](docs/index_resource.md#installation).

Under `wasm32` `.compress()` comes only with the `with-wasm-compression` feature, so that a client which never compresses a request body does not carry flate2: it is decided at run time, and the linker can not drop it. The native backend always has it. See [Request Compression under wasm](docs/compression.md#request-compression-under-wasm).

```rust
use flurl::FlUrl;

let mut response = FlUrl::new("https://api.example.com")
    .append_path_segment("users")
    .with_header("Authorization", "Bearer token")
    .get()
    .await?;

let users: Vec<User> = response.get_body()?.into_json(1024 * 1024).await?;
```

## Documentation

The library is documented in [`docs/`](docs). [The basics](docs/index_resource.md) cover everything an ordinary request needs: installation and features, building and sending requests, url building, headers, request bodies, response handling, redirects and errors. Everything else is a topic of its own:

- [**wasm**](docs/wasm.md) — the same API on `wasm32` over the browser `fetch`: what is a no-op there, what is native-only, `with-wasm-compression`, root-relative urls.
- [**streamed-body**](docs/streamed-body.md) — a request body of any size at constant memory, with `Content-Length` or chunked framing (native only).
- [**model-driven-requests**](docs/model-driven-requests.md) — `execute_request` with a `MyHttpInput` model, and `EmptyRequestModel`.
- [**connections**](docs/connections.md) — connection reuse and the pool, when a connection is dropped, HTTP/2 and HTTP/1.1 modes.
- [**tls**](docs/tls.md) — invalid certificates and client certificates.
- [**ssh**](docs/ssh.md) — requests through an SSH tunnel (`with-ssh`).
- [**unix-socket**](docs/unix-socket.md) — requests to a unix socket.
- [**known-ip**](docs/known-ip.md) — connect to a known ip without DNS, keeping the host name for `Host`, SNI and the certificate check.
- [**timeouts-and-limits**](docs/timeouts-and-limits.md) — request and body timeouts, the size limit a response body is read with.
- [**retries**](docs/retries.md) — `with_retries` and `with_retry`.
- [**compression**](docs/compression.md) — gzip of a request body and of a response; under wasm a request body needs `with-wasm-compression`.
- [**debugging**](docs/debugging.md) — printing a request and the `*_with_debug` methods.

## License

See LICENSE file for details.

## Contributing

Contributions are welcome! Please feel free to submit a Pull Request.
