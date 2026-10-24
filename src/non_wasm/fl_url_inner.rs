use bytes::Bytes;
use http_body_util::Full;
use hyper::Method;

use hyper::Uri;
use hyper::Version;
use my_http_client::http1::MyHttpRequestBuilder;
use my_http_client::MyHttpClientConnector;
use my_http_client::RequestBuildError;
#[cfg(feature = "_tls")]
use my_tls::tokio_rustls::client::TlsStream;

use rust_extensions::remote_endpoint::Scheme;
use rust_extensions::StrOrString;

use std::net::IpAddr;
use std::sync::Arc;
use std::time::Duration;
use tokio::net::TcpStream;

use super::fl_url::{FlUrlMode, HttpVerb};
use super::FlUrlResponse;
use crate::body::HttpRequestBody;
use crate::non_wasm::compiled_http_request::{CompiledHttpRequest, RequestToExecute};
use crate::non_wasm::http_connectors::*;
use crate::non_wasm::model_body_stream::ModelBodyStream;
use crate::non_wasm::resolved_ip::extract_resolved_ip;

use crate::non_wasm::http_clients_cache::*;

use crate::CompressionLevel;
use crate::HttpConnectionResolver;

use crate::FlUrlError;

use crate::RetryPolicy;

use crate::FlUrlHeaders;

use my_http_utils::UrlBuilder;

/// Everything a request is made of, and everything that is done with it. [`FlUrl`]
/// keeps one of these — or the first error that came up while it was being built —
/// and hands every call over to it.
///
/// [`FlUrl`]: super::FlUrl
pub(crate) struct FlUrlInner {
    pub url_builder: UrlBuilder,
    // Set by a `scheme://server-name@ip/...` url: the socket is opened to this ip
    // instead of resolving the url's host, which stays the Host header and TLS SNI.
    resolved_ip: Option<IpAddr>,
    pub headers: FlUrlHeaders,
    #[cfg(feature = "_tls")]
    pub client_cert: Option<my_tls::ClientCertificate>,
    pub accept_invalid_certificate: bool,
    pub request_timeout: Duration,
    // Bounds how long reading the response body may take. `None` = unbounded.
    pub response_body_timeout: Option<Duration>,
    pub do_not_reuse_connection: bool,
    pub connections_cache: Option<Arc<FlUrlHttpConnectionsCache>>,
    // `Some` once `compress`/`compress_with_level` asked for the body to be gzipped.
    pub compress_body: Option<CompressionLevel>,
    pub decompress_gzip_response: bool,
    pub print_input_request: bool,
    // If we reuse connection and it has not been used more seconds than this parameter - it disposed
    pub reuse_connection_timeout_sec: i64,
    mode: FlUrlMode,
    #[cfg(all(unix, feature = "with-ssh"))]
    ssh_credentials: Option<my_ssh::SshCredentials>,
    #[cfg(all(unix, feature = "with-ssh"))]
    ssh_security_credentials_resolver:
        Option<Arc<dyn my_ssh::ssh_settings::SshSecurityCredentialsResolver + Send + Sync>>,

    retry: RetryPolicy,
}

impl FlUrlInner {
    pub fn new<'s>(url: impl Into<StrOrString<'s>>) -> Result<Self, FlUrlError> {
        let url: StrOrString<'s> = url.into();

        #[cfg(all(unix, feature = "with-ssh"))]
        let (url_builder, credentials, resolved_ip) = {
            let endpoint =
                rust_extensions::remote_endpoint::RemoteEndpointHostString::try_parse(url.as_str())
                    .map_err(|err| FlUrlError::InvalidUrl(err))?;

            match endpoint {
                rust_extensions::remote_endpoint::RemoteEndpointHostString::Direct(
                    _remote_endpoint,
                ) => {
                    let (url, resolved_ip) = extract_resolved_ip(url.as_str())?;
                    (UrlBuilder::new(&url), None, resolved_ip)
                }
                rust_extensions::remote_endpoint::RemoteEndpointHostString::ViaSsh {
                    ssh_remote_host,
                    remote_host_behind_ssh,
                } => {
                    // `ssh://user@:22->...` parses, but there is nowhere to open the
                    // tunnel to.
                    if ssh_remote_host.get_host().trim().is_empty() {
                        return Err(FlUrlError::InvalidUrl(format!(
                            "Invalid url '{}': it names no ssh host",
                            url.as_str()
                        )));
                    }

                    let (url, resolved_ip) = extract_resolved_ip(remote_host_behind_ssh.as_str())?;
                    (
                        UrlBuilder::new(&url),
                        Some(crate::non_wasm::ssh::to_ssh_credentials(&ssh_remote_host)),
                        resolved_ip,
                    )
                }
            }
        };

        #[cfg(not(all(unix, feature = "with-ssh")))]
        let (url_builder, resolved_ip) = {
            let endpoint =
                rust_extensions::remote_endpoint::RemoteEndpointHostString::try_parse(url.as_str())
                    .map_err(|err| FlUrlError::InvalidUrl(err))?;

            match endpoint {
                rust_extensions::remote_endpoint::RemoteEndpointHostString::Direct(
                    _remote_endpoint,
                ) => {
                    let (url, resolved_ip) = extract_resolved_ip(url.as_str())?;
                    (UrlBuilder::new(&url), resolved_ip)
                }
                rust_extensions::remote_endpoint::RemoteEndpointHostString::ViaSsh {
                    ssh_remote_host: _,
                    remote_host_behind_ssh: _,
                } => {
                    return Err(FlUrlError::UnsupportedScheme(
                        "To use ssh you need to enable the 'with-ssh' feature".to_string(),
                    ))
                }
            }
        };

        crate::host_check::ensure_host_is_usable(url.as_str(), &url_builder)?;

        let result = Self {
            headers: FlUrlHeaders::new(),
            #[cfg(feature = "_tls")]
            client_cert: Default::default(),
            url_builder,
            resolved_ip,
            accept_invalid_certificate: false,
            do_not_reuse_connection: false,
            connections_cache: Default::default(),
            retry: RetryPolicy::default(),
            request_timeout: Duration::from_secs(10),
            response_body_timeout: None,
            print_input_request: false,
            compress_body: None,
            decompress_gzip_response: false,
            #[cfg(all(unix, feature = "with-ssh"))]
            ssh_credentials: credentials,
            #[cfg(all(unix, feature = "with-ssh"))]
            ssh_security_credentials_resolver: None,
            mode: Default::default(),
            reuse_connection_timeout_sec: 120,
        };

        Ok(result)
    }

    #[cfg(all(unix, feature = "with-ssh"))]
    pub fn via_ssh(&self) -> bool {
        self.ssh_credentials.is_some()
    }

    pub fn get_resolved_ip(&self) -> Option<IpAddr> {
        self.resolved_ip
    }

    pub fn compress(self) -> Self {
        self.compress_with_level(CompressionLevel::Default)
    }

    pub fn compress_with_level(mut self, level: CompressionLevel) -> Self {
        self.compress_body = Some(level);
        self
    }

    pub fn accept_gzip(mut self) -> Self {
        if !self.headers.has_header("Accept-Encoding") {
            self.headers.add("Accept-Encoding", "gzip");
        }
        self.decompress_gzip_response = true;
        self
    }

    pub fn set_not_used_connection_timeout(mut self, timeout: Duration) -> Self {
        // Round up and clamp to at least 1s: as_secs() truncation would turn a
        // sub-second timeout into 0, which evicts the whole per-key pool on
        // every checkout (pooling silently disabled).
        self.reuse_connection_timeout_sec = (timeout.as_secs_f64().ceil() as i64).max(1);
        self
    }

    pub fn update_mode(mut self, mode: FlUrlMode) -> Self {
        self.mode = mode;
        self
    }

    pub fn set_connections_cache(mut self, clients_cache: Arc<FlUrlHttpConnectionsCache>) -> Self {
        self.connections_cache = Some(clients_cache);
        self
    }

    pub fn with_retries(mut self, max_retries: usize) -> Self {
        self.retry = RetryPolicy::transport_failures(max_retries);
        self
    }

    pub fn with_retry(mut self, retry_delay: Duration, amount: usize) -> Self {
        self.retry = RetryPolicy::transport_failures_and_server_errors(retry_delay, amount);
        self
    }

    pub fn print_input_request(mut self) -> Self {
        self.print_input_request = true;
        self
    }

    #[cfg(all(unix, feature = "with-ssh"))]
    pub fn set_ssh_security_credentials_resolver(
        mut self,
        resolver: Arc<dyn my_ssh::ssh_settings::SshSecurityCredentialsResolver + Send + Sync>,
    ) -> Self {
        self.ssh_security_credentials_resolver = Some(resolver);
        self
    }

    #[cfg(all(unix, feature = "with-ssh"))]
    pub fn set_ssh_password<'s>(mut self, password: impl Into<StrOrString<'s>>) -> Self {
        let Some(ssh_credentials) = self.ssh_credentials.take() else {
            return self;
        };

        let (host, port) = ssh_credentials.get_host_port();

        let password = password.into();

        self.ssh_credentials = Some(my_ssh::SshCredentials::UserNameAndPassword {
            ssh_remote_host: host.to_string(),
            ssh_remote_port: port,
            ssh_user_name: ssh_credentials.get_user_name().to_string(),
            password: password.to_string(),
        });
        self
    }

    #[cfg(all(unix, feature = "with-ssh"))]
    pub fn set_ssh_credentials(mut self, ssh_credentials: my_ssh::SshCredentials) -> Self {
        self.ssh_credentials = Some(ssh_credentials);
        self
    }

    #[cfg(all(unix, feature = "with-ssh"))]
    pub fn set_ssh_private_key<'s>(
        mut self,
        private_key: String,
        passphrase: Option<String>,
    ) -> Self {
        let Some(ssh_credentials) = self.ssh_credentials.take() else {
            return self;
        };

        let (host, port) = ssh_credentials.get_host_port();

        self.ssh_credentials = Some(my_ssh::SshCredentials::PrivateKey {
            ssh_remote_host: host.to_string(),
            ssh_remote_port: port,
            ssh_user_name: ssh_credentials.get_user_name().to_string(),
            private_key,
            passphrase,
        });
        self
    }

    #[cfg(all(unix, feature = "with-ssh"))]
    pub fn set_ssh_user_password<'s>(mut self, password: String) -> Self {
        let Some(ssh_credentials) = self.ssh_credentials.take() else {
            return self;
        };

        let (host, port) = ssh_credentials.get_host_port();

        self.ssh_credentials = Some(my_ssh::SshCredentials::UserNameAndPassword {
            ssh_remote_host: host.to_string(),
            ssh_remote_port: port,
            ssh_user_name: ssh_credentials.get_user_name().to_string(),
            password,
        });
        self
    }

    pub fn set_timeout(mut self, timeout: Duration) -> Self {
        self.request_timeout = timeout;
        self
    }

    pub fn set_response_body_timeout(mut self, timeout: Duration) -> Self {
        self.response_body_timeout = Some(timeout);
        self
    }

    pub fn do_not_reuse_connection(mut self) -> Self {
        self.do_not_reuse_connection = true;
        self
    }

    #[cfg(feature = "_tls")]
    pub fn with_client_certificate(
        mut self,
        certificate: my_tls::ClientCertificate,
    ) -> Result<Self, FlUrlError> {
        // Two certificates for one request is a mistake in the code rather than in the
        // settings, but it fails the request the same way as everything else here.
        if self.client_cert.is_some() {
            return Err(FlUrlError::RequestBuild(
                "Client certificate is already set".to_string(),
            ));
        }

        // Sending the request without the certificate would look like a success right
        // up to the answer of a server that never saw one.
        if !self.url_builder.get_scheme().is_https() {
            return Err(FlUrlError::RequestBuild(format!(
                "Client certificate can only be used with https. Url: {}",
                self.url_builder
            )));
        }

        // A key that can not be loaded is a broken setting, so it is found here, by
        // my-tls' own loader, rather than in the middle of a connection attempt, where
        // it would be retried as an outage.
        if let Err(err) =
            my_tls::ssl::calc_cert_key(&certificate.private_key, certificate.cert_chain.clone())
        {
            return Err(FlUrlError::RequestBuild(format!(
                "Client certificate can not be used: {}",
                err
            )));
        }

        self.client_cert = Some(certificate);
        Ok(self)
    }

    pub fn accept_invalid_certificate(mut self) -> Self {
        self.accept_invalid_certificate = true;
        self
    }

    pub fn append_path_segment<'s>(mut self, path_segment: impl Into<StrOrString<'s>>) -> Self {
        self.url_builder
            .append_path_segment(path_segment.into().as_str());
        self
    }

    pub fn append_query_param<'n, 'v>(
        mut self,
        param_name: impl Into<StrOrString<'n>>,
        value: Option<impl Into<StrOrString<'v>>>,
    ) -> Self {
        let param_name = param_name.into();

        if let Some(value) = value {
            let value = value.into();
            self.url_builder
                .append_query_param(param_name.as_str(), Some(value.as_str()));
        } else {
            self.url_builder
                .append_query_param(param_name.as_str(), None);
        };

        self
    }

    pub fn with_header<'n, 'v>(
        mut self,
        name: impl Into<StrOrString<'n>>,
        value: impl Into<StrOrString<'v>>,
    ) -> Result<Self, FlUrlError> {
        let name: StrOrString<'_> = name.into();
        let value: StrOrString<'_> = value.into();

        self.headers.add(name.as_str(), value.as_str());
        // `add` keeps a header that must not go on the wire instead of reporting it —
        // a request model pushes its header fields through it too, and has no error
        // to return there. Here the error is known at once.
        self.headers.ensure_valid()?;
        Ok(self)
    }

    pub fn append_raw_ending_to_url<'r>(mut self, raw: impl Into<StrOrString<'r>>) -> Self {
        let raw: StrOrString<'r> = raw.into();
        self.url_builder.append_raw_ending(raw.as_str());
        self
    }

    /// Pours a `my_http_utils` request model into this `FlUrl`: the model appends its
    /// path segments + query params to our `url_builder`, pushes its header fields
    /// into our `headers`, and hands over its body (which it consumes). The base
    /// host and any static route prefix must already be configured on `self`.
    fn fill_from_model(
        &mut self,
        model: impl my_http_utils::schema::client::THttpRequestBuilder,
    ) -> Result<HttpRequestBody, FlUrlError> {
        model.fill_url(&mut self.url_builder)?;
        model.fill_headers(&mut self.headers)?;
        // `get_body` consumes the model, so it must be the last thing we read.
        // The body is our own `HttpRequestBody` already — no conversion needed;
        // `compile_*_request` reads its (possibly dynamic, e.g. FormData boundary)
        // content type via `get_content_type()`. `FlUrlRnd` supplies the random
        // multipart boundary suffix (my-http-utils carries no RNG of its own).
        let body = model.get_body::<crate::body::FlUrlRnd>()?;
        Ok(body)
    }

    pub async fn execute_request(
        mut self,
        verb: HttpVerb,
        model: impl my_http_utils::schema::client::THttpRequestBuilder,
    ) -> Result<FlUrlResponse, FlUrlError> {
        let body = self.fill_from_model(model)?;

        if let HttpRequestBody::Stream(stream) = body {
            return self.execute_model_stream(verb, stream, None).await;
        }

        match verb {
            HttpVerb::Get => self.get().await,
            HttpVerb::Delete => self.delete().await,
            HttpVerb::Head => self.head().await,
            HttpVerb::Post => self.post(body).await,
            HttpVerb::Put => self.put(body).await,
            HttpVerb::Patch => self.patch(body).await,
        }
    }

    pub async fn execute_request_with_debug(
        mut self,
        verb: HttpVerb,
        model: impl my_http_utils::schema::client::THttpRequestBuilder,
        request_debug_string: &mut String,
    ) -> Result<FlUrlResponse, FlUrlError> {
        let body = self.fill_from_model(model)?;

        if let HttpRequestBody::Stream(stream) = body {
            return self
                .execute_model_stream(verb, stream, Some(request_debug_string))
                .await;
        }

        match verb {
            HttpVerb::Get => self.get_with_debug(request_debug_string).await,
            HttpVerb::Delete => self.delete_with_debug(request_debug_string).await,
            HttpVerb::Head => self.head_with_debug(request_debug_string).await,
            HttpVerb::Post => self.post_with_debug(body, request_debug_string).await,
            HttpVerb::Put => self.put_with_debug(body, request_debug_string).await,
            HttpVerb::Patch => self.patch_with_debug(body, request_debug_string).await,
        }
    }

    /// Sends a model whose body is a `#[http_body_as_stream]` field: the chunks the
    /// application writes into the stream are pulled out of it and written to the
    /// socket as they arrive, so the payload is never materialized.
    ///
    /// The framing comes from the stream itself — the `content_length` given to
    /// `HttpBodyAsStream::create` becomes `Content-Length`, and `None` goes out
    /// chunked. Everything else that applies to a streamed body applies here too
    /// (see [`Self::execute_streamed`]): no `compress()`, no retries, the timeout
    /// covers the whole upload.
    async fn execute_model_stream(
        self,
        verb: HttpVerb,
        stream: my_http_utils::http_input::HttpBodyAsStream,
        debug: Option<&mut String>,
    ) -> Result<FlUrlResponse, FlUrlError> {
        // A streamed payload on a body-less verb is a mistake in the call, not
        // something to drop quietly the way a materialized body is: the application
        // is already writing into the stream, and nothing would ever read it.
        let method = match verb {
            HttpVerb::Post => Method::POST,
            HttpVerb::Put => Method::PUT,
            HttpVerb::Patch => Method::PATCH,
            HttpVerb::Get | HttpVerb::Delete | HttpVerb::Head => {
                return Err(FlUrlError::RequestBuild(format!(
                    "{:?} carries no body, but the model streams one (#[http_body_as_stream])",
                    verb
                )))
            }
        };

        // `empty()` (a model built to be parsed by a server, never sent) and a stream
        // whose reader was already taken both land here, and the message says which.
        let reader = stream.get_body_reader().map_err(|err| {
            FlUrlError::RequestBuild(format!(
                "#[http_body_as_stream] model has no body to send: {}",
                err
            ))
        })?;

        let content_length = reader.get_content_length().map(|len| len as usize);

        self.execute_streamed_impl(
            method,
            ModelBodyStream::new(reader),
            content_length,
            debug,
        )
        .await
    }

    // `mut` is for the ssh credentials, which only a `with-ssh` build has.
    #[cfg_attr(not(all(unix, feature = "with-ssh")), allow(unused_mut))]
    async fn execute(mut self, request: RequestToExecute) -> Result<FlUrlResponse, FlUrlError> {
        // The scheme decides what is spoken, with or without an ssh tunnel. The tunnel
        // only decides how the target is reached.
        let response = match self.url_builder.get_scheme() {
            Scheme::Ws => {
                return Err(FlUrlError::UnsupportedScheme(
                    "WebSocket 'ws' scheme is not supported".to_string(),
                ))
            }

            Scheme::Wss => {
                return Err(FlUrlError::UnsupportedScheme(
                    "WebSocket 'wss' scheme is not supported".to_string(),
                ))
            }
            Scheme::Http => {
                #[cfg(all(unix, feature = "with-ssh"))]
                if let Some(ssh_credentials) = self.ssh_credentials.take() {
                    return self
                        .execute_ssh::<my_ssh::SshAsyncChannel, SshHttpConnector>(
                            request,
                            ssh_credentials,
                            Arc::new(crate::non_wasm::http_clients_cache::creators::SshConnectionCreator),
                            crate::consts::HTTP_DEFAULT_PORT.into(),
                        )
                        .await;
                }

                if self.do_not_reuse_connection {
                    self.execute_with_retry::<TcpStream, HttpConnector>(
                        request,
                        Arc::new(crate::non_wasm::http_clients_cache::creators::HttpConnectionCreator),
                        crate::consts::HTTP_DEFAULT_PORT.into(),
                        #[cfg(all(unix, feature = "with-ssh"))]
                        None,
                    )
                    .await?
                } else {
                    let clients_cache = self.get_connections_cache();
                    self.execute_with_retry::<TcpStream, HttpConnector>(
                        request,
                        clients_cache,
                        crate::consts::HTTP_DEFAULT_PORT.into(),
                        #[cfg(all(unix, feature = "with-ssh"))]
                        None,
                    )
                    .await?
                }
            }
            #[cfg(not(feature = "_tls"))]
            Scheme::Https => {
                return Err(FlUrlError::UnsupportedScheme(format!(
                    "FlUrl does not support https: it is compiled without a TLS provider feature. Enable 'with-ring-tls' (ring) or 'with-rust-tls' (pure Rust). Url: {}",
                    self.url_builder
                )))
            }
            #[cfg(feature = "_tls")]
            Scheme::Https => {
                // Behind a tunnel the TLS session runs inside its channel, end to end
                // with the target.
                #[cfg(all(unix, feature = "with-ssh"))]
                if let Some(ssh_credentials) = self.ssh_credentials.take() {
                    return self
                        .execute_ssh::<TlsStream<my_ssh::SshAsyncChannel>, SshHttpsConnector>(
                            request,
                            ssh_credentials,
                            Arc::new(crate::non_wasm::http_clients_cache::creators::SshHttpsConnectionCreator),
                            crate::consts::HTTPS_DEFAULT_PORT.into(),
                        )
                        .await;
                }

                if self.do_not_reuse_connection {
                    self.execute_with_retry::<TlsStream<TcpStream>, HttpsConnector>(
                        request,
                        Arc::new(crate::non_wasm::http_clients_cache::creators::HttpsConnectionCreator),
                        crate::consts::HTTPS_DEFAULT_PORT.into(),
                        #[cfg(all(unix, feature = "with-ssh"))]
                        None,
                    )
                    .await?
                } else {
                    let clients_cache = self.get_connections_cache();

                    self.execute_with_retry::<TlsStream<TcpStream>, HttpsConnector>(
                        request,
                        clients_cache,
                        crate::consts::HTTPS_DEFAULT_PORT.into(),
                        #[cfg(all(unix, feature = "with-ssh"))]
                        None,
                    )
                    .await?
                }
            }
            #[cfg(not(unix))]
            Scheme::UnixSocket => {
                return Err(FlUrlError::UnsupportedScheme(
                    "This OS does not support unix sockets".to_string(),
                ))
            }
            #[cfg(unix)]
            Scheme::UnixSocket => {
                // Behind a tunnel it is a socket on the ssh server.
                #[cfg(all(unix, feature = "with-ssh"))]
                if let Some(ssh_credentials) = self.ssh_credentials.take() {
                    return self
                        .execute_ssh::<my_ssh::SshAsyncChannel, SshHttpConnector>(
                            request,
                            ssh_credentials,
                            Arc::new(crate::non_wasm::http_clients_cache::creators::SshConnectionCreator),
                            None,
                        )
                        .await;
                }

                if self.do_not_reuse_connection {
                    self.execute_with_retry::<UnixSocketStream, UnixSocketConnector>(
                        request,
                        Arc::new(crate::non_wasm::http_clients_cache::creators::UnixSocketHttpClientCreator),
                        None,
                        #[cfg(all(unix, feature = "with-ssh"))]
                        None,
                    )
                    .await?
                } else {
                    let clients_cache = self.get_connections_cache();

                    self.execute_with_retry::<UnixSocketStream, UnixSocketConnector>(
                        request,
                        clients_cache,
                        None,
                        #[cfg(all(unix, feature = "with-ssh"))]
                        None,
                    )
                    .await?
                }
            }
        };

        Ok(response)
    }

    /// The request through an ssh tunnel. `creator` makes its connection when
    /// connections are not reused; otherwise the cache does.
    #[cfg(all(unix, feature = "with-ssh"))]
    async fn execute_ssh<
        TStream: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + Sync + 'static,
        TConnector: MyHttpClientConnector<TStream> + Send + Sync + 'static,
    >(
        mut self,
        request: RequestToExecute,
        mut ssh_credentials: my_ssh::SshCredentials,
        creator: Arc<dyn HttpConnectionResolver<TStream, TConnector>>,
        default_port: Option<u16>,
    ) -> Result<FlUrlResponse, FlUrlError>
    where
        FlUrlHttpConnectionsCache: HttpConnectionResolver<TStream, TConnector>,
    {
        if let Some(private_key_resolver) = self.ssh_security_credentials_resolver.take() {
            ssh_credentials = private_key_resolver
                .update_credentials(&ssh_credentials)
                .await;
        }

        let connection_resolver: Arc<dyn HttpConnectionResolver<TStream, TConnector>> =
            if self.do_not_reuse_connection {
                creator
            } else {
                self.get_connections_cache()
            };

        self.execute_with_retry::<TStream, TConnector>(
            request,
            connection_resolver,
            default_port,
            Some(Arc::new(ssh_credentials)),
        )
        .await
    }
    pub(crate) fn get_connections_cache(&self) -> Arc<FlUrlHttpConnectionsCache> {
        match self.connections_cache.as_ref() {
            Some(cache) => cache.clone(),
            None => crate::non_wasm::CLIENTS_CACHED.clone(),
        }
    }

    fn compress_body(
        &mut self,
        body: Vec<u8>,
        level: CompressionLevel,
    ) -> Result<Vec<u8>, FlUrlError> {
        if body.len() < 64 {
            return Ok(body);
        }

        if !self.headers.has_header("Content-Encoding") {
            self.headers.add("Content-Encoding", "gzip");
        }

        crate::compression::gzip(&body, level)
    }

    fn get_path_and_query_with_leading_slash(&self) -> String {
        let mut path_and_query = self.url_builder.get_path_and_query();
        // A URL with a query but no path yields "?a=b"; the request target must
        // start with "/", so we normalize it here.
        if path_and_query.starts_with('?') {
            path_and_query.insert(0, '/');
        }
        path_and_query
    }

    fn compile_request(
        &mut self,
        method: Method,
        body: HttpRequestBody,
        debug: Option<&mut String>,
    ) -> Result<CompiledHttpRequest, FlUrlError> {
        let result = match self.mode {
            FlUrlMode::H2 => CompiledHttpRequest::new_hyper(
                self.compile_hyper_request(method.clone(), body, debug)?,
                method,
            ),
            FlUrlMode::Http1NoHyper => CompiledHttpRequest::new_my_http_client(
                self.compile_non_hyper_request(method.clone(), body, debug)?,
                method,
            ),
            FlUrlMode::Http1Hyper => CompiledHttpRequest::new_hyper(
                self.compile_hyper_request(method.clone(), body, debug)?,
                method,
            ),
        };

        Ok(result)
    }

    fn compile_hyper_request(
        &mut self,
        method: Method,
        body: HttpRequestBody,
        debug: Option<&mut String>,
    ) -> Result<my_http_client::http::request::Request<Full<Bytes>>, FlUrlError> {
        if let Some(content_type) = body.get_content_type() {
            if !self.headers.has_header("Content-Type") {
                self.headers.add("Content-Type", content_type.as_str());
            }
        }

        let mut body = body.into_vec();

        if let Some(debug) = debug {
            self.compile_debug_info_with_body(debug, method.as_str(), &body);
        }

        if let Some(level) = self.compress_body {
            body = self.compress_body(body, level)?;
        }

        let path_and_query = self.get_path_and_query_with_leading_slash();

        let mut result = match self.mode {
            FlUrlMode::H2 => {
                let scheme = if self.url_builder.get_scheme().is_https() {
                    "https"
                } else {
                    "http"
                };

                // H1 puts the socket path into the Host header and gets away with it;
                // h2 can not, because ':authority' is parsed as a real authority and a
                // path ends one at its first '/'. The caller's own Host header wins,
                // otherwise the placeholder — the socket to open is already known to
                // the connector, so the authority is only what the server sees.
                #[cfg(unix)]
                let authority = if self.url_builder.is_unix_socket() {
                    self.headers
                        .get_host_header_value()
                        .unwrap_or(crate::consts::UNIX_SOCKET_AUTHORITY)
                } else {
                    self.url_builder.get_host_port()
                };

                #[cfg(not(unix))]
                let authority = self.url_builder.get_host_port();

                let uri = Uri::builder()
                    .authority(authority)
                    .path_and_query(path_and_query)
                    .scheme(scheme)
                    .build()?;
                my_http_client::http::request::Builder::new()
                    .version(Version::HTTP_2)
                    .method(method.clone())
                    .uri(uri)
            }
            _ => my_http_client::http::request::Builder::new()
                .method(method.clone())
                .uri(path_and_query),
        };

        self.headers.ensure_valid()?;

        for (key, value) in self.headers.iter() {
            result = result.header(key, value);
        }

        if !self.headers.has_host_header() {
            if !self.mode.is_h2() {
                result = result.header(
                    hyper::header::HOST.as_str(),
                    self.url_builder.get_host_port(),
                );
            }
        }

        if self.url_builder.is_unix_socket() {
            result = result.header(hyper::header::ACCEPT, "*/*");
        } else {
            if !self.headers.has_connection_header {
                if !self.do_not_reuse_connection {
                    result = result.header(hyper::header::CONNECTION.as_str(), "keep-alive");
                }
            }
        }

        let result = match result.body(Full::new(body.into())) {
            Ok(result) => result,
            Err(err) => {
                return Err(FlUrlError::ReadingHyperBodyError(format!(
                    "[{}]. '{}' '{}' Invalid getting fl_url body: {}",
                    method.as_str(),
                    self.url_builder.get_host_port(),
                    self.url_builder.get_path_and_query(),
                    err
                )));
            }
        };

        Ok(result)
    }

    fn compile_non_hyper_request(
        &mut self,
        method: Method,
        body: HttpRequestBody,
        debug: Option<&mut String>,
    ) -> Result<my_http_client::http1::MyHttpRequest, FlUrlError> {
        if let Some(content_type) = body.get_content_type() {
            if !self.headers.has_header("Content-Type") {
                self.headers.add("Content-Type", content_type.as_str());
            }
        }

        let mut body = body.into_vec();

        if let Some(debug) = debug {
            self.compile_debug_info_with_body(debug, method.as_str(), &body);
        }

        if let Some(level) = self.compress_body {
            body = self.compress_body(body, level)?;
        }

        let path_and_query = self.get_path_and_query_with_leading_slash();

        // MyHttpRequestBuilder refuses a request line and a header it must not put on
        // the wire. The request line and the Host header are made of the url, and the
        // url comes from settings — so what is refused in them is an error of the url,
        // the way it is in the hyper modes. The builder keeps the first error it meets
        // and skips every step after it, so each of the two is checked right after the
        // step that takes it.
        let mut builder = MyHttpRequestBuilder::new(method, &path_and_query);

        if let Some(RequestBuildError::ForbiddenByteInPath(byte)) = builder.get_error() {
            return Err(invalid_url(
                &self.url_builder,
                format!("the path and query contain forbidden byte 0x{:02x}", byte),
            ));
        }

        self.headers.ensure_valid()?;

        if !self.headers.has_host_header() {
            builder = builder.append_header("Host", self.url_builder.get_host_port());

            if let Some(RequestBuildError::ForbiddenByteInHeaderValue(byte)) = builder.get_error()
            {
                return Err(invalid_url(
                    &self.url_builder,
                    format!("the host contains forbidden control byte 0x{:02x}", byte),
                ));
            }
        }

        if self.url_builder.is_unix_socket() {
            builder = builder.append_header("Accept", "*/*");
        } else {
            if !self.headers.has_connection_header {
                if !self.do_not_reuse_connection {
                    builder = builder.append_header("Connection", "keep-alive");
                }
            }
        }

        for header in self.headers.iter() {
            builder = builder.append_header(header.0, header.1);
        }

        Ok(builder.build_with_body(body)?)
    }

    pub async fn get(mut self) -> Result<FlUrlResponse, FlUrlError> {
        let request = self.compile_request(Method::GET, HttpRequestBody::Empty, None)?;
        self.execute(RequestToExecute::Compiled(request)).await
    }

    pub async fn get_with_debug(
        mut self,
        request_debug_string: &mut String,
    ) -> Result<FlUrlResponse, FlUrlError> {
        let request =
            self.compile_request(Method::GET, HttpRequestBody::Empty, Some(request_debug_string))?;
        self.execute(RequestToExecute::Compiled(request)).await
    }

    pub async fn head(mut self) -> Result<FlUrlResponse, FlUrlError> {
        let request = self.compile_request(Method::HEAD, HttpRequestBody::Empty, None)?;
        self.execute(RequestToExecute::Compiled(request)).await
    }

    pub async fn head_with_debug(
        mut self,
        request_debug_string: &mut String,
    ) -> Result<FlUrlResponse, FlUrlError> {
        let request = self.compile_request(
            Method::HEAD,
            HttpRequestBody::Empty,
            Some(request_debug_string),
        )?;
        self.execute(RequestToExecute::Compiled(request)).await
    }

    pub async fn post(mut self, body: impl Into<HttpRequestBody>) -> Result<FlUrlResponse, FlUrlError> {
        let request = self.compile_request(Method::POST, body.into(), None)?;
        self.execute(RequestToExecute::Compiled(request)).await
    }

    pub async fn post_with_debug(
        mut self,
        body: impl Into<HttpRequestBody>,
        request_debug_string: &mut String,
    ) -> Result<FlUrlResponse, FlUrlError> {
        let body = body.into();

        let request = self.compile_request(Method::POST, body, Some(request_debug_string))?;
        self.execute(RequestToExecute::Compiled(request)).await
    }

    pub async fn post_request_streamed<TBody>(
        self,
        body: TBody,
        content_length: Option<usize>,
    ) -> Result<FlUrlResponse, FlUrlError>
    where
        TBody: hyper::body::Body<Data = Bytes> + Send + Sync + 'static,
        TBody::Error: std::fmt::Display,
    {
        self.execute_streamed(Method::POST, body, content_length)
            .await
    }

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
        self.execute_streamed_with_debug(Method::POST, body, content_length, request_debug_string)
            .await
    }

    pub async fn put_request_streamed<TBody>(
        self,
        body: TBody,
        content_length: Option<usize>,
    ) -> Result<FlUrlResponse, FlUrlError>
    where
        TBody: hyper::body::Body<Data = Bytes> + Send + Sync + 'static,
        TBody::Error: std::fmt::Display,
    {
        self.execute_streamed(Method::PUT, body, content_length)
            .await
    }

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
        self.execute_streamed_with_debug(Method::PUT, body, content_length, request_debug_string)
            .await
    }

    pub async fn patch_request_streamed<TBody>(
        self,
        body: TBody,
        content_length: Option<usize>,
    ) -> Result<FlUrlResponse, FlUrlError>
    where
        TBody: hyper::body::Body<Data = Bytes> + Send + Sync + 'static,
        TBody::Error: std::fmt::Display,
    {
        self.execute_streamed(Method::PATCH, body, content_length)
            .await
    }

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
        self.execute_streamed_with_debug(Method::PATCH, body, content_length, request_debug_string)
            .await
    }

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
        self.execute_streamed_impl(method, body, content_length, None)
            .await
    }

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
        self.execute_streamed_impl(method, body, content_length, Some(request_debug_string))
            .await
    }

    async fn execute_streamed_impl<TBody>(
        mut self,
        method: Method,
        body: TBody,
        content_length: Option<usize>,
        debug: Option<&mut String>,
    ) -> Result<FlUrlResponse, FlUrlError>
    where
        TBody: hyper::body::Body<Data = Bytes> + Send + Sync + 'static,
        TBody::Error: std::fmt::Display,
    {
        if self.compress_body.is_some() {
            return Err(FlUrlError::StreamedBodyCanNotBeCompressed);
        }

        self.mode = FlUrlMode::Http1Hyper;

        if let Some(debug) = debug {
            self.compile_debug_info_streamed(debug, method.as_str());
        }

        let request = self.compile_streamed_request(method, body)?;

        self.execute(RequestToExecute::streamed(request, content_length))
            .await
    }

    /// Builds the request head for [`Self::execute_streamed`] and erases the body to
    /// the trait object the connection carries. Mirrors the header work of
    /// `compile_hyper_request`, minus everything that needs the body in hand:
    /// no `Content-Type` derived from the payload (a stream carries none — set it with
    /// [`Self::with_header`]), no debug dump of the body, no compression.
    fn compile_streamed_request<TBody>(
        &mut self,
        method: Method,
        body: TBody,
    ) -> Result<my_http_client::HyperRequest, FlUrlError>
    where
        TBody: hyper::body::Body<Data = Bytes> + Send + Sync + 'static,
        TBody::Error: std::fmt::Display,
    {
        let path_and_query = self.get_path_and_query_with_leading_slash();

        // Http1Hyper only, so the origin-form target is the right one — no absolute
        // URI + :authority the way the h2 branch builds it.
        let mut result = my_http_client::http::request::Builder::new()
            .method(method.clone())
            .uri(path_and_query);

        self.headers.ensure_valid()?;

        for (key, value) in self.headers.iter() {
            result = result.header(key, value);
        }

        if !self.headers.has_host_header() {
            result = result.header(
                hyper::header::HOST.as_str(),
                self.url_builder.get_host_port(),
            );
        }

        if self.url_builder.is_unix_socket() {
            result = result.header(hyper::header::ACCEPT, "*/*");
        } else {
            if !self.headers.has_connection_header {
                if !self.do_not_reuse_connection {
                    result = result.header(hyper::header::CONNECTION.as_str(), "keep-alive");
                }
            }
        }

        let body = http_body_util::BodyExt::boxed(http_body_util::BodyExt::map_err(
            body,
            |err| err.to_string(),
        ));

        match result.body(body) {
            Ok(result) => Ok(result),
            Err(err) => Err(FlUrlError::ReadingHyperBodyError(format!(
                "[{}]. '{}' '{}' Invalid getting fl_url streamed body: {}",
                method.as_str(),
                self.url_builder.get_host_port(),
                self.url_builder.get_path_and_query(),
                err
            ))),
        }
    }

    pub async fn post_json(
        mut self,
        json: &impl serde::Serialize,
    ) -> Result<FlUrlResponse, FlUrlError> {
        let body = HttpRequestBody::try_as_json(json)?;
        let request = self.compile_request(Method::POST, body, None)?;

        self.execute(RequestToExecute::Compiled(request)).await
    }

    pub async fn patch(mut self, body: impl Into<HttpRequestBody>) -> Result<FlUrlResponse, FlUrlError> {
        let request = self.compile_request(Method::PATCH, body.into(), None)?;
        self.execute(RequestToExecute::Compiled(request)).await
    }

    pub async fn patch_with_debug(
        mut self,
        body: impl Into<HttpRequestBody>,
        request_debug_string: &mut String,
    ) -> Result<FlUrlResponse, FlUrlError> {
        let request = self.compile_request(Method::PATCH, body.into(), Some(request_debug_string))?;
        self.execute(RequestToExecute::Compiled(request)).await
    }

    pub async fn patch_json(
        mut self,
        json: &impl serde::Serialize,
    ) -> Result<FlUrlResponse, FlUrlError> {
        let body = HttpRequestBody::try_as_json(json)?;
        let request = self.compile_request(Method::PATCH, body, None)?;

        self.execute(RequestToExecute::Compiled(request)).await
    }

    pub async fn put(mut self, body: impl Into<HttpRequestBody>) -> Result<FlUrlResponse, FlUrlError> {
        let request = self.compile_request(Method::PUT, body.into(), None)?;
        self.execute(RequestToExecute::Compiled(request)).await
    }

    pub async fn put_with_debug(
        mut self,
        body: impl Into<HttpRequestBody>,
        request_debug_string: &mut String,
    ) -> Result<FlUrlResponse, FlUrlError> {
        let request = self.compile_request(Method::PUT, body.into(), Some(request_debug_string))?;
        self.execute(RequestToExecute::Compiled(request)).await
    }

    pub async fn put_json(
        mut self,
        json: &impl serde::Serialize,
    ) -> Result<FlUrlResponse, FlUrlError> {
        let body = HttpRequestBody::try_as_json(json)?;
        let request = self.compile_request(Method::PUT, body, None)?;
        self.execute(RequestToExecute::Compiled(request)).await
    }

    pub async fn delete(mut self) -> Result<FlUrlResponse, FlUrlError> {
        let request = self.compile_request(Method::DELETE, HttpRequestBody::Empty, None)?;
        self.execute(RequestToExecute::Compiled(request)).await
    }

    pub async fn delete_with_debug(
        mut self,
        request_debug_string: &mut String,
    ) -> Result<FlUrlResponse, FlUrlError> {
        let request =
            self.compile_request(Method::DELETE, HttpRequestBody::Empty, Some(request_debug_string))?;
        self.execute(RequestToExecute::Compiled(request)).await
    }
    fn compile_debug_info(&self, out: &mut String) {
        out.push_str("PathAndQuery: '");
        out.push_str(self.url_builder.get_path_and_query().as_str());
        out.push_str("'; Headers: '");
        out.push_str(self.headers.headers.as_str());
    }
    fn compile_debug_info_with_body(
        &self,
        request_debug_string: &mut String,
        method: &str,
        body: &[u8],
    ) {
        request_debug_string.push_str("[");
        request_debug_string.push_str(method);
        request_debug_string.push_str("] ");

        self.compile_debug_info(request_debug_string);

        if body.len() == 0 {
            return;
        }
        match std::str::from_utf8(body) {
            Ok(body_as_str) => {
                request_debug_string.push_str("Body: ");
                request_debug_string.push_str(body_as_str);
            }
            Err(_) => {
                request_debug_string.push_str("Body: ");
                request_debug_string.push_str(body.len().to_string().as_str());
                request_debug_string.push_str(" non string bytes");
            }
        }
    }

    /// Debug dump for a streamed request: the head only. There is no body line — a
    /// streamed payload exists only as it is written to the socket, so printing it
    /// would mean buffering the very thing streaming avoids.
    fn compile_debug_info_streamed(&self, request_debug_string: &mut String, method: &str) {
        request_debug_string.push_str("[");
        request_debug_string.push_str(method);
        request_debug_string.push_str("] ");

        self.compile_debug_info(request_debug_string);
    }

    pub fn to_string(&self) -> String {
        let mut result = String::new();
        self.compile_debug_info(&mut result);

        result
    }

    async fn get_connection_params<'s>(
        &'s self,
        default_port: Option<u16>,
        #[cfg(all(unix, feature = "with-ssh"))] ssh_credentials: Option<Arc<my_ssh::SshCredentials>>,
    ) -> ConnectionParams<'s> {
        let remote_endpoint = self.url_builder.get_remote_endpoint(default_port);

        #[cfg(all(unix, feature = "with-ssh"))]
        let ssh_session = match ssh_credentials.clone() {
            Some(ssh_credentials) => {
                let ssh_credentials = Arc::new(ssh_credentials);
                let ssh_session = my_ssh::SSH_SESSIONS_POOL
                    .get_or_create(&ssh_credentials)
                    .await;

                Some(ssh_session)
            }
            None => None,
        };

        ConnectionParams {
            mode: self.mode,
            remote_endpoint,
            resolved_ip: self.resolved_ip,
            host_header: self.headers.get_host_header_value(),
            #[cfg(feature = "_tls")]
            client_certificate: self.client_cert.as_ref(),
            accept_invalid_certificate: self.accept_invalid_certificate,
            #[cfg(all(unix, feature = "with-ssh"))]
            ssh_session,
            #[cfg(all(unix, feature = "with-ssh"))]
            is_unix_socket: self.url_builder.is_unix_socket(),
            reuse_connection_timeout_seconds: self.reuse_connection_timeout_sec,
        }
    }

    async fn execute_with_retry<
        TStream: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + Sync + 'static,
        TConnector: MyHttpClientConnector<TStream> + Send + Sync + 'static,
    >(
        self,
        mut request: RequestToExecute,
        http_connection_resolver: Arc<dyn HttpConnectionResolver<TStream, TConnector>>,
        default_port: Option<u16>,
        #[cfg(all(unix, feature = "with-ssh"))] ssh_credentials: Option<Arc<my_ssh::SshCredentials>>,
    ) -> Result<FlUrlResponse, FlUrlError> {
        if self.print_input_request {
            request.print_http_headers();
        }
        let mut attempt_no = 0;
        // A streamed body is consumed as it is sent, so there is nothing left to
        // replay — neither `with_retries` nor `with_retry` applies to it.
        let retry = if request.is_streamed() {
            RetryPolicy::default()
        } else {
            self.retry
        };
        let request_timeout = self.request_timeout;
        let params: ConnectionParams<'_> = self
            .get_connection_params(
                default_port,
                #[cfg(all(unix, feature = "with-ssh"))]
                ssh_credentials,
            )
            .await;

        loop {
            let connection = match http_connection_resolver.get_http_connection(&params).await {
                Ok(connection) => connection,
                // Nothing went out, and a replay would get the same answer.
                Err(err) => return Err(map_my_http_client_error(err)),
            };

            let response = match &mut request {
                RequestToExecute::Compiled(request) => {
                    connection.do_request(request, request_timeout).await
                }
                RequestToExecute::Streamed {
                    request,
                    content_size,
                    ..
                } => match request.take() {
                    Some(request) => {
                        connection
                            .do_streamed_request(request, *content_size, request_timeout)
                            .await
                    }
                    // Unreachable while the retry policy is pinned to none above; kept
                    // as an error rather than an unwrap so a future change to the retry
                    // policy can not silently resend a half-consumed body.
                    None => Err(my_http_client::MyHttpClientError::CanNotExecuteRequest(
                        "A streamed request body has already been consumed and can not be replayed"
                            .to_string(),
                    )),
                },
            };

            match response {
                Ok(response) => {
                    // `with_retry` replays a 5xx the way it replays a transport
                    // failure. The response is dropped unread together with its
                    // connection handle, which aborts the body before the pause: an
                    // HTTP/1 connection is checked out exclusively and is disposed
                    // with it, an h2 stream is reset while the shared h2 connection
                    // stays pooled for the next attempt.
                    if attempt_no < retry.amount()
                        && request.method_is_idempotent()
                        && retry.retries_status(response.status().as_u16())
                    {
                        drop(response);
                        drop(connection);
                        attempt_no += 1;
                        pause_before_next_attempt(retry.delay()).await;
                        continue;
                    }

                    // The connection stays checked out until the response body
                    // is fully consumed; the returner puts it back (or disposes
                    // it) at that point.
                    let connection = Box::new(
                        crate::non_wasm::http_clients_cache::PooledConnectionReturner {
                            resolver: http_connection_resolver.clone(),
                            connection,
                        },
                    );

                    return Ok(FlUrlResponse::of_request(
                        self.url_builder,
                        response,
                        connection,
                        self.response_body_timeout,
                        self.decompress_gzip_response,
                    ));
                }
                Err(err) => {
                    // A single timeout means a slow response, not a dead
                    // connection — the shared H2 client must survive it (its
                    // own consecutive-timeouts policy handles dead peers). Any
                    // other error evicts the connection from the pool; dropping
                    // the Arc disposes it.
                    if matches!(&err, my_http_client::MyHttpClientError::RequestTimeout(_)) {
                        drop(connection);
                    } else {
                        http_connection_resolver.drop_connection(connection).await;
                    }

                    if !error_is_safe_to_retry(&err, &request) || attempt_no >= retry.amount() {
                        return Err(map_my_http_client_error(err));
                    }

                    attempt_no += 1;
                    pause_before_next_attempt(retry.delay()).await;
                }
            }
        }
    }
}

/// The pause `with_retry` puts between two attempts. `with_retries` has none and
/// replays at once, without touching the timer — exactly as it always did.
async fn pause_before_next_attempt(delay: Duration) {
    if !delay.is_zero() {
        tokio::time::sleep(delay).await;
    }
}

/// Replay safety: fl-url's outer retry loop replays only idempotent requests.
/// Error kinds are NOT a reliable pre-wire signal across the three client
/// modes (e.g. in Http1NoHyper a `CanNotConnectToRemoteHost` can surface after
/// a POST already hit the wire, when the internal reconnect after a mid-flight
/// disconnect fails), so a non-idempotent request is never replayed here —
/// my-http-client's own retry loops already cover the genuinely-safe cases.
fn error_is_safe_to_retry(
    err: &my_http_client::MyHttpClientError,
    request: &RequestToExecute,
) -> bool {
    match err {
        // The connection is consumed by the upgrade; a retry would just
        // re-trigger it.
        my_http_client::MyHttpClientError::UpgradedToWebSocket => false,
        _ => request.method_is_idempotent(),
    }
}

fn map_my_http_client_error(err: my_http_client::MyHttpClientError) -> FlUrlError {
    match err {
        my_http_client::MyHttpClientError::RequestTimeout(_) => FlUrlError::Timeout,
        other => FlUrlError::MyHttpClientError(other),
    }
}

fn invalid_url(url_builder: &UrlBuilder, reason: String) -> FlUrlError {
    FlUrlError::InvalidUrl(format!(
        "Invalid url '{}': {}",
        url_builder.to_string().escape_debug(),
        reason
    ))
}

#[cfg(test)]
mod test {

    use super::FlUrlInner;
    use crate::FlUrl;

    /// Most of the tests below look inside a request, so they start from the inner
    /// state itself — made of a url that is known to be valid.
    fn inner(url: &str) -> FlUrlInner {
        FlUrlInner::new(url).unwrap()
    }

    #[cfg(feature = "_tls")]
    #[tokio::test]
    #[ignore = "hits the network (jetdev.eu); run with --ignored"]
    async fn test_h1() {
        let mut fl_url_resp = FlUrl::new("https://jetdev.eu/img/logo.png")
            .do_not_reuse_connection()
            .get()
            .await
            .unwrap();

        println!("{}", fl_url_resp.get_status_code());

        let resp = fl_url_resp.get_body().unwrap().into_vec(usize::MAX).await.unwrap();
        println!("{}", resp.len());
    }

    #[cfg(feature = "_tls")]
    #[tokio::test]
    #[ignore = "hits the network (jetdev.eu); run with --ignored"]
    async fn test_h2() {
        let mut fl_url_resp = FlUrl::new("https://jetdev.eu/img/logo.png")
            .update_mode(crate::non_wasm::fl_url::FlUrlMode::H2)
            .get()
            .await
            .unwrap();

        let resp = fl_url_resp.get_body().unwrap().into_vec(usize::MAX).await.unwrap();

        println!("{}", resp.len());
    }

    #[cfg(feature = "_tls")]
    #[tokio::test]
    #[ignore = "hits the network (jetdev.eu); run with --ignored"]
    async fn test_head() {
        let mut fl_url_resp = FlUrl::new("https://jetdev.eu/img/logo.png")
            .head()
            .await
            .unwrap();

        let resp = fl_url_resp.get_body().unwrap().into_vec(usize::MAX).await.unwrap();

        println!("{}", resp.len());
    }

    #[test]
    fn execute_request_fills_url_headers_and_body_from_model() {
        use my_http_utils::macros::MyHttpInput;

        #[derive(MyHttpInput)]
        struct CreateUser {
            #[http_path(name = "orgId", description = "")]
            org_id: String,
            #[http_query(name = "notify", description = "")]
            notify: bool,
            #[http_header(name = "X-Api-Key", description = "")]
            api_key: String,
            #[http_body(name = "name", description = "")]
            name: String,
        }

        let model = CreateUser {
            org_id: "org-42".to_string(),
            notify: true,
            api_key: "secret".to_string(),
            name: "John".to_string(),
        };

        // Base host + static route prefix set by the caller, model fills the rest.
        let mut fl_url = inner("https://api.example.com")
            .append_path_segment("api")
            .append_path_segment("users");

        let body = fl_url.fill_from_model(model).unwrap();

        // Static prefix + model path segment + model query param.
        assert_eq!(
            fl_url.url_builder.get_path_and_query(),
            "/api/users/org-42?notify=true"
        );

        // Model header field landed in FlUrlHeaders.
        assert!(fl_url
            .headers
            .iter()
            .any(|(name, value)| name == "X-Api-Key" && value == "secret"));

        // Body field serialized to JSON.
        match body {
            crate::body::HttpRequestBody::Json(bytes) => {
                let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
                assert_eq!(json["name"], "John");
            }
            _ => panic!("expected a JSON body"),
        }
    }

    #[test]
    fn execute_request_builds_multipart_body_with_random_boundary() {
        use my_http_utils::macros::MyHttpInput;

        // A form-data model is the only path where `FlUrlRnd` is actually used:
        // it supplies the random `multipart/form-data` boundary suffix.
        #[derive(MyHttpInput)]
        struct UploadForm {
            #[http_form_data(name = "title", description = "")]
            title: String,
            #[http_form_data(name = "count", description = "")]
            count: i32,
        }

        let model = UploadForm {
            title: "MyTitle".to_string(),
            count: 5,
        };

        let mut fl_url = inner("https://api.example.com").append_path_segment("upload");

        let body = fl_url.fill_from_model(model).unwrap();

        // The model yields a FormData body, and its content type carries the
        // random boundary generated via `FlUrlRnd`.
        let content_type = body.get_content_type().unwrap().as_str().to_string();
        assert!(content_type.starts_with("multipart/form-data; boundary="));

        let boundary = content_type
            .split("boundary=")
            .nth(1)
            .expect("content type must carry a boundary");
        // A non-empty random suffix was appended to the fixed boundary prefix.
        assert!(boundary.len() > "------DataFormBoundary".len());

        // The very boundary advertised in the content type delimits the body
        // bytes — i.e. the random string flowed all the way through.
        let text = String::from_utf8(body.into_vec()).unwrap();
        assert!(text.contains(boundary));
        assert!(text.contains("name=\"title\""));
        assert!(text.contains("MyTitle"));
        assert!(text.contains("name=\"count\""));
        assert!(text.contains('5'));
    }

    // Compile-only: the "no model" example must type-check. `EmptyRequestModel`
    // is the parameter-less stand-in — no model type has to be derived or named.
    #[allow(dead_code)]
    fn readme_no_model_example_compiles() {
        use crate::{EmptyRequestModel, FlUrlError, FlUrlResponse, HttpVerb};

        async fn _call() -> Result<FlUrlResponse, FlUrlError> {
            FlUrl::new("https://api.example.com")
                .append_path_segment("health")
                .execute_request(HttpVerb::Get, EmptyRequestModel)
                .await
        }
    }

    #[test]
    fn execute_request_keeps_headers_added_before_it() {
        use my_http_utils::macros::MyHttpInput;

        #[derive(MyHttpInput)]
        struct Model {
            #[http_header(name = "X-Api-Key", description = "")]
            api_key: String,
            #[http_body(name = "name", description = "")]
            name: String,
        }

        let model = Model {
            api_key: "secret".to_string(),
            name: "John".to_string(),
        };

        // Two headers set on the builder BEFORE the model is poured in.
        let mut fl_url = inner("https://api.example.com")
            .with_header("Authorization", "Bearer token")
            .unwrap()
            .with_header("X-Trace", "abc")
            .unwrap();

        fl_url.fill_from_model(model).unwrap();

        let has = |n: &str, v: &str| {
            fl_url
                .headers
                .iter()
                .any(|(name, value)| name == n && value == v)
        };

        // The headers added before execute_request/fill_from_model survive...
        assert!(has("Authorization", "Bearer token"));
        assert!(has("X-Trace", "abc"));
        // ...right alongside the header field the model pushes in.
        assert!(has("X-Api-Key", "secret"));
    }

    /// Every verb goes through `compile_request`, which is what the `*_with_debug`
    /// methods hand the debug string to — so the dump carries the verb, the target
    /// and, for a body-carrying verb, the payload.
    #[test]
    fn every_verb_dumps_the_request_into_the_debug_string() {
        use crate::body::HttpRequestBody;
        use hyper::Method;

        let cases = [
            (Method::GET, None),
            (Method::HEAD, None),
            (Method::DELETE, None),
            (Method::POST, Some("{\"a\":1}")),
            (Method::PUT, Some("{\"b\":2}")),
            (Method::PATCH, Some("{\"c\":3}")),
        ];

        for (method, body) in cases {
            let mut fl_url = inner("https://api.example.com")
                .append_path_segment("users")
                .append_query_param("notify", Some("true"))
                .with_header("X-Api-Key", "secret")
                .unwrap();

            let request_body = match body {
                Some(body) => HttpRequestBody::from_raw_data(
                    body.as_bytes().to_vec(),
                    Some("application/json".into()),
                ),
                None => HttpRequestBody::Empty,
            };

            let mut debug = String::new();
            fl_url
                .compile_request(method.clone(), request_body, Some(&mut debug))
                .unwrap();

            assert!(
                debug.starts_with(&format!("[{}] ", method.as_str())),
                "[{}] dump must open with the verb: {}",
                method.as_str(),
                debug
            );
            assert!(
                debug.contains("/users?notify=true"),
                "[{}] dump must carry the target: {}",
                method.as_str(),
                debug
            );
            assert!(
                debug.contains("X-Api-Key"),
                "[{}] dump must carry the headers: {}",
                method.as_str(),
                debug
            );

            match body {
                Some(body) => assert!(
                    debug.contains(&format!("Body: {}", body)),
                    "[{}] dump must carry the body: {}",
                    method.as_str(),
                    debug
                ),
                // A bodyless verb has nothing to print — and prints nothing.
                None => assert!(
                    !debug.contains("Body: "),
                    "[{}] must not invent a body: {}",
                    method.as_str(),
                    debug
                ),
            }
        }
    }

    /// A streamed body cannot be dumped — it exists only as it is written to the
    /// socket — so the dump is the head and nothing else.
    #[test]
    fn a_streamed_request_dumps_the_head_without_a_body() {
        let fl_url = inner("https://api.example.com")
            .append_path_segment("upload")
            .with_header("X-Api-Key", "secret")
            .unwrap();

        let mut debug = String::new();
        fl_url.compile_debug_info_streamed(&mut debug, "POST");

        assert!(debug.starts_with("[POST] "), "{}", debug);
        assert!(debug.contains("/upload"), "{}", debug);
        assert!(debug.contains("X-Api-Key"), "{}", debug);
        assert!(!debug.contains("Body"), "{}", debug);
    }

    /// `server-name@ip`: the ip only picks the socket; to everything the server
    /// sees — Host, :authority, SNI — the url's host is the server name.
    #[tokio::test]
    async fn server_name_at_ip_url_keeps_the_server_name_as_host() {
        let fl_url = inner("https://domain.com@15.0.0.5/xxx/fff");

        assert_eq!(fl_url.get_resolved_ip(), Some("15.0.0.5".parse().unwrap()));
        assert_eq!(fl_url.url_builder.get_host(), "domain.com");
        assert_eq!(fl_url.url_builder.get_host_port(), "domain.com");
        assert_eq!(fl_url.url_builder.get_path_and_query(), "/xxx/fff");

        let params = fl_url
            .get_connection_params(
                Some(443),
                #[cfg(all(unix, feature = "with-ssh"))]
                None,
            )
            .await;
        assert_eq!(params.resolved_ip, Some("15.0.0.5".parse().unwrap()));
        assert_eq!(params.remote_endpoint.get_port(), Some(443));
        #[cfg(feature = "_tls")]
        assert_eq!(params.get_server_name(), "domain.com");
    }

    #[cfg(all(unix, feature = "with-ssh"))]
    #[test]
    fn server_name_at_ip_url_behind_ssh() {
        let fl_url = inner("ssh://user@ssh.example.com:22->http://domain.com@10.0.0.7:8080/xxx");

        assert!(fl_url.via_ssh());
        assert_eq!(fl_url.get_resolved_ip(), Some("10.0.0.7".parse().unwrap()));
        assert_eq!(fl_url.url_builder.get_host_port(), "domain.com:8080");
        assert_eq!(fl_url.url_builder.get_path_and_query(), "/xxx");
    }
}
