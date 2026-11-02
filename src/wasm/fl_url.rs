use std::sync::Arc;
use std::time::Duration;

use my_http_utils::UrlBuilder;
use rust_extensions::StrOrString;

use super::fl_url_inner::FlUrlInner;
use crate::body::HttpRequestBody;
use crate::wasm::{FlUrlHttpConnectionsCache, FlUrlResponse};
#[cfg(feature = "with-wasm-compression")]
use crate::CompressionLevel;
use crate::FlUrlError;

/// Kept for API parity with the native backend. Under wasm the browser negotiates
/// the HTTP version, so the mode is stored but otherwise ignored.
#[derive(Debug, Clone, Copy)]
pub enum FlUrlMode {
    H2,
    Http1NoHyper,
    Http1Hyper,
}

impl FlUrlMode {
    pub fn is_h2(&self) -> bool {
        matches!(self, Self::H2)
    }
}

impl Default for FlUrlMode {
    fn default() -> Self {
        Self::Http1Hyper
    }
}

/// HTTP verb selector for [`FlUrl::execute_request`].
#[derive(Clone, Copy, Debug)]
pub enum HttpVerb {
    Get,
    Post,
    Put,
    Patch,
    Delete,
    Head,
}

/// The wasm `FlUrl`. Same fluent, builder-style API as the native backend, but
/// backed by the browser `fetch`. Knobs that only make sense for the native
/// transport (connection pool, TLS validation, HTTP mode, connection reuse) are
/// stored for signature parity and otherwise ignored — the browser handles them.
/// `compress` and `compress_with_level` are there only with the
/// `with-wasm-compression` feature, which is what keeps flate2 out of a bundle.
///
/// It is a plain builder — no step of it returns a `Result`. A url that can not be
/// used becomes the state of the builder instead: every step after it is skipped, and
/// the call that sends the request returns that error without sending anything.
pub struct FlUrl {
    // The request, for as long as everything it was given could be used — and the
    // first error once something could not. All the logic is in `FlUrlInner`; this
    // type only decides whether there still is an inner to hand a call to.
    inner: Result<FlUrlInner, FlUrlError>,
}

impl FlUrl {
    /// Never fails and never panics. A url that can not be used becomes the error the
    /// request fails with: [`FlUrlError::InvalidUrl`], or [`FlUrlError::FetchError`]
    /// when a relative url has no page origin to be resolved against.
    pub fn new<'s>(url: impl Into<StrOrString<'s>>) -> Self {
        Self {
            inner: FlUrlInner::new(url),
        }
    }

    /// A step of the builder. Skipped once the builder has met an error.
    fn map(self, step: impl FnOnce(FlUrlInner) -> FlUrlInner) -> Self {
        Self {
            inner: self.inner.map(step),
        }
    }

    /// The error the builder has met, if any — the one the request is going to fail
    /// with. It lets a url that came from settings be checked without sending
    /// anything.
    pub fn get_error(&self) -> Option<&FlUrlError> {
        self.inner.as_ref().err()
    }

    /// The url as it stands: what [`Self::new`] was given — resolved against the page
    /// origin when it was a relative one — plus everything appended to it since.
    /// Once the builder has met an error it is that error instead, so an `unwrap`
    /// says what was wrong with the url.
    pub fn get_url_builder(&self) -> Result<&UrlBuilder, &FlUrlError> {
        self.inner.as_ref().map(|inner| &inner.url_builder)
    }

    /// Gzips a request body of 64 bytes and up at [`CompressionLevel::Default`] and
    /// sends it with `Content-Encoding: gzip`. A shorter body goes out as it is.
    #[cfg(feature = "with-wasm-compression")]
    pub fn compress(self) -> Self {
        self.map(|inner| inner.compress())
    }

    /// [`Self::compress`] at the level given: [`CompressionLevel::Fast`] spends the
    /// least time on the body, [`CompressionLevel::Best`] makes it the smallest. The
    /// later of the two calls wins.
    #[cfg(feature = "with-wasm-compression")]
    pub fn compress_with_level(self, level: CompressionLevel) -> Self {
        self.map(|inner| inner.compress_with_level(level))
    }

    /// No-op under wasm: the browser negotiates `Accept-Encoding` and
    /// transparently decompresses the response, so there is nothing to do here.
    /// Kept for API parity.
    pub fn accept_gzip(self) -> Self {
        self.map(|inner| inner.accept_gzip())
    }

    pub fn set_not_used_connection_timeout(self, timeout: Duration) -> Self {
        self.map(|inner| inner.set_not_used_connection_timeout(timeout))
    }

    pub fn update_mode(self, mode: FlUrlMode) -> Self {
        self.map(|inner| inner.update_mode(mode))
    }

    pub fn set_connections_cache(self, clients_cache: Arc<FlUrlHttpConnectionsCache>) -> Self {
        self.map(|inner| inner.set_connections_cache(clients_cache))
    }

    /// Retries the request up to `max_retries` extra times on failure. Only
    /// idempotent methods are replayed (a POST/PATCH that may have reached the
    /// server is never re-sent).
    ///
    /// Only an attempt that got no response at all (a `Timeout` or a `fetch`
    /// failure) is replayed, and the next one follows at once. A response that did
    /// arrive is the result, 5xx included — [`Self::with_retry`] is the one that
    /// waits out a restarting service. Both set the same policy, so the later of the
    /// two calls wins.
    pub fn with_retries(self, max_retries: usize) -> Self {
        self.map(|inner| inner.with_retries(max_retries))
    }

    /// Rides out a restart of the service behind the url. The request is replayed up
    /// to `amount` more times, `retry_delay` apart, on either face of an outage: no
    /// response at all (a `Timeout` or a `fetch` failure — what
    /// [`Self::with_retries`] replays), or a response with a status of 500 and up,
    /// which is what a reverse proxy answers while the container behind it is being
    /// recreated.
    ///
    /// The same semantics as the native backend: only idempotent methods are
    /// replayed, a 4xx is returned at once, and when the attempts run out the last
    /// one is the result — a 5xx as `Ok(response)`, a transport failure as `Err`.
    /// The pause is a `setTimeout`-driven future, and a 5xx that gets replayed is
    /// never read: its body is aborted through the attempt's `AbortController` before
    /// the pause. Each attempt is still bounded by [`Self::set_timeout`].
    ///
    /// Sets the same policy as [`Self::with_retries`], so the later of the two calls
    /// wins.
    pub fn with_retry(self, retry_delay: Duration, amount: usize) -> Self {
        self.map(|inner| inner.with_retry(retry_delay, amount))
    }

    pub fn print_input_request(self) -> Self {
        self.map(|inner| inner.print_input_request())
    }

    /// Bounds the request up to the response head. 10 seconds by default. Reading the
    /// body is not bounded by it under wasm — natively a read of the whole body is —
    /// but by [`Self::set_response_body_timeout`].
    pub fn set_timeout(self, timeout: Duration) -> Self {
        self.map(|inner| inner.set_timeout(timeout))
    }

    /// Bounds how long reading the response body may take — a read of the whole body
    /// as a whole, a read piece by piece for each piece: the read is aborted through
    /// the request's `AbortController` and fails with [`FlUrlError::Timeout`].
    /// Unbounded by default, as on the native backend.
    pub fn set_response_body_timeout(self, timeout: Duration) -> Self {
        self.map(|inner| inner.set_response_body_timeout(timeout))
    }

    /// No-op under wasm (the browser owns connection reuse). Kept for API parity.
    pub fn do_not_reuse_connection(self) -> Self {
        self.map(|inner| inner.do_not_reuse_connection())
    }

    /// No-op under wasm: server-certificate validation is controlled by the
    /// browser and can not be relaxed from JS. Kept for API parity.
    pub fn accept_invalid_certificate(self) -> Self {
        self.map(|inner| inner.accept_invalid_certificate())
    }

    pub fn append_path_segment<'s>(self, path_segment: impl Into<StrOrString<'s>>) -> Self {
        self.map(|inner| inner.append_path_segment(path_segment))
    }

    pub fn append_query_param<'n, 'v>(
        self,
        param_name: impl Into<StrOrString<'n>>,
        value: Option<impl Into<StrOrString<'v>>>,
    ) -> Self {
        self.map(|inner| inner.append_query_param(param_name, value))
    }

    /// The header goes to the browser as it is: what may be sent is the browser's to
    /// decide, and a header it refuses fails the request with
    /// [`FlUrlError::FetchError`].
    pub fn with_header<'n, 'v>(
        self,
        name: impl Into<StrOrString<'n>>,
        value: impl Into<StrOrString<'v>>,
    ) -> Self {
        self.map(|inner| inner.with_header(name, value))
    }

    pub fn append_raw_ending_to_url<'r>(self, raw: impl Into<StrOrString<'r>>) -> Self {
        self.map(|inner| inner.append_raw_ending_to_url(raw))
    }

    /// Executes an HTTP request described by a `my_http_utils` request model (any
    /// type deriving `my_http_utils::macros::MyHttpInput`). Mirrors the native
    /// backend.
    ///
    /// For a parameter-less request, pass [`crate::EmptyRequestModel`] instead of
    /// deriving a dedicated model — the URL/headers already set on `self` are used
    /// as-is and body-carrying verbs send an empty body.
    ///
    /// `Get`/`Delete`/`Head` do not carry a body, so a body produced by the model
    /// is ignored for those verbs.
    pub async fn execute_request(
        self,
        verb: HttpVerb,
        model: impl my_http_utils::schema::client::THttpRequestBuilder,
    ) -> Result<FlUrlResponse, FlUrlError> {
        self.inner?.execute_request(verb, model).await
    }

    /// Same as [`Self::execute_request`], but dumps the compiled request — verb,
    /// path and query, headers and body — into `request_debug_string` before it goes
    /// on the wire. The dump is written for every verb, body-carrying or not.
    pub async fn execute_request_with_debug(
        self,
        verb: HttpVerb,
        model: impl my_http_utils::schema::client::THttpRequestBuilder,
        request_debug_string: &mut String,
    ) -> Result<FlUrlResponse, FlUrlError> {
        self.inner?
            .execute_request_with_debug(verb, model, request_debug_string)
            .await
    }

    pub async fn get(self) -> Result<FlUrlResponse, FlUrlError> {
        self.inner?.get().await
    }

    pub async fn get_with_debug(
        self,
        request_debug_string: &mut String,
    ) -> Result<FlUrlResponse, FlUrlError> {
        self.inner?.get_with_debug(request_debug_string).await
    }

    pub async fn head(self) -> Result<FlUrlResponse, FlUrlError> {
        self.inner?.head().await
    }

    pub async fn head_with_debug(
        self,
        request_debug_string: &mut String,
    ) -> Result<FlUrlResponse, FlUrlError> {
        self.inner?.head_with_debug(request_debug_string).await
    }

    pub async fn post(
        self,
        body: impl Into<HttpRequestBody>,
    ) -> Result<FlUrlResponse, FlUrlError> {
        self.inner?.post(body).await
    }

    pub async fn post_with_debug(
        self,
        body: impl Into<HttpRequestBody>,
        request_debug_string: &mut String,
    ) -> Result<FlUrlResponse, FlUrlError> {
        self.inner?.post_with_debug(body, request_debug_string).await
    }

    #[deprecated(note = "Use `post` instead")]
    pub async fn post_json(
        self,
        json: &impl serde::Serialize,
    ) -> Result<FlUrlResponse, FlUrlError> {
        self.inner?.post_json(json).await
    }

    pub async fn patch(
        self,
        body: impl Into<HttpRequestBody>,
    ) -> Result<FlUrlResponse, FlUrlError> {
        self.inner?.patch(body).await
    }

    pub async fn patch_with_debug(
        self,
        body: impl Into<HttpRequestBody>,
        request_debug_string: &mut String,
    ) -> Result<FlUrlResponse, FlUrlError> {
        self.inner?.patch_with_debug(body, request_debug_string).await
    }

    #[deprecated(note = "Use `patch` instead")]
    pub async fn patch_json(
        self,
        json: &impl serde::Serialize,
    ) -> Result<FlUrlResponse, FlUrlError> {
        self.inner?.patch_json(json).await
    }

    pub async fn put(
        self,
        body: impl Into<HttpRequestBody>,
    ) -> Result<FlUrlResponse, FlUrlError> {
        self.inner?.put(body).await
    }

    pub async fn put_with_debug(
        self,
        body: impl Into<HttpRequestBody>,
        request_debug_string: &mut String,
    ) -> Result<FlUrlResponse, FlUrlError> {
        self.inner?.put_with_debug(body, request_debug_string).await
    }

    #[deprecated(note = "Use `put` instead")]
    pub async fn put_json(
        self,
        json: &impl serde::Serialize,
    ) -> Result<FlUrlResponse, FlUrlError> {
        self.inner?.put_json(json).await
    }

    pub async fn delete(self) -> Result<FlUrlResponse, FlUrlError> {
        self.inner?.delete().await
    }

    pub async fn delete_with_debug(
        self,
        request_debug_string: &mut String,
    ) -> Result<FlUrlResponse, FlUrlError> {
        self.inner?.delete_with_debug(request_debug_string).await
    }

    /// The path and query and the headers of the request as they stand — or, once
    /// the builder has met an error, that error.
    pub fn to_string(&self) -> String {
        match self.inner.as_ref() {
            Ok(inner) => inner.to_string(),
            Err(err) => format!("Error: {}", err),
        }
    }
}
