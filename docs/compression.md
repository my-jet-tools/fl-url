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

- `compress()` and `compress_with_level` are only applied if the body size is >= 64 bytes; a shorter body goes out as it is
- Uses gzip compression
- Sets the `Content-Encoding: gzip` header, unless the request already carries a `Content-Encoding`
- Compression threshold can be adjusted by modifying the source code

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
