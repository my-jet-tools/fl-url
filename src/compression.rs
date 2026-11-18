/// How hard `compress_with_level` works at a request body: the time it takes to
/// compress traded for the size of what goes on the wire. It is my-http-utils' own
/// type, since my-http-utils gzips the body (`HttpRequestBody::compress_with_level`).
/// The same on both backends: under wasm `compress` and `compress_with_level` come
/// with the `with-wasm-compression` feature, this type is there without it as well.
pub use my_http_utils::body::CompressionLevel;
