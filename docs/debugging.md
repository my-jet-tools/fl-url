# Debugging requests

## Debug Request Output

```rust
let response = FlUrl::new("https://api.example.com/data")
    .print_input_request() // Prints HTTP headers to stdout
    .get()
    .await?;
```

Under wasm it writes the method and the url to the browser console instead.

## Request Debug String

Every request method has a `*_with_debug` twin that takes a `&mut String` as its last
argument and fills it with the request as it goes on the wire — verb, path and query,
headers, and the body:

```rust
let mut debug_string = String::new();
let body = HttpRequestBody::as_json(&my_data);
let response = FlUrl::new("https://api.example.com/data")
    .post_with_debug(body, &mut debug_string)
    .await?;
println!("Request details: {}", debug_string);
```

The two backends lay the headers out differently. Native ends every header with a
CRLF, so the dump takes several lines:

```
[POST] PathAndQuery: '/data'; Headers: 'Content-Type: application/json
Body: {"a":1}
```

Under wasm it is one line:

```
[POST] PathAndQuery: '/data'; Headers: 'Content-Type: application/json; 'Body: {"a":1}
```

| method | debug twin |
| --- | --- |
| `get()` | `get_with_debug(&mut s)` |
| `head()` | `head_with_debug(&mut s)` |
| `delete()` | `delete_with_debug(&mut s)` |
| `post(body)` | `post_with_debug(body, &mut s)` |
| `put(body)` | `put_with_debug(body, &mut s)` |
| `patch(body)` | `patch_with_debug(body, &mut s)` |
| `execute_request(verb, model)` | `execute_request_with_debug(verb, model, &mut s)` |
| `post_request_streamed(body, len)` | `post_request_streamed_with_debug(body, len, &mut s)` |
| `put_request_streamed(body, len)` | `put_request_streamed_with_debug(body, len, &mut s)` |
| `patch_request_streamed(body, len)` | `patch_request_streamed_with_debug(body, len, &mut s)` |
| `execute_streamed(method, body, len)` | `execute_streamed_with_debug(method, body, len, &mut s)` |

The dump is written **before** compression, so `compress()` does not turn it into
gzip noise — what you read is what you sent.

A body sent with [`encrypted(&key)`](encryption.md) is the other way round: it is
encrypted because it carries secrets, and a debug string ends up in a log more often
than not, so the dump gets `Body: <n> bytes, sent encrypted` instead of the text.

The streamed variants are the one exception to "the body is in the dump": a streamed
payload exists only as it is written to the socket, so printing it would mean
buffering the very thing streaming avoids. Their dump is the request head alone.

The `IntoFlUrl` shortcuts on `&str` / `String` — `get`, `head`, `delete`, `post` and
`put`; there is no `patch` among them — carry their twins too, so
`"https://api.example.com/data".get_with_debug(&mut debug_string).await?` works as well.

Everything except the streamed methods (which are native-only) exists on both the
native and the wasm backend.
