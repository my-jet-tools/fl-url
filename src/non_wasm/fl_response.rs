use std::{
    collections::HashMap,
    fmt::Debug,
    pin::Pin,
    task::{Context, Poll},
    time::Duration,
};

use bytes::Bytes;
use http::response::Parts;
use http_body_util::BodyExt;
use hyper::header::{CONNECTION, CONTENT_ENCODING};
use my_http_client::{http1::MyHttpResponse, HyperResponse};
use my_http_utils::UrlBuilder;

use crate::{ConnectionReturner, FlUrlBodyReader, FlUrlError, FlUrlReadingHeaderError};

const BODY_IS_TAKEN: &str = "Response body is already taken";

/// A response as it is once its head is read: the status and the headers are here,
/// and the body is not read yet — [`Self::get_body`] gives the reader it is read with.
pub struct FlUrlResponse {
    pub url: UrlBuilder,
    // The head of the response: its status, its version and its headers.
    head: Parts,
    // The body, until `get_body` takes it. It owns the checked-out connection: a
    // response dropped with its body in it drops the connection, which disposes it.
    body: Option<FlUrlBodyReader>,
}

impl Debug for FlUrlResponse {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FlUrlResponse")
            .field("url", &self.url.to_string())
            .field("status_code", &self.head.status)
            .finish()
    }
}

impl FlUrlResponse {
    /// A response made from the outside of a request of FlUrl: it has no connection
    /// of the pool to settle, no timeout of the body and nothing to decode.
    pub fn from_http1_response<
        TStream: tokio::io::AsyncRead + tokio::io::AsyncWrite + Send + Sync + 'static,
    >(
        url: UrlBuilder,
        response: MyHttpResponse<TStream>,
    ) -> Self {
        Self::create(url, response, None, None, false)
    }

    /// The response to a request of FlUrl. `connection` is the connection it has come
    /// over: it stays checked out until the body is read, and the reader of the body
    /// puts it back — or disposes it — at that point.
    pub(crate) fn of_request<
        TStream: tokio::io::AsyncRead + tokio::io::AsyncWrite + Send + Sync + 'static,
    >(
        url: UrlBuilder,
        response: MyHttpResponse<TStream>,
        connection: Box<dyn ConnectionReturner>,
        body_read_timeout: Option<Duration>,
        accept_gzip: bool,
    ) -> Self {
        Self::create(url, response, Some(connection), body_read_timeout, accept_gzip)
    }

    fn create<TStream: tokio::io::AsyncRead + tokio::io::AsyncWrite + Send + Sync + 'static>(
        url: UrlBuilder,
        response: MyHttpResponse<TStream>,
        connection: Option<Box<dyn ConnectionReturner>>,
        body_read_timeout: Option<Duration>,
        accept_gzip: bool,
    ) -> Self {
        let (head, body) = match response {
            MyHttpResponse::Response(response) => response.into_parts(),
            // The answer to a websocket handshake carries no body.
            MyHttpResponse::WebSocketUpgrade { response, .. } => {
                (response.into_parts().0, my_http_client::BodyReader::empty())
            }
        };

        let body = FlUrlBodyReader::new(
            body,
            connection,
            connection_can_be_reused(&head),
            body_read_timeout,
            accept_gzip && is_gzip(&head),
        );

        Self {
            url,
            head,
            body: Some(body),
        }
    }

    /// The body of the response: nothing of it is read by then, and the reader is how
    /// it is read — piece by piece with `next_item()`, or as a whole within a limit
    /// with `into_vec(max_size)`, `into_string(max_size)` and `into_json(max_size)`.
    ///
    /// The body is taken out of the response, which keeps its status and its headers.
    /// It is there to be taken once: the next call fails.
    pub fn get_body(&mut self) -> Result<FlUrlBodyReader, FlUrlError> {
        self.body
            .take()
            .ok_or_else(|| FlUrlError::ReadingHyperBodyError(BODY_IS_TAKEN.to_string()))
    }

    pub fn drop_connection(&self) -> bool {
        connection_close_is_asked(&self.head)
    }

    /// The response as hyper's, its body still streaming from the connection, as it
    /// came — nothing of it is decoded. Once the body is taken with [`Self::get_body`]
    /// there is none to give, and the body of the response fails with that.
    ///
    /// It is for passing the body on — to hyper's server, which writes a frame and lets
    /// it go. hyper's `collect()` keeps every frame until the end: in
    /// `FlUrlMode::Http1NoHyper` a body bigger than the two buffers the connection is
    /// read into waits for ever that way. A body is read into memory with
    /// [`Self::get_body`] and `into_vec`.
    pub fn into_hyper_response(self) -> HyperResponse {
        let body = match self.body {
            Some(body) => body.into_hyper_body(),
            None => BodyIsTaken(Some(BODY_IS_TAKEN.to_string())).boxed(),
        };

        http::Response::from_parts(self.head, body)
    }

    pub fn get_header(&self, name: &str) -> Result<Option<&str>, FlUrlReadingHeaderError> {
        match self.head.headers.get(name) {
            Some(value) => Ok(Some(value.to_str()?)),
            None => Ok(None),
        }
    }

    pub fn get_header_case_insensitive(
        &self,
        name: &str,
    ) -> Result<Option<&str>, FlUrlReadingHeaderError> {
        for (header, value) in self.head.headers.iter() {
            if rust_extensions::str_utils::compare_strings_case_insensitive(header.as_str(), name) {
                return Ok(Some(value.to_str()?));
            }
        }

        Ok(None)
    }

    pub fn get_headers(&self) -> HashMap<&str, Option<&str>> {
        let mut result = HashMap::new();

        for (key, value) in &self.head.headers {
            if let Ok(value) = value.to_str() {
                result.insert(key.as_str(), Some(value));
            }
        }

        result
    }

    pub fn fill_headers_to_hashmap_of_string(&self, dest: &mut HashMap<String, Option<String>>) {
        for (key, value) in &self.head.headers {
            dest.insert(
                key.as_str().to_string(),
                value.to_str().ok().map(|value| value.to_string()),
            );
        }
    }

    pub fn get_status_code(&self) -> u16 {
        self.head.status.as_u16()
    }

}

/// The status code and the headers: `StatusCode: 200; Headers: name='value';...`.
impl std::fmt::Display for FlUrlResponse {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "StatusCode: {}; Headers: ", self.get_status_code())?;

        for (key, value) in self.get_headers() {
            write!(f, "{}='{}';", key, value.unwrap_or(""))?;
        }

        Ok(())
    }
}

fn connection_close_is_asked(head: &Parts) -> bool {
    head.headers
        .get(CONNECTION)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.eq_ignore_ascii_case("close"))
}

/// Whether the connection may serve the next request once the body of this response
/// is read to its end: not when the response asks to close it, nor when its status is
/// one a connection is dropped on.
fn connection_can_be_reused(head: &Parts) -> bool {
    !connection_close_is_asked(head)
        && !crate::fl_drop_connection_scenario::should_drop_connection_by_status(
            head.status.as_u16(),
        )
}

fn is_gzip(head: &Parts) -> bool {
    head.headers
        .get(CONTENT_ENCODING)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.eq_ignore_ascii_case("gzip"))
}

/// The body of a response handed on as hyper's after its body was taken: it has
/// nothing to give but the reason, and its first frame fails with it.
struct BodyIsTaken(Option<String>);

impl hyper::body::Body for BodyIsTaken {
    type Data = Bytes;
    type Error = String;

    fn poll_frame(
        mut self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
    ) -> Poll<Option<Result<hyper::body::Frame<Bytes>, String>>> {
        Poll::Ready(self.0.take().map(Err))
    }
}
