# WebAssembly (WASM) Support

`flurl` is a **single crate that compiles for both native and `wasm32`**, and the
backend is chosen automatically by target — the public API (`FlUrl`,
`FlUrlResponse`, `FlUrlError`, `FlUrlHeaders`, `IntoFlUrl`, `body::*`, …) is the
same on both, so the same call sites compile everywhere:

```rust
use flurl::FlUrl;

// Identical code on native and in the browser:
let mut response = FlUrl::new("https://api.example.com")
    .append_path_segment("users")
    .with_header("Authorization", "Bearer token")
    .get()
    .await?;

let users: Vec<User> = response.get_body()?.into_json(1024 * 1024).await?;
```

## How it is wired

| Target | Backend (`cfg`) | Transport |
| --- | --- | --- |
| non-wasm | `src/non_wasm/` — full hyper/tokio impl | HTTP/1.1 & HTTP/2, TLS, client certs, connection pooling, unix sockets, SSH |
| `wasm32-unknown-unknown` | `src/wasm/` | the browser `fetch` API via `web-sys` |

Neither backend module is public: both alias their types to the crate root — use
`flurl::FlUrl`, never a backend path — and the shared pieces
(`FlUrlError` and the request `body` types) live at the root and are used by both. Native-only dependencies (hyper, tokio, my-tls, …) are
excluded from the wasm build; the wasm build pulls only `web-sys` / `wasm-bindgen`.

Add it to a wasm project exactly like a normal dependency (no extra feature
needed — the target is detected automatically):

```toml
[dependencies]
flurl = { tag = "0.7.0", git = "https://github.com/MyJetTools/fl-url.git" }
```

## What differs under wasm

Because the browser owns the connection pool, TLS and redirects, the following
native knobs are kept for signature parity but are **no-ops** under wasm:
`set_connections_cache`, `accept_invalid_certificate`, `do_not_reuse_connection`,
`update_mode`, `accept_gzip` (the browser decompresses transparently),
`set_not_used_connection_timeout`.

Redirects are followed by the browser, so under wasm you get the response at the end
of them; natively FlUrl returns the redirect as it is — see [Redirects](index_resource.md#redirects).

These **do** work under wasm: `set_timeout` bounds the request→headers round-trip
via `AbortController` + `setTimeout`; `set_response_body_timeout` bounds the body
read on the same signal; `with_retries` and `with_retry` replay idempotent methods
only, with the same semantics as on native (the `with_retry` pause is a
`setTimeout`-driven future); `compress` and `compress_with_level` gzip the request body.

One difference in the timeouts: natively `set_timeout` covers a read of the whole body
as well (`into_vec`, `into_string`, `into_json`), under wasm it ends with the response
head and the body is bounded by `set_response_body_timeout` alone — see
[Timeouts](timeouts-and-limits.md#timeouts).

Native-only surface that is **not available** under wasm (browsers can't express
it): `with_client_certificate` (native + a TLS provider feature), all `*_ssh_*` methods, unix-socket URLs,
`into_hyper_response`, the
`*_request_streamed` / `execute_streamed` methods and `get_resolved_ip`.

Futures returned under wasm are `!Send` (the browser is single-threaded), so drive
them with `wasm_bindgen_futures::spawn_local` / your framework's async context
rather than `tokio::spawn`. The `.await` call sites are unchanged.

## Reading a response body

[`get_body()`](index_resource.md#read-the-body) gives the same reader as on native,
with the same methods. Under wasm it reads the response of `fetch`:

```rust
let mut response = FlUrl::new("/api/export").get().await?;

// the whole body, 10 MB at most
let body = response.get_body()?.into_vec(10 * 1024 * 1024).await?;
```

```rust
let mut body = response.get_body()?;
while let Some(piece) = body.next_item().await? {
    // a piece, as the browser has it from the network - good until the next call
}
```

How the body gets from the browser into wasm memory:

- **Piece by piece** (`next_item`) it comes off the stream of the response
  (`Response.body`). Each piece is a `read()` of the stream: a promise to wait for, and
  a copy of the piece from the memory of JS into the wasm one. The pieces are as big as
  the browser makes them — what a read of the network has brought, as a rule — so the
  cost of crossing over is small next to the network.
- **As a whole** (`into_vec`, `into_string`, `into_json`) it is taken with one
  `Response.arrayBuffer()` — one promise, one copy — whenever it can not turn out to be
  over the limit half way: the response says its size, or the limit is `usize::MAX`.
  A body which may is read piece by piece instead, to be cut off as it grows.

What follows from it:

- A body over the limit is not downloaded to its end: the `fetch` is aborted, so the
  limit keeps the body out of the browser as well, not only out of wasm memory.
- So is the body of a reader dropped before its end, and of a response dropped with its
  body in it.
- A response as it is being sent — an event stream — can be read as it comes, with
  `next_item`.
- The reader is not a `rust_extensions::AsyncBytesStream` under wasm: that trait wants
  futures which are `Send`, and those of JS are not.
- `content_length()` is `None` for a response with a `Content-Encoding`: the browser
  decodes it, the body comes decoded, and `Content-Length` counts the encoded bytes.
  A cross-origin response shows `Content-Encoding` only when the server exposes it
  (`Access-Control-Expose-Headers`); without that the size reported for a compressed
  body is the encoded one — and such a body may come out bigger than it said, which
  fails a read with a limit once it is known.

## Relative URLs (origin resolution, wasm-only)

Under wasm you can pass a **root-relative** URL — anything that does not start with
`http://` or `https://`, e.g. `/api/users` — and FlUrl resolves it against the
current page (or web-worker) origin before the request, exactly the way the browser
resolves a relative `fetch`:

```rust
// In a page served from https://my-app.com:
let response = FlUrl::new("/api/dashboards/v1/ab-books-compare")
    .get()
    .await?;
// → GET https://my-app.com/api/dashboards/v1/ab-books-compare
```

The origin is read via `web-sys` from `Window.location` (or, in a worker,
`WorkerGlobalScope.location`), so no base URL has to be threaded through your code.
Absolute `http(s)://…` URLs are used as-is. The prefix is applied **before** the URL
is parsed, so the parser always sees a well-formed absolute URL.

This resolution is wasm-only: on native there is no ambient origin, so pass an
absolute URL there.
