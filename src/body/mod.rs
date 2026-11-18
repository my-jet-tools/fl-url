//! Request body types. These are re-exported straight from `my_http_utils::body`
//! (the single, transport-agnostic definition shared by both backends), so
//! `flurl::body::HttpRequestBody` and `my_http_utils::body::HttpRequestBody` are the
//! same type on native and wasm alike.
pub use my_http_utils::body::{FormDataBody, HttpRequestBody, UrlEncodedBody};

/// What a `*_with_debug` dump says of a body that came compressed or encrypted
/// already: its length and its `Content-Encoding` (`headers`, what
/// `HttpRequestBody::get_headers` gives), never its bytes. They are gzip or
/// ciphertext, and a body is encrypted because it carries secrets.
///
/// It takes the length rather than the body: `HttpRequestBody::Encoded` is there only
/// with my-http-utils' `compression` or `encryption` feature, and a wasm build without
/// `with-wasm-compression` can not name it, although another crate in the build can
/// switch the feature on and hand such a body over.
pub(crate) fn describe_encoded_body(len: usize, headers: &[(&'static str, String)]) -> String {
    let mut result = format!("Body: {} bytes, given encoded", len);

    for (name, value) in headers {
        result.push_str("; ");
        result.push_str(name);
        result.push_str(": ");
        result.push_str(value);
    }

    result
}

/// Supplies randomness to `my_http_utils` request building — currently only the
/// `multipart/form-data` boundary. `my_http_utils` itself carries no RNG (so it stays
/// wasm-safe); FlUrl plugs a target-appropriate generator in here. Used as the
/// `TRnd` type parameter of `THttpRequestBuilder::get_body`.
pub struct FlUrlRnd;

impl my_http_utils::schema::client::RandomStringGenerator for FlUrlRnd {
    fn generate_random_string(len: usize) -> String {
        rand_string(len)
    }
}

/// Builds a multipart [`FormDataBody`] with a randomly generated boundary.
///
/// `my_http_utils::body::FormDataBody::new` takes the random boundary string
/// explicitly (so it stays wasm-safe, with no built-in RNG); we generate it here.
pub fn new_form_data() -> FormDataBody {
    FormDataBody::new(&rand_string(16))
}

/// One implementation for both targets, on top of uuid-v4: `rust_extensions::uuid`
/// resolves to the `uuid` crate natively and to `crypto.randomUUID()` in the
/// browser, so FlUrl carries no RNG dependency of its own.
///
/// Hyphens are dropped — callers want a plain alphanumeric token — leaving 32 hex
/// chars per uuid, then the result is cut to `len`. A multipart boundary only has
/// to be absent from the body of that one request, so this is ample.
fn rand_string(len: usize) -> String {
    let mut result = String::with_capacity(len);

    // One uuid covers len <= 32; the loop only matters if a caller wants more.
    while result.len() < len {
        result.extend(
            rust_extensions::uuid::generate_v4()
                .chars()
                .filter(|c| *c != '-'),
        );
    }

    result.truncate(len); // hex only, so byte length == char count
    result
}
