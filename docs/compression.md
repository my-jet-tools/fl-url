# Compression

## Request Compression

```rust
let body = HttpRequestBody::as_json(&large_data);
let response = FlUrl::new("https://api.example.com/data")
    .compress() // gzips the body when it is 64 bytes or longer
    .post(body)
    .await?;
```

## Compression Level

`compress()` compresses at gzip's default level. `compress_with_level` takes the level
as a `CompressionLevel`:

```rust
use flurl::{CompressionLevel, FlUrl};

let response = FlUrl::new("https://api.example.com/data")
    .compress_with_level(CompressionLevel::Best)
    .post(body)
    .await?;
```

| `CompressionLevel` | gzip level | what for |
| --- | --- | --- |
| `Fast` | 1 | the least CPU time, the least gain — a big body sent often |
| `Default` | 6 | the balance; what `compress()` uses |
| `Best` | 9 | the smallest body, the most CPU time — a slow or metered link |

The later of `compress()` and `compress_with_level` wins. The level means the same on
native and under wasm.

## Body Compression

The body is gzipped by my-http-utils: `compress()` is `HttpRequestBody::compress`
applied to the body the request is sent with.

- A body of 64 bytes and up is gzipped; a shorter body only gets bigger gzipped, so it
  goes out as it is. The threshold is my-http-utils'.
- The request goes out with `Content-Encoding: gzip`. A `Content-Encoding` the caller
  set with `with_header` stays, with `gzip` after it on the same line: `br, gzip`.
- With [`encrypted(&key)`](encryption.md) the body is gzipped first, then encrypted:
  `Content-Encoding: gzip, aes128gcm`.
- A streamed body is refused with `FlUrlError::StreamedBodyCanNotBeCompressed`.

## A body that comes compressed

The body can be gzipped before it is given to `post`, `put` or `patch`:

```rust
use flurl::body::HttpRequestBody;
use flurl::CompressionLevel;

let body = HttpRequestBody::as_json(&large_data).compress_with_level(CompressionLevel::Best);

let response = FlUrl::new("https://api.example.com/data")
    .post(body)
    .await?;
```

It goes out as it is, with the `Content-Type` of the body as it was made and
`Content-Encoding: gzip`. `.compress()` on the builder changes nothing on it: it is
gzipped once. A body that comes encrypted is not compressed either, since encrypted
data does not compress. See [Encrypted Request Body](encryption.md#a-body-that-comes-compressed-or-encrypted).

## Request Compression under wasm

Under wasm `compress()` and `compress_with_level` come only with the
`with-wasm-compression` feature:

```toml
[dependencies]
flurl = { tag = "0.7.0", git = "https://github.com/MyJetTools/fl-url.git", features = [
    "with-wasm-compression",
] }
```

Without the feature the two methods are not there, so a call is a compile error, not a
request that quietly goes out uncompressed. `CompressionLevel` stays public either way.

The feature turns on my-http-utils' `compression`, which gzips the body. It is off by
default because whether a body is compressed is decided at run time: the linker can not
see that nothing calls `compress()`, so it can not drop the gzip code, and every wasm
client would carry flate2 and crc32fast. The sixteen 1 KB tables of crc32fast hardly
compress: 20 KB over the wire, measured on a trading terminal (brotli q11).

A body that comes compressed goes out with its `Content-Encoding` with or without the
feature: another crate in the build can turn my-http-utils' `compression` on and hand
such a body over.

The native backend does not depend on the feature: it always turns my-http-utils'
`compression` on, so it always has `compress()`. Turning the feature on changes nothing
there, so a crate built for both targets that calls `compress()` just turns it on, with
no `cfg` of its own. `accept_gzip()` needs no feature on either backend.

## Response Decompression

```rust
let mut response = FlUrl::new("https://api.example.com/data")
    .accept_gzip()
    .get()
    .await?;

let body = response.get_body()?.into_vec(1024 * 1024).await?; // already decompressed
```

`accept_gzip()` sends `Accept-Encoding: gzip`, unless the request already carries that
header, and has the reader of the body give a `Content-Encoding: gzip` body decoded —
whichever way it is read: `into_vec`, `into_string`, `into_json`, or piece by piece
with `next_item`.

Nothing is collected to be decoded: gzip is a stream, and the decoder takes the pieces
as they come and gives what they decode into at once, holding nothing but its own state
(the 32 KB window of deflate) whatever the size of the body. A piece may decode into
nothing yet — the head of gzip is not all there — and such a piece gives nothing; one
which decodes into a lot is given in pieces of about 64 KB.

- What the body checks itself with — the CRC and the size at the end of gzip — comes
  last: a body which does not check out ends with an error after its data is given.

- The limit of a read holds the decoded body: a few kilobytes of gzip can inflate into
  gigabytes, and the read fails with `FlUrlError::ResponseBodyTooLarge` once what it
  has decoded is over the limit.
- The reader of a decoded body does not know its size: `content_length()` and
  `remains_to_read()` are `None`, since `Content-Length` counts the encoded bytes.
- The headers of the response stay as they came — `Content-Encoding: gzip` included.
- `into_hyper_response()` passes the body on as it came, encoded.
- Under wasm the method does nothing: the browser negotiates and decompresses by
  itself.
