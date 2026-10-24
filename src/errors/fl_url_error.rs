/// The single, shared FlUrl error type. Variants that carry native-only payloads
/// (hyper / rustls / my-http-client / ssh) are compiled only for non-wasm
/// targets — and the rustls one only with a TLS provider feature; the wasm
/// backend adds [`FlUrlError::FetchError`]. It is
/// `#[non_exhaustive]`, so consumers must already carry a catch-all arm — which
/// makes the per-target variant set a non-breaking implementation detail.
#[derive(Debug)]
#[non_exhaustive]
pub enum FlUrlError {
    Timeout,
    SerializationError(serde_json::Error),
    IoError(std::io::Error),
    CanNotConvertToUtf8(std::str::Utf8Error),
    ReadingHyperBodyError(String),
    /// The response body is bigger than the `limit`, in bytes, it was read into memory
    /// with: `into_vec`, `into_string` and `into_json` of the reader of the body each
    /// take one. A gzip body which is given decoded is measured decoded.
    ResponseBodyTooLarge { limit: usize },
    /// The url can not be used: it names no host, its scheme is unknown, its
    /// `name@ip` or ssh tunnel part is malformed, or it carries a byte that must not
    /// be put on the wire (a new line in the host, a space in the path). `FlUrl::new`
    /// can not report it, so it is the request that fails with it.
    InvalidUrl(String),
    /// The url is valid, but this build or this target can not serve its scheme:
    /// `https://` without a TLS provider feature, an ssh tunnel without `with-ssh`,
    /// `ws://`, a unix socket where there are none.
    UnsupportedScheme(String),
    /// The request could not be built, so nothing was sent: a `my_http_utils`
    /// request model failed to build (e.g. a field validator rejected its value)
    /// inside `FlUrl::execute_request`, a header can not be put on the wire — a
    /// value with a CR, LF or NUL in it, a name that is not an HTTP token — or a
    /// client certificate was given for a url that is not `https://`.
    RequestBuild(String),

    /// `.compress()` or `.compress_with_level()` was combined with a streamed request
    /// body. Gzipping needs the whole body in one buffer, which is exactly what
    /// streaming exists to avoid, so the request is refused instead of quietly
    /// materializing the payload.
    #[cfg(not(target_arch = "wasm32"))]
    StreamedBodyCanNotBeCompressed,

    #[cfg(not(target_arch = "wasm32"))]
    HttpError(hyper::http::Error),
    #[cfg(all(not(target_arch = "wasm32"), feature = "_tls"))]
    RustTlsError(my_tls::tokio_rustls::rustls::Error),
    #[cfg(not(target_arch = "wasm32"))]
    MyHttpClientError(my_http_client::MyHttpClientError),
    #[cfg(all(unix, feature = "with-ssh", not(target_arch = "wasm32")))]
    SshSessionError(my_ssh::SshSessionError),

    /// The browser `fetch` call (or reading the response body) failed. Carries a
    /// human-readable description of the underlying JS error / `DOMException`.
    #[cfg(target_arch = "wasm32")]
    FetchError(String),
}

impl FlUrlError {
    pub fn is_timeout(&self) -> bool {
        if matches!(self, FlUrlError::Timeout) {
            return true;
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            if matches!(
                self,
                FlUrlError::MyHttpClientError(my_http_client::MyHttpClientError::RequestTimeout(_))
            ) {
                return true;
            }
        }
        false
    }
}

impl std::fmt::Display for FlUrlError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}", self)
    }
}

impl std::error::Error for FlUrlError {}

impl From<std::str::Utf8Error> for FlUrlError {
    fn from(src: std::str::Utf8Error) -> Self {
        Self::CanNotConvertToUtf8(src)
    }
}

impl From<serde_json::Error> for FlUrlError {
    fn from(src: serde_json::Error) -> Self {
        Self::SerializationError(src)
    }
}

impl From<std::io::Error> for FlUrlError {
    fn from(src: std::io::Error) -> Self {
        Self::IoError(src)
    }
}

impl From<my_http_utils::schema::client::HttpRequestBuildError> for FlUrlError {
    fn from(src: my_http_utils::schema::client::HttpRequestBuildError) -> Self {
        Self::RequestBuild(format!("{}: {}", src.field, src.reason))
    }
}

#[cfg(all(not(target_arch = "wasm32"), feature = "_tls"))]
impl From<my_tls::tokio_rustls::rustls::Error> for FlUrlError {
    fn from(value: my_tls::tokio_rustls::rustls::Error) -> Self {
        Self::RustTlsError(value)
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl From<my_http_client::MyHttpClientError> for FlUrlError {
    fn from(value: my_http_client::MyHttpClientError) -> Self {
        match value {
            my_http_client::MyHttpClientError::ResponseBodyTooLarge { limit } => {
                Self::ResponseBodyTooLarge { limit }
            }
            other => Self::MyHttpClientError(other),
        }
    }
}

/// What my-http-client refuses to put on the wire. The message carries the offending
/// byte and none of the input around it.
#[cfg(not(target_arch = "wasm32"))]
impl From<my_http_client::RequestBuildError> for FlUrlError {
    fn from(src: my_http_client::RequestBuildError) -> Self {
        Self::RequestBuild(src.to_string())
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl From<hyper::http::Error> for FlUrlError {
    fn from(src: hyper::http::Error) -> Self {
        Self::HttpError(src)
    }
}

#[cfg(all(unix, feature = "with-ssh", not(target_arch = "wasm32")))]
impl FlUrlError {
    pub fn is_ssh_session_error(&self) -> bool {
        matches!(self, FlUrlError::SshSessionError(_))
    }
}

#[cfg(all(unix, feature = "with-ssh", not(target_arch = "wasm32")))]
impl From<my_ssh::SshSessionError> for FlUrlError {
    fn from(src: my_ssh::SshSessionError) -> Self {
        Self::SshSessionError(src)
    }
}
