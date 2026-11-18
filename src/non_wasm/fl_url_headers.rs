use my_http_client::{HeaderValuePosition, MyHttpClientHeadersBuilder, RequestBuildError};

use crate::FlUrlError;

pub struct FlUrlHeaders {
    pub(crate) headers: MyHttpClientHeadersBuilder,
    pub has_connection_header: bool,
    pub len: usize,
    pub host_header_value: Option<HeaderValuePosition>,
    // The first header that can not be sent. `add` has no way to report it, so it is
    // kept here and fails the request instead — see `ensure_valid`.
    invalid_header: Option<String>,
}

impl FlUrlHeaders {
    pub fn new() -> Self {
        Self {
            headers: MyHttpClientHeadersBuilder::new(),

            has_connection_header: false,
            host_header_value: None,
            len: 0,
            invalid_header: None,
        }
    }

    /*
       pub fn add_json_content_type(&mut self) {
           self.headers
               .add_header(CONTENT_TYPE.as_str(), "application/json");
       }
    */
    pub fn add(&mut self, name: &str, value: &str) {
        // What must not be put on the wire is my-http-client's to decide: its header
        // builder refuses such a header and adds nothing. A header value comes from
        // settings as often as from code — an api key read together with its trailing
        // new line — so the refusal is kept, and it is the request that fails with it.
        let pos = match self.headers.add_header(name, value) {
            Ok(pos) => pos,
            Err(err) => {
                if self.invalid_header.is_none() {
                    self.invalid_header = Some(describe_refused_header(name, &err));
                }
                return;
            }
        };

        if rust_extensions::str_utils::compare_strings_case_insensitive(name, "connection") {
            self.has_connection_header = true;
        }

        if name.eq_ignore_ascii_case("host") {
            self.host_header_value = Some(pos);
        }
        self.len += 1;
    }

    /// Adds `value` to the list header `name` (`Content-Encoding`). The result is one
    /// line: `name: <what was there>, <value>`, or a new header when there was none.
    /// Two lines would mean the same thing to HTTP, but a server that reads only the
    /// first line of a header would see half the list. Codings go in the order they
    /// were applied, so a coding applied after the ones already listed goes last.
    ///
    /// The builder can only append, so when the header is already there the headers
    /// are put together again, in the same order.
    #[cfg(feature = "encryption")]
    pub(crate) fn append_to_list_header(&mut self, name: &str, value: &str) {
        if !self.has_header(name) {
            self.add(name, value);
            return;
        }

        let headers: Vec<(String, String)> = self
            .iter()
            .map(|(name, value)| (name.to_string(), value.to_string()))
            .collect();

        // Every line of the header is joined into the first one, the new value after
        // them all.
        let mut combined = headers
            .iter()
            .filter(|(header_name, _)| header_name.eq_ignore_ascii_case(name))
            .map(|(_, header_value)| header_value.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        combined.push_str(", ");
        combined.push_str(value);

        let invalid_header = self.invalid_header.take();
        *self = Self::new();
        self.invalid_header = invalid_header;

        let mut combined = Some(combined);

        for (header_name, header_value) in headers {
            if !header_name.eq_ignore_ascii_case(name) {
                self.add(&header_name, &header_value);
                continue;
            }

            if let Some(combined) = combined.take() {
                self.add(&header_name, &combined);
            }
        }
    }

    /// Fails with the first header [`Self::add`] was given that can not be sent.
    /// Called right before the headers go into a request.
    pub(crate) fn ensure_valid(&self) -> Result<(), FlUrlError> {
        match self.invalid_header.as_ref() {
            Some(reason) => Err(FlUrlError::RequestBuild(reason.clone())),
            None => Ok(()),
        }
    }

    pub fn has_host_header(&self) -> bool {
        self.host_header_value.is_some()
    }

    pub fn has_header(&self, name: &str) -> bool {
        self.headers
            .iter()
            .any(|(header_name, _)| header_name.eq_ignore_ascii_case(name))
    }

    pub fn get_host_header_value(&self) -> Option<&str> {
        let host_value_pos = self.host_header_value.as_ref()?;
        self.headers.get_value(host_value_pos)
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn iter(&self) -> impl Iterator<Item = (&str, &str)> {
        self.headers.iter()
    }

    pub fn get_builder(&self) -> &MyHttpClientHeadersBuilder {
        &self.headers
    }
}

impl Default for FlUrlHeaders {
    fn default() -> Self {
        Self::new()
    }
}

/// Lets a `my_http_utils` request model (`#[derive(MyHttpInput)]`) push its header
/// fields straight into our header collection during `execute_request`.
impl my_http_utils::schema::client::HeaderBuilder for FlUrlHeaders {
    fn add_header(&mut self, name: &str, value: &str) {
        self.add(name, value);
    }
}

/// What the request fails with for a header my-http-client refused. The message names
/// the header and the byte, never the value: that is where the secrets are.
fn describe_refused_header(name: &str, err: &RequestBuildError) -> String {
    match err {
        RequestBuildError::HeaderNameIsEmpty => "Header name must not be empty".to_string(),
        RequestBuildError::ForbiddenByteInHeaderName(byte) => format!(
            "Header name '{}' contains forbidden byte 0x{:02x}",
            name.escape_debug(),
            byte
        ),
        RequestBuildError::ForbiddenByteInHeaderValue(byte) => format!(
            "Value of header '{}' contains forbidden control byte 0x{:02x}",
            name, byte
        ),
        other => format!("Header '{}' can not be sent: {}", name.escape_debug(), other),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_valid_header_is_kept() {
        let mut headers = FlUrlHeaders::new();
        headers.add("Authorization", "Bearer token123");
        headers.add("X-Name", "значение\twith a tab");

        assert!(headers.ensure_valid().is_ok());
        assert_eq!(headers.len(), 2);
        assert!(headers.has_header("authorization"));
    }

    #[test]
    fn a_value_with_a_new_line_fails_the_request_and_is_not_added() {
        let mut headers = FlUrlHeaders::new();
        headers.add("Authorization", "Bearer token123\n");

        assert_eq!(headers.len(), 0);
        assert!(!headers.has_header("Authorization"));

        match headers.ensure_valid() {
            Err(FlUrlError::RequestBuild(reason)) => {
                assert_eq!(
                    reason,
                    "Value of header 'Authorization' contains forbidden control byte 0x0a"
                );
            }
            other => panic!("unexpected result {other:?}"),
        }
    }

    #[test]
    fn the_message_does_not_carry_the_value() {
        let mut headers = FlUrlHeaders::new();
        headers.add("Authorization", "Bearer secret\r\n");

        let reason = format!("{:?}", headers.ensure_valid().unwrap_err());
        assert!(!reason.contains("secret"), "{reason}");
    }

    #[test]
    fn an_invalid_name_fails_the_request() {
        for name in ["", "X Name", "X-Name:", "X-Имя", "X\nName"] {
            let mut headers = FlUrlHeaders::new();
            headers.add(name, "value");

            assert!(
                matches!(headers.ensure_valid(), Err(FlUrlError::RequestBuild(_))),
                "{name:?}"
            );
        }
    }

    #[test]
    fn the_first_invalid_header_is_the_one_reported() {
        let mut headers = FlUrlHeaders::new();
        headers.add("First", "a\nb");
        headers.add("Second", "a\rb");
        headers.add("Third", "fine");

        let reason = format!("{:?}", headers.ensure_valid().unwrap_err());
        assert!(reason.contains("'First'"), "{reason}");
        assert_eq!(headers.len(), 1);
    }

    #[cfg(feature = "encryption")]
    fn lines(headers: &FlUrlHeaders) -> Vec<(String, String)> {
        headers
            .iter()
            .map(|(name, value)| (name.to_string(), value.to_string()))
            .collect()
    }

    #[cfg(feature = "encryption")]
    #[test]
    fn a_list_header_that_is_not_there_is_added() {
        let mut headers = FlUrlHeaders::new();
        headers.add("X-Api-Key", "key");
        headers.append_to_list_header("Content-Encoding", "aes128gcm");

        assert_eq!(
            lines(&headers),
            [
                ("X-Api-Key".to_string(), "key".to_string()),
                ("Content-Encoding".to_string(), "aes128gcm".to_string()),
            ]
        );
    }

    /// The value goes last, on the line that was there, and every other header keeps
    /// its place and what it marks (the Host header, the Connection header).
    #[cfg(feature = "encryption")]
    #[test]
    fn a_list_header_that_is_there_gets_the_value_on_the_same_line() {
        let mut headers = FlUrlHeaders::new();
        headers.add("Host", "example.com");
        headers.add("content-encoding", "br");
        headers.add("Connection", "close");
        headers.add("Content-Encoding", "gzip");
        headers.append_to_list_header("Content-Encoding", "aes128gcm");

        assert_eq!(
            lines(&headers),
            [
                ("Host".to_string(), "example.com".to_string()),
                (
                    "content-encoding".to_string(),
                    "br, gzip, aes128gcm".to_string()
                ),
                ("Connection".to_string(), "close".to_string()),
            ]
        );
        assert_eq!(headers.len(), 3);
        assert_eq!(headers.get_host_header_value(), Some("example.com"));
        assert!(headers.has_connection_header);
    }

    /// A header that could not be added still fails the request after the headers are
    /// put together again.
    #[cfg(feature = "encryption")]
    #[test]
    fn a_refused_header_is_still_reported_after_the_list_header_is_joined() {
        let mut headers = FlUrlHeaders::new();
        headers.add("Content-Encoding", "gzip");
        headers.add("Authorization", "Bearer secret\n");
        headers.append_to_list_header("Content-Encoding", "aes128gcm");

        assert!(matches!(
            headers.ensure_valid(),
            Err(FlUrlError::RequestBuild(_))
        ));
    }
}
