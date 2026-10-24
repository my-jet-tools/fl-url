//! # FlUrl
//!
//! A fluent, async HTTP client. The **same public API** compiles for two very
//! different transports, selected automatically by the target:
//!
//! * **native** (`cfg(not(target_arch = "wasm32"))`) — the full hyper/tokio
//!   backend in [`mod@non_wasm`]: HTTP/1.1 & HTTP/2, connection pooling, unix
//!   sockets, and — behind feature gates — TLS + client certificates
//!   (`with-ring-tls` for ring, or `with-rust-tls` for a pure-Rust provider) and
//!   (on unix) SSH tunneling (`with-ssh`). With neither TLS feature the crate
//!   links no TLS stack at all and an `https://` request fails with
//!   [`FlUrlError::UnsupportedScheme`].
//! * **wasm32** (`cfg(target_arch = "wasm32")`) — the browser `fetch` backend in
//!   [`mod@wasm`]. Connection pooling, TLS and redirects are handled by the
//!   browser, so those knobs become no-ops; every request-building and
//!   response-reading method keeps its native signature.
//!
//! Both backends alias their `FlUrl`, `FlUrlResponse`, `FlUrlHeaders`, … to this
//! crate root, so `flurl::FlUrl` resolves to whichever backend is active and
//! call sites need no `cfg` of their own.
//!
//! The shared, transport-agnostic pieces — [`enum@FlUrlError`], the request
//! [`body`] types and [`CompressionLevel`] — live at the crate root and are used
//! by both backends.

// ---- Shared, target-agnostic modules ---------------------------------------

pub mod body;
mod compression;
mod empty_request_model;
mod errors;
#[cfg(not(target_arch = "wasm32"))]
mod fl_drop_connection_scenario;
mod host_check;
mod retry_policy;

pub use compression::CompressionLevel;
pub use empty_request_model::*;
pub use errors::*;
pub(crate) use retry_policy::RetryPolicy;

pub extern crate my_http_utils;

/// Its `AsyncBytesStream` is what the reader of a response body is read in chunks
/// through, so it comes with fl-url, at the version fl-url is built on.
pub extern crate rust_extensions;

#[cfg(not(target_arch = "wasm32"))]
mod consts;

// ---- Native backend --------------------------------------------------------

#[cfg(not(target_arch = "wasm32"))]
mod non_wasm;

#[cfg(not(target_arch = "wasm32"))]
pub use non_wasm::*;

// ---- wasm backend ----------------------------------------------------------

#[cfg(target_arch = "wasm32")]
mod wasm;

#[cfg(target_arch = "wasm32")]
pub use wasm::*;
