# Encrypted Request Body

`encrypted(&key)` encrypts the request body and sends it with
`Content-Encoding: aes128gcm` ([RFC 8188](https://www.rfc-editor.org/rfc/rfc8188)).
This is the format of my-reverse-proxy's encrypted calls, and the one my-http-server
decrypts. Native only, behind the `encryption` feature:

```toml
[dependencies]
flurl = { tag = "0.7.0", git = "https://github.com/MyJetTools/fl-url.git", features = [
    "encryption",
] }
```

## The key

The key is my-http-utils' `HttpBodyEncryptionKey`, which my-http-server uses too, so
both ends encrypt the same way. fl-url re-exports my-http-utils, so the key needs no
dependency of its own: `flurl::my_http_utils::HttpBodyEncryptionKey`.

A key is 32 random bytes written as standard base64, 44 characters. Make one with
`openssl rand -base64 32`. `from_base64` checks the length and ignores whitespace around
the text, since a key pasted from a password manager often ends with a line break:

```rust
use flurl::my_http_utils::HttpBodyEncryptionKey;

// At start-up, from settings: a key that is not base64 of 32 bytes fails here, not at
// the first request.
let key = HttpBodyEncryptionKey::from_base64(&settings.network_key)?;
```

The key's `Debug` never prints it, and nothing gives the bytes or the base64 back out.

## Sending an encrypted body

```rust
let response = FlUrl::new("http://127.0.0.1:8000")
    .append_path_segment("api")
    .append_path_segment("store")
    .encrypted(&key)
    .post_json(&request)
    .await?;
```

- **Every body gets a fresh random salt**, so the same body never looks the same twice.
- **A retry resends the bytes already built.** It does not encrypt again, so a server
  that remembers salts (my-reverse-proxy does, to accept each body once) sees the
  replay as a replay. Only idempotent methods are replayed anyway (see
  [Retry Logic](retries.md)).
- **An empty body is sent as it is**, with no `Content-Encoding`. A GET or a DELETE has
  nothing to encrypt, and an encrypted empty body would turn it into a request with a
  body.
- **The plain text is whatever the server expects.** my-reverse-proxy's calls, for one,
  want a JSON with `ts`, the sender's clock in unix seconds. That is the caller's to put
  in, because fl-url does not look inside the body.

## With compression

With [`compress()`](compression.md) the body is gzipped first and encrypted after, since
encrypted data does not compress. It goes out with `Content-Encoding: gzip, aes128gcm`:
the codings in the order they were applied. A body under 64 bytes is not gzipped, and
goes out with `aes128gcm` alone.

The order of the builder calls does not matter: `.compress().encrypted(&key)` and
`.encrypted(&key).compress()` send the same thing.

A `Content-Encoding` the caller set with `with_header` stays, with `aes128gcm` added to
the end of the same line: `br, aes128gcm`. It is one line, not two. HTTP reads two lines
of the header as one list, but a server that reads only the first line would see half of
it.

## The response

The response is **not** decrypted. An encrypted answer comes with
`Content-Encoding: aes128gcm`. Open it with the same key:

```rust
let encrypted = response
    .get_header_case_insensitive("content-encoding")?
    .is_some_and(|value| value.eq_ignore_ascii_case(flurl::my_http_utils::AES128GCM_CONTENT_ENCODING));

let body = response.get_body()?.into_vec(1024 * 1024).await?;

let body = if encrypted { key.decrypt(&body)? } else { body };
```

`decrypt` takes all of the body or nothing. A body opened with another key, damaged, or
cut short is refused as a whole.

## What does not apply

| knob | what happens |
| --- | --- |
| a streamed body (`*_request_streamed`, `execute_streamed`, a model whose body is a stream) | `FlUrlError::StreamedBodyCanNotBeEncrypted`: the body is encrypted as one buffer, which is exactly what streaming avoids |
| `*_with_debug` | the dump gets `Body: <n> bytes, sent encrypted` instead of the text: a body is encrypted because it carries secrets, and a debug string ends up in a log more often than not |
| wasm | not available. `encrypted()` is not part of the wasm `FlUrl`, and the feature does not build for `wasm32`: my-http-utils takes the salt from getrandom, which has no browser backend unless the application turns one on |
