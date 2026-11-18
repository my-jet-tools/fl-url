use std::collections::HashMap;
use std::fmt::Debug;

use my_http_utils::UrlBuilder;

use crate::wasm::fetch::collect_headers;
use crate::{FlUrlBodyReader, FlUrlError, FlUrlReadingHeaderError};

const BODY_IS_TAKEN: &str = "response body is already taken";

/// wasm counterpart of the native `FlUrlResponse`.
///
/// The status and the headers are read from the `fetch` `Response` as it comes; the
/// body is not read yet — [`Self::get_body`] gives the reader it is read with.
pub struct FlUrlResponse {
    pub url: UrlBuilder,
    status_code: u16,
    headers: Vec<(String, String)>,
    // The body, until `get_body` takes it. A response dropped with its body in it
    // ends the download of that body.
    body: Option<FlUrlBodyReader>,
}

impl Debug for FlUrlResponse {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FlUrlResponse")
            .field("url", &self.url.to_string())
            .field("status_code", &self.status_code)
            .finish()
    }
}

impl FlUrlResponse {
    /// `controller` is the request's `AbortController`: the reader of the body bounds
    /// the read with it when `body_timeout_millis` (`set_response_body_timeout`) is
    /// set, and ends the download with it when the body is not read to its end.
    pub(crate) fn new(
        url: UrlBuilder,
        response: web_sys::Response,
        controller: Option<web_sys::AbortController>,
        body_timeout_millis: Option<i32>,
    ) -> Self {
        let status_code = response.status();
        let headers = collect_headers(&response.headers());
        Self {
            url,
            status_code,
            headers,
            body: Some(FlUrlBodyReader::of_response(
                response,
                controller,
                body_timeout_millis,
            )),
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
            .ok_or_else(|| FlUrlError::FetchError(BODY_IS_TAKEN.to_string()))
    }

    pub fn get_status_code(&self) -> u16 {
        self.status_code
    }

    pub fn get_header(&self, name: &str) -> Result<Option<&str>, FlUrlReadingHeaderError> {
        // The browser lower-cases response header names, and the native backend's
        // `get_header` is case-insensitive too (hyper normalizes header names), so
        // we match case-insensitively to keep the same call portable across
        // targets.
        self.get_header_case_insensitive(name)
    }

    pub fn get_header_case_insensitive(
        &self,
        name: &str,
    ) -> Result<Option<&str>, FlUrlReadingHeaderError> {
        Ok(self
            .headers
            .iter()
            .find(|(header_name, _)| header_name.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str()))
    }

    pub fn get_headers(&self) -> HashMap<&str, Option<&str>> {
        self.headers
            .iter()
            .map(|(name, value)| (name.as_str(), Some(value.as_str())))
            .collect()
    }

    pub fn fill_headers_to_hashmap_of_string(&self, dest: &mut HashMap<String, Option<String>>) {
        for (name, value) in &self.headers {
            dest.insert(name.clone(), Some(value.clone()));
        }
    }

    /// Always `false` under wasm — kept for API parity. The browser owns the
    /// connection, so FlUrl never decides to drop it.
    pub fn drop_connection(&self) -> bool {
        false
    }

}

/// The status code and the headers: `StatusCode: 200; Headers: name='value';...`.
impl std::fmt::Display for FlUrlResponse {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "StatusCode: {}; Headers: ", self.status_code)?;

        for (name, value) in &self.headers {
            write!(f, "{}='{}';", name, value)?;
        }

        Ok(())
    }
}
