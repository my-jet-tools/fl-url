use bytes::Bytes;
use hyper::Method;

use rust_extensions::StrOrString;

use std::net::IpAddr;
use std::sync::Arc;
use std::time::Duration;

use my_http_utils::UrlBuilder;

use super::fl_url_inner::FlUrlInner;
use super::FlUrlResponse;
use crate::body::HttpRequestBody;
use crate::CompressionLevel;
use crate::FlUrlError;
use crate::FlUrlHttpConnectionsCache;

#[derive(Debug, Clone, Copy)]
pub enum FlUrlMode {
    H2,
    Http1NoHyper,
    Http1Hyper,
}

impl FlUrlMode {
    pub fn is_h2(&self) -> bool {
        match self {
            Self::H2 => true,
            _ => false,
        }
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

/// A request: built step by step, then sent by one of the verb methods.
///
/// It is a plain builder — no step of it returns a `Result`. What a step can not use
/// — a url that names no host, a header that must not go on the wire, a client
/// certificate for a url that is not `https://` — becomes the state of the builder
/// instead: from the first such error on every step is skipped, and the call that
/// sends the request returns that error without sending anything.
///
/// ```no_run
/// # async fn doc(url_from_settings: &str) -> Result<(), flurl::FlUrlError> {
/// // Whatever is wrong with the url or with the header comes out of `get()`.
/// let response = flurl::FlUrl::new(url_from_settings)
///     .append_path_segment("api")
///     .with_header("X-Api-Key", "secret")
///     .get()
///     .await?;
/// # Ok(())
/// # }
/// ```
pub struct FlUrl {
    // The request, for as long as everything it was given could be used — and the
    // first error once something could not. All the logic is in `FlUrlInner`; this
    // type only decides whether there still is an inner to hand a call to.
    inner: Result<FlUrlInner, FlUrlError>,
}

impl FlUrl {
    /// Never fails and never panics. A url that can not be used becomes the error the
    /// request fails with: [`FlUrlError::InvalidUrl`], or
    /// [`FlUrlError::UnsupportedScheme`] for an ssh tunnel in a build without
    /// `with-ssh`.
    pub fn new<'s>(url: impl Into<StrOrString<'s>>) -> Self {
        Self {
            inner: FlUrlInner::new(url),
        }
    }

    /// A step that can not fail. Skipped once the builder has met an error.
    fn map(self, step: impl FnOnce(FlUrlInner) -> FlUrlInner) -> Self {
        Self {
            inner: self.inner.map(step),
        }
    }

    /// A step that can fail: its error takes the place of the inner state, and every
    /// step after it is skipped.
    fn and_then(self, step: impl FnOnce(FlUrlInner) -> Result<FlUrlInner, FlUrlError>) -> Self {
        Self {
            inner: self.inner.and_then(step),
        }
    }

    /// The error the builder has met, if any — the one the request is going to fail
    /// with. It lets a url that came from settings be checked without sending
    /// anything, at start-up for one.
    pub fn get_error(&self) -> Option<&FlUrlError> {
        self.inner.as_ref().err()
    }

    /// The url as it stands: what [`Self::new`] was given plus everything appended to
    /// it since — or, once the builder has met an error, that error, so an `unwrap`
    /// says what was wrong with the url.
    pub fn get_url_builder(&self) -> Result<&UrlBuilder, &FlUrlError> {
        self.inner.as_ref().map(|inner| &inner.url_builder)
    }

    /// `false` once the builder has met an error.
    #[cfg(all(unix, feature = "with-ssh"))]
    pub fn via_ssh(&self) -> bool {
        match self.inner.as_ref() {
            Ok(inner) => inner.via_ssh(),
            Err(_) => false,
        }
    }

    /// The ip a `scheme://server-name@ip/...` url pinned the connection to — the
    /// socket goes there with no DNS lookup, while `server-name` stays the Host
    /// header and the TLS server name.
    pub fn get_resolved_ip(&self) -> Option<IpAddr> {
        self.inner.as_ref().ok()?.get_resolved_ip()
    }

    /// Gzips a request body of 64 bytes and up at [`CompressionLevel::Default`] and
    /// sends it with `Content-Encoding: gzip`. A shorter body goes out as it is.
    pub fn compress(self) -> Self {
        self.map(|inner| inner.compress())
    }

    /// [`Self::compress`] at the level given: [`CompressionLevel::Fast`] spends the
    /// least time on the body, [`CompressionLevel::Best`] makes it the smallest. The
    /// later of the two calls wins.
    pub fn compress_with_level(self, level: CompressionLevel) -> Self {
        self.map(|inner| inner.compress_with_level(level))
    }

    /// Advertises gzip support to the server (`Accept-Encoding: gzip`) and
    /// has the reader of the body (`get_body`) give a gzip-encoded response body
    /// decoded, whichever way it is read. `into_hyper_response` passes the body on as
    /// it came.
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
    /// IDEMPOTENT methods are replayed (a POST that may have reached the server
    /// is never re-sent). Note that my-http-client performs its own internal
    /// reconnect/retry cycles per attempt, so each outer retry is a full fresh
    /// cycle on top of those — keep this number small.
    ///
    /// Only an attempt that got no response at all is replayed, and the next one
    /// follows at once. A response that did arrive is the result, 5xx included —
    /// [`Self::with_retry`] is the one that waits out a restarting service. Both set
    /// the same policy, so the later of the two calls wins.
    pub fn with_retries(self, max_retries: usize) -> Self {
        self.map(|inner| inner.with_retries(max_retries))
    }

    /// Rides out a restart of the service behind the url. The request is replayed up
    /// to `amount` more times, `retry_delay` apart, on either face of an outage:
    ///
    /// * no response at all — the transport failures [`Self::with_retries`] replays
    ///   (connection refused or reset, timeout);
    /// * a response with a status of 500 and up — what a reverse proxy answers while
    ///   the container behind it is being recreated.
    ///
    /// Only idempotent methods are replayed: a POST or a PATCH goes out once, and
    /// whatever came back is the result. A 4xx is an answer, not an outage, and is
    /// returned at once.
    ///
    /// When the attempts run out, the last one is the result: a 5xx comes back as
    /// `Ok(response)` for the caller to read the status of, a transport failure as
    /// `Err`. A 5xx that gets replayed is never read — its body is aborted before the
    /// pause: the HTTP/1 connection it came on is disposed, an h2 stream is reset while
    /// the shared h2 connection stays pooled.
    ///
    /// ```no_run
    /// # async fn doc() -> Result<(), flurl::FlUrlError> {
    /// use std::time::Duration;
    ///
    /// // Up to 16 attempts, 2s apart: about 30s of outage covered.
    /// let response = flurl::FlUrl::new("https://api.example.com")
    ///     .append_path_segment("data")
    ///     .with_retry(Duration::from_secs(2), 15)
    ///     .get()
    ///     .await?;
    ///
    /// if response.get_status_code() >= 500 {
    ///     // still down after the last attempt
    /// }
    /// # Ok(())
    /// # }
    /// ```
    ///
    /// The pauses come on top of the attempts themselves. An attempt the upstream never
    /// answers costs a request timeout ([`Self::set_timeout`]) — possibly several, since
    /// my-http-client replays a timed-out idempotent request internally before it
    /// reports it.
    ///
    /// Sets the same policy as [`Self::with_retries`], so the later of the two calls
    /// wins. A streamed body is never replayed (see [`Self::execute_streamed`]).
    pub fn with_retry(self, retry_delay: Duration, amount: usize) -> Self {
        self.map(|inner| inner.with_retry(retry_delay, amount))
    }

    pub fn print_input_request(self) -> Self {
        self.map(|inner| inner.print_input_request())
    }

    #[cfg(all(unix, feature = "with-ssh"))]
    pub fn set_ssh_security_credentials_resolver(
        self,
        resolver: Arc<dyn my_ssh::ssh_settings::SshSecurityCredentialsResolver + Send + Sync>,
    ) -> Self {
        self.map(|inner| inner.set_ssh_security_credentials_resolver(resolver))
    }

    /// Does nothing on a url that is not an ssh tunnel, the same as
    /// [`Self::set_ssh_private_key`] and [`Self::set_ssh_user_password`]: whether the
    /// url is a tunnel is decided by settings as often as by code.
    #[cfg(all(unix, feature = "with-ssh"))]
    pub fn set_ssh_password<'s>(self, password: impl Into<StrOrString<'s>>) -> Self {
        self.map(|inner| inner.set_ssh_password(password))
    }

    #[cfg(all(unix, feature = "with-ssh"))]
    pub fn set_ssh_credentials(self, ssh_credentials: my_ssh::SshCredentials) -> Self {
        self.map(|inner| inner.set_ssh_credentials(ssh_credentials))
    }

    #[cfg(all(unix, feature = "with-ssh"))]
    pub fn set_ssh_private_key<'s>(
        self,
        private_key: String,
        passphrase: Option<String>,
    ) -> Self {
        self.map(|inner| inner.set_ssh_private_key(private_key, passphrase))
    }

    #[cfg(all(unix, feature = "with-ssh"))]
    pub fn set_ssh_user_password<'s>(self, password: String) -> Self {
        self.map(|inner| inner.set_ssh_user_password(password))
    }

    /// Bounds the request: sending it, the response head and — on the same clock,
    /// which does not start over — the read of the whole body after it (`into_vec`,
    /// `into_string`, `into_json` of its reader). 10 seconds by default. A body which
    /// is read piece by piece (`next_item`) is not bounded by it.
    pub fn set_timeout(self, timeout: Duration) -> Self {
        self.map(|inner| inner.set_timeout(timeout))
    }

    /// Bounds how long reading the response body may take: a read of the whole body
    /// (`into_vec`, `into_string`, `into_json` of its reader) as a whole, a read piece
    /// by piece (`next_item`) for each piece. Unbounded by default.
    pub fn set_response_body_timeout(self, timeout: Duration) -> Self {
        self.map(|inner| inner.set_response_body_timeout(timeout))
    }

    pub fn do_not_reuse_connection(self) -> Self {
        self.map(|inner| inner.do_not_reuse_connection())
    }

    /// Only available with a TLS provider feature (`with-ring-tls` or
    /// `with-rust-tls`) — without one the crate does not link a TLS stack at all,
    /// so there is no certificate type to pass in.
    ///
    /// The certificate is checked here, and what is wrong with it fails the request
    /// with [`FlUrlError::RequestBuild`] before a socket is opened:
    ///
    /// * the url is not `https://` — a client certificate is presented in a TLS
    ///   handshake, and whether the url is `https://` is decided by settings as often
    ///   as by code;
    /// * its private key can not be loaded — a broken or an unsupported key, read from
    ///   a file that settings name. Found here, it is not retried as an outage the way
    ///   a failed connection is;
    /// * the request has been given a certificate already.
    #[cfg(feature = "_tls")]
    pub fn with_client_certificate(self, certificate: my_tls::ClientCertificate) -> Self {
        self.and_then(|inner| inner.with_client_certificate(certificate))
    }

    /// Without a TLS provider feature this is inert: the request never reaches a
    /// TLS handshake because `https://` is refused at execute time with
    /// [`FlUrlError::UnsupportedScheme`]. It also needs `dangerous-tls` to have any
    /// effect at all — see that feature's docs.
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

    /// A header that must not go on the wire — a value with a CR, LF or NUL in it, a
    /// name that is empty or is not an HTTP token — is not added: the request fails
    /// with [`FlUrlError::RequestBuild`]. The message names the header and the byte,
    /// never the value.
    pub fn with_header<'n, 'v>(
        self,
        name: impl Into<StrOrString<'n>>,
        value: impl Into<StrOrString<'v>>,
    ) -> Self {
        self.and_then(|inner| inner.with_header(name, value))
    }

    pub fn append_raw_ending_to_url<'r>(self, raw: impl Into<StrOrString<'r>>) -> Self {
        self.map(|inner| inner.append_raw_ending_to_url(raw))
    }

    /// Executes an HTTP request described by a `my_http_utils` request model (any
    /// type deriving `my_http_utils::macros::MyHttpInput`). The model fills the URL
    /// path/query, headers, and body; `verb` selects the method. The base host and
    /// any static route prefix are configured on `self` beforehand via the usual
    /// builder methods (`append_path_segment`, `with_header`, …).
    ///
    /// For a parameter-less request, pass [`crate::EmptyRequestModel`] instead of
    /// deriving a dedicated model — the URL/headers already set on `self` are used
    /// as-is and body-carrying verbs send an empty body.
    ///
    /// `Get`/`Delete`/`Head` do not carry a body, so a body produced by the model
    /// is ignored for those verbs.
    ///
    /// A model with a `#[http_body_as_stream]` field goes down the streamed path
    /// instead: the chunks the application writes into the stream are written to the
    /// socket as they arrive. The framing comes from the stream itself — the
    /// `content_length` given to `HttpBodyAsStream::create` becomes `Content-Length`,
    /// and `None` goes out chunked — and everything else that applies to a streamed
    /// body applies here too (see [`Self::execute_streamed`]): no `compress()`, no
    /// retries, the timeout covers the whole upload.
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

    pub async fn post(self, body: impl Into<HttpRequestBody>) -> Result<FlUrlResponse, FlUrlError> {
        self.inner?.post(body).await
    }

    pub async fn post_with_debug(
        self,
        body: impl Into<HttpRequestBody>,
        request_debug_string: &mut String,
    ) -> Result<FlUrlResponse, FlUrlError> {
        self.inner?.post_with_debug(body, request_debug_string).await
    }

    /// POSTs a body that is produced as a stream instead of living in memory as a
    /// whole. See [`Self::execute_streamed`] for what applies to every streamed
    /// request — framing, retries, timeouts.
    ///
    /// ```no_run
    /// # async fn doc() -> Result<(), flurl::FlUrlError> {
    /// use flurl::my_http_client::RequestBodyStream;
    ///
    /// let (publisher, body) = RequestBodyStream::new(4);
    ///
    /// tokio::spawn(async move {
    ///     for chunk in 0..10u8 {
    ///         // Err means the request is over — nothing else can be published
    ///         if publisher.publish(vec![chunk; 64 * 1024]).await.is_err() {
    ///             break;
    ///         }
    ///     }
    ///     // dropping the publisher is what ends the body
    /// });
    ///
    /// let response = flurl::FlUrl::new("https://api.example.com")
    ///     .append_path_segment("upload")
    ///     .set_timeout(std::time::Duration::from_secs(600))
    ///     // None: the size is unknown, so the body goes out chunked
    ///     .post_request_streamed(body, None)
    ///     .await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn post_request_streamed<TBody>(
        self,
        body: TBody,
        content_length: Option<usize>,
    ) -> Result<FlUrlResponse, FlUrlError>
    where
        TBody: hyper::body::Body<Data = Bytes> + Send + Sync + 'static,
        TBody::Error: std::fmt::Display,
    {
        self.inner?.post_request_streamed(body, content_length).await
    }

    /// Same as [`Self::post_request_streamed`], with the request head dumped into
    /// `request_debug_string` — the streamed payload itself is not printed. See
    /// [`Self::execute_streamed_with_debug`].
    pub async fn post_request_streamed_with_debug<TBody>(
        self,
        body: TBody,
        content_length: Option<usize>,
        request_debug_string: &mut String,
    ) -> Result<FlUrlResponse, FlUrlError>
    where
        TBody: hyper::body::Body<Data = Bytes> + Send + Sync + 'static,
        TBody::Error: std::fmt::Display,
    {
        self.inner?
            .post_request_streamed_with_debug(body, content_length, request_debug_string)
            .await
    }

    /// PUTs a body that is produced as a stream. See [`Self::execute_streamed`].
    ///
    /// An upload whose size is known — a file being sent somewhere — is the case for
    /// passing a length rather than letting it go out chunked:
    ///
    /// ```no_run
    /// # async fn doc(len: usize, body: flurl::my_http_client::RequestBodyStream<Vec<u8>>)
    /// # -> Result<(), flurl::FlUrlError> {
    /// let response = flurl::FlUrl::new("https://api.example.com")
    ///     .append_path_segment("files")
    ///     .append_path_segment("archive.tar")
    ///     .set_timeout(std::time::Duration::from_secs(600))
    ///     // `len` and the producer of `body` must come from one source
    ///     .put_request_streamed(body, Some(len))
    ///     .await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn put_request_streamed<TBody>(
        self,
        body: TBody,
        content_length: Option<usize>,
    ) -> Result<FlUrlResponse, FlUrlError>
    where
        TBody: hyper::body::Body<Data = Bytes> + Send + Sync + 'static,
        TBody::Error: std::fmt::Display,
    {
        self.inner?.put_request_streamed(body, content_length).await
    }

    /// Same as [`Self::put_request_streamed`], with the request head dumped into
    /// `request_debug_string`. See [`Self::execute_streamed_with_debug`].
    pub async fn put_request_streamed_with_debug<TBody>(
        self,
        body: TBody,
        content_length: Option<usize>,
        request_debug_string: &mut String,
    ) -> Result<FlUrlResponse, FlUrlError>
    where
        TBody: hyper::body::Body<Data = Bytes> + Send + Sync + 'static,
        TBody::Error: std::fmt::Display,
    {
        self.inner?
            .put_request_streamed_with_debug(body, content_length, request_debug_string)
            .await
    }

    /// PATCHes a body that is produced as a stream. See [`Self::execute_streamed`].
    pub async fn patch_request_streamed<TBody>(
        self,
        body: TBody,
        content_length: Option<usize>,
    ) -> Result<FlUrlResponse, FlUrlError>
    where
        TBody: hyper::body::Body<Data = Bytes> + Send + Sync + 'static,
        TBody::Error: std::fmt::Display,
    {
        self.inner?.patch_request_streamed(body, content_length).await
    }

    /// Same as [`Self::patch_request_streamed`], with the request head dumped into
    /// `request_debug_string`. See [`Self::execute_streamed_with_debug`].
    pub async fn patch_request_streamed_with_debug<TBody>(
        self,
        body: TBody,
        content_length: Option<usize>,
        request_debug_string: &mut String,
    ) -> Result<FlUrlResponse, FlUrlError>
    where
        TBody: hyper::body::Body<Data = Bytes> + Send + Sync + 'static,
        TBody::Error: std::fmt::Display,
    {
        self.inner?
            .patch_request_streamed_with_debug(body, content_length, request_debug_string)
            .await
    }

    /// Sends `body` as a stream under `method` — a [`my_http_client::RequestBodyStream`]
    /// fed by a publisher, a proxied `hyper::body::Incoming`, a `StreamBody` over a
    /// file reader, anything implementing `hyper::body::Body<Data = Bytes>`. Peak
    /// memory is one chunk plus whatever the producer buffers, not the size of the
    /// payload.
    ///
    /// This path is hyper HTTP/1.1 only: the mode is pinned to
    /// [`FlUrlMode::Http1Hyper`] regardless of [`Self::update_mode`], because the own
    /// HTTP/1.1 implementation serializes a request into one buffer and the h2 client
    /// has no streaming entry point.
    ///
    /// **Framing** is what `content_length` picks. HTTP/1.1 delimits a request body
    /// exactly two ways (RFC 9112 §6), and these are they:
    ///
    /// * `None` → `Transfer-Encoding: chunked`. The length never has to be known, and
    ///   every HTTP/1.1 recipient is required to understand chunked, so this is the
    ///   default a streamed body wants.
    /// * `Some(n)` → `Content-Length: n`, no chunked. For endpoints that refuse a
    ///   chunked request body, and for anything that has to know the size before it
    ///   starts reading.
    ///
    /// With `Some(n)` the body must then deliver **exactly** `n` bytes. That is the
    /// protocol's rule, not fl-url's: a short body makes the message incomplete, and
    /// extra bytes would be read as the start of the next request on the same
    /// connection. A stream that ends early therefore fails the request
    /// (`"user body write aborted"`) instead of putting a truncated payload on the
    /// wire — so `n` and the producer must come from one source (a file's metadata and
    /// that same file), never be computed twice.
    ///
    /// `content_length` is the single source of the framing, so it overrides a
    /// `Content-Length` added with [`Self::with_header`] in both directions: `Some(n)`
    /// replaces such a header (never emits a second one, which would be a protocol
    /// violation), and `None` removes it, because a body of unknown size must not
    /// claim a length it may not deliver.
    ///
    /// Three builder knobs do not apply, and none of them fails quietly:
    ///
    /// * [`Self::compress`] and [`Self::compress_with_level`] gzip the body as one
    ///   buffer, which is exactly what streaming avoids →
    ///   [`FlUrlError::StreamedBodyCanNotBeCompressed`].
    /// * [`Self::with_retries`] and [`Self::with_retry`] are ignored: the payload is
    ///   consumed as it is sent, so the request is attempted exactly once. Rebuilding
    ///   the stream and calling again is the caller's decision — it owns the source
    ///   data.
    /// * [`Self::set_timeout`] covers the **whole** call, upload included, not just
    ///   the wait for the response head. The 10s default is far too short for a real
    ///   upload; set it to the size of the transfer you expect.
    pub async fn execute_streamed<TBody>(
        self,
        method: Method,
        body: TBody,
        content_length: Option<usize>,
    ) -> Result<FlUrlResponse, FlUrlError>
    where
        TBody: hyper::body::Body<Data = Bytes> + Send + Sync + 'static,
        TBody::Error: std::fmt::Display,
    {
        self.inner?.execute_streamed(method, body, content_length).await
    }

    /// Same as [`Self::execute_streamed`], with the request head — verb, path and
    /// query, headers — dumped into `request_debug_string` before it goes on the wire.
    ///
    /// The payload is **not** in the dump: a streamed body exists only as it is
    /// written to the socket, so printing it would mean buffering the very thing
    /// streaming avoids.
    pub async fn execute_streamed_with_debug<TBody>(
        self,
        method: Method,
        body: TBody,
        content_length: Option<usize>,
        request_debug_string: &mut String,
    ) -> Result<FlUrlResponse, FlUrlError>
    where
        TBody: hyper::body::Body<Data = Bytes> + Send + Sync + 'static,
        TBody::Error: std::fmt::Display,
    {
        self.inner?
            .execute_streamed_with_debug(method, body, content_length, request_debug_string)
            .await
    }

    #[deprecated(note = "Use `post` instead")]
    pub async fn post_json(
        self,
        json: &impl serde::Serialize,
    ) -> Result<FlUrlResponse, FlUrlError> {
        self.inner?.post_json(json).await
    }

    pub async fn patch(self, body: impl Into<HttpRequestBody>) -> Result<FlUrlResponse, FlUrlError> {
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

    pub async fn put(self, body: impl Into<HttpRequestBody>) -> Result<FlUrlResponse, FlUrlError> {
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

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use crate::body::HttpRequestBody;
    use crate::{EmptyRequestModel, FlUrl, FlUrlError, HttpVerb};

    const NO_HOST: &str = "Invalid url 'http://': it names no host";

    fn assert_no_host(error: Option<&FlUrlError>) {
        match error {
            Some(FlUrlError::InvalidUrl(message)) => assert_eq!(message, NO_HOST),
            other => panic!("unexpected state {other:?}"),
        }
    }

    #[test]
    fn a_url_that_can_be_used_leaves_the_builder_alive() {
        let fl_url = FlUrl::new("http://localhost:8080/api").append_path_segment("users");

        assert!(fl_url.get_error().is_none());

        let url_builder = fl_url.get_url_builder().unwrap();
        assert_eq!(url_builder.get_host_port(), "localhost:8080");
        assert_eq!(url_builder.get_path_and_query(), "/api/users");
    }

    #[test]
    fn a_url_that_can_not_be_used_becomes_the_error() {
        let fl_url = FlUrl::new("http://");

        assert_no_host(fl_url.get_error());
        assert_no_host(fl_url.get_url_builder().err());
        assert_eq!(fl_url.get_resolved_ip(), None);
        assert_eq!(fl_url.to_string(), format!("Error: InvalidUrl({NO_HOST:?})"));
    }

    #[test]
    fn every_step_after_the_error_is_skipped_and_the_first_error_stays() {
        let fl_url = FlUrl::new("http://")
            .append_path_segment("api")
            .append_query_param("a", Some("b"))
            .append_raw_ending_to_url("/raw")
            // An error of its own, had the builder still been alive.
            .with_header("X-Name", "a\nb")
            .compress()
            .accept_gzip()
            .set_timeout(Duration::from_secs(1))
            .with_retry(Duration::from_secs(1), 3)
            .do_not_reuse_connection();

        assert_no_host(fl_url.get_error());
    }

    #[test]
    fn a_header_that_can_not_be_sent_becomes_the_error() {
        let fl_url = FlUrl::new("http://localhost")
            .with_header("X-First", "fine")
            .with_header("X-Name", "a\nb")
            .with_header("X-Other", "c\rd");

        match fl_url.get_error() {
            Some(FlUrlError::RequestBuild(message)) => assert_eq!(
                message,
                "Value of header 'X-Name' contains forbidden control byte 0x0a"
            ),
            other => panic!("unexpected state {other:?}"),
        }
    }

    #[tokio::test]
    async fn every_way_of_sending_returns_the_error() {
        let fl_url = || FlUrl::new("http://");
        let body = || HttpRequestBody::from_raw_data(b"body".to_vec(), None);

        let mut debug = String::new();

        let results = [
            fl_url().get().await,
            fl_url().get_with_debug(&mut debug).await,
            fl_url().head().await,
            fl_url().head_with_debug(&mut debug).await,
            fl_url().delete().await,
            fl_url().delete_with_debug(&mut debug).await,
            fl_url().post(body()).await,
            fl_url().post_with_debug(body(), &mut debug).await,
            fl_url().put(body()).await,
            fl_url().put_with_debug(body(), &mut debug).await,
            fl_url().patch(body()).await,
            fl_url().patch_with_debug(body(), &mut debug).await,
            fl_url()
                .execute_request(HttpVerb::Get, EmptyRequestModel)
                .await,
            fl_url()
                .execute_request_with_debug(HttpVerb::Post, EmptyRequestModel, &mut debug)
                .await,
            fl_url()
                .post_request_streamed(http_body_util::Full::new(bytes::Bytes::new()), None)
                .await,
            fl_url()
                .execute_streamed_with_debug(
                    hyper::Method::PUT,
                    http_body_util::Full::new(bytes::Bytes::new()),
                    Some(0),
                    &mut debug,
                )
                .await,
        ];

        for (index, result) in results.into_iter().enumerate() {
            match result {
                Err(FlUrlError::InvalidUrl(message)) => assert_eq!(message, NO_HOST, "#{index}"),
                Err(err) => panic!("#{index}: unexpected error {err:?}"),
                Ok(_) => panic!("#{index}: nothing can be sent to a url with no host"),
            }
        }

        // Nothing was compiled, so nothing was dumped either.
        assert_eq!(debug, "");
    }

    #[test]
    fn a_plain_url_has_no_resolved_ip() {
        let fl_url = FlUrl::new("https://domain.com/xxx/fff");
        assert_eq!(fl_url.get_resolved_ip(), None);
    }

    #[test]
    fn a_server_name_at_ip_url_has_one() {
        let fl_url = FlUrl::new("https://domain.com@15.0.0.5/xxx/fff");
        assert_eq!(fl_url.get_resolved_ip(), Some("15.0.0.5".parse().unwrap()));
    }
}
