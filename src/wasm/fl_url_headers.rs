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

    /// Adds `value` to the list header `name`: the `Content-Encoding` a compressed body
    /// hands back. The result is one line, `name: <what was there>, <value>`, or a new
    /// header when there was none, the same as the native backend does it. Codings go
    /// in the order they were applied, so the caller's own come first: `br, gzip`.
    pub(crate) fn append_to_list_header(&mut self, name: &str, value: &str) {
        let Some(first) = self
            .headers
            .iter()
            .position(|(header_name, _)| header_name.eq_ignore_ascii_case(name))
        else {
            self.add(name, value);
            return;
        };

        // Every line of the header is joined into the first one, the new value after
        // them all.
        let mut combined = self
            .headers
            .iter()
            .filter(|(header_name, _)| header_name.eq_ignore_ascii_case(name))
            .map(|(_, header_value)| header_value.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        combined.push_str(", ");
        combined.push_str(value);

        self.headers[first].1 = combined;

        let mut index = 0;
        self.headers.retain(|(header_name, _)| {
            let keep = index <= first || !header_name.eq_ignore_ascii_case(name);
            index += 1;
            keep
        });
        self.len = self.headers.len();
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
