/// wasm counterpart of the native `FlUrlHeaders`.
///
/// The native version is backed by `my-http-client`'s header builder; under wasm
/// we keep a plain `Vec` since headers are handed to the browser `Headers` object
/// at request time. The public methods match the native ones so request-building
/// code is portable.
pub struct FlUrlHeaders {
    headers: Vec<(String, String)>,
    pub has_connection_header: bool,
    pub len: usize,
    host_header_present: bool,
}

impl FlUrlHeaders {
    pub fn new() -> Self {
        Self {
            headers: Vec::new(),
            has_connection_header: false,
            len: 0,
            host_header_present: false,
        }
    }

    pub fn add(&mut self, name: &str, value: &str) {
        if name.eq_ignore_ascii_case("connection") {
            self.has_connection_header = true;
        }
        if name.eq_ignore_ascii_case("host") {
            self.host_header_present = true;
        }
        self.headers.push((name.to_string(), value.to_string()));
        self.len += 1;
    }

    pub fn has_host_header(&self) -> bool {
        self.host_header_present
    }

    pub fn has_header(&self, name: &str) -> bool {
        self.headers
            .iter()
            .any(|(header_name, _)| header_name.eq_ignore_ascii_case(name))
    }

    pub fn get_host_header_value(&self) -> Option<&str> {
        self.headers
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case("host"))
            .map(|(_, value)| value.as_str())
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn iter(&self) -> impl Iterator<Item = (&str, &str)> {
        self.headers
            .iter()
            .map(|(name, value)| (name.as_str(), value.as_str()))
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
