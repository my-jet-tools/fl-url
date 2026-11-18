use rust_extensions::StrOrString;

use crate::{body::HttpRequestBody, FlUrl, FlUrlError, FlUrlResponse};

/// wasm counterpart of the native `IntoFlUrl`. Identical method set; the async
/// methods use `#[async_trait(?Send)]` because browser `fetch` futures are not
/// `Send` (the browser is single-threaded). This does not change the call sites —
/// `"http://...".get().await` works the same on both targets.
#[async_trait::async_trait(?Send)]
pub trait IntoFlUrl {
    fn append_path_segment<'s>(self, path_segment: impl Into<StrOrString<'s>>) -> FlUrl;
    fn append_query_param<'n, 'v>(
        self,
        name: impl Into<StrOrString<'n>>,
        value: Option<impl Into<StrOrString<'v>>>,
    ) -> FlUrl;

    fn with_header<'n, 'v>(
        self,
        name: impl Into<StrOrString<'n>>,
        value: impl Into<StrOrString<'v>>,
    ) -> FlUrl;

    fn append_raw_ending_to_url<'s>(self, raw: impl Into<StrOrString<'s>>) -> FlUrl;

    async fn get(self) -> Result<FlUrlResponse, FlUrlError>;
    async fn post(self, body: HttpRequestBody) -> Result<FlUrlResponse, FlUrlError>;
    async fn put(self, body: HttpRequestBody) -> Result<FlUrlResponse, FlUrlError>;

    async fn delete(self) -> Result<FlUrlResponse, FlUrlError>;
    async fn head(self) -> Result<FlUrlResponse, FlUrlError>;

    // Each verb above, with the request dumped into `request_debug_string` —
    // same twins `FlUrl` itself carries.
    async fn get_with_debug(
        self,
        request_debug_string: &mut String,
    ) -> Result<FlUrlResponse, FlUrlError>;
    async fn post_with_debug(
        self,
        body: HttpRequestBody,
        request_debug_string: &mut String,
    ) -> Result<FlUrlResponse, FlUrlError>;
    async fn put_with_debug(
        self,
        body: HttpRequestBody,
        request_debug_string: &mut String,
    ) -> Result<FlUrlResponse, FlUrlError>;

    async fn delete_with_debug(
        self,
        request_debug_string: &mut String,
    ) -> Result<FlUrlResponse, FlUrlError>;
    async fn head_with_debug(
        self,
        request_debug_string: &mut String,
    ) -> Result<FlUrlResponse, FlUrlError>;
}

#[async_trait::async_trait(?Send)]
impl IntoFlUrl for &str {
    fn append_path_segment<'s>(self, path_segment: impl Into<StrOrString<'s>>) -> FlUrl {
        FlUrl::new(self).append_path_segment(path_segment)
    }

    fn append_query_param<'n, 'v>(
        self,
        name: impl Into<StrOrString<'n>>,
        value: Option<impl Into<StrOrString<'v>>>,
    ) -> FlUrl {
        FlUrl::new(self).append_query_param(name, value)
    }

    fn with_header<'n, 'v>(
        self,
        name: impl Into<StrOrString<'n>>,
        value: impl Into<StrOrString<'v>>,
    ) -> FlUrl {
        FlUrl::new(self).with_header(name, value)
    }

    fn append_raw_ending_to_url<'s>(self, raw: impl Into<StrOrString<'s>>) -> FlUrl {
        FlUrl::new(self).append_raw_ending_to_url(raw)
    }

    async fn get(self) -> Result<FlUrlResponse, FlUrlError> {
        FlUrl::new(self).get().await
    }

    async fn head(self) -> Result<FlUrlResponse, FlUrlError> {
        FlUrl::new(self).head().await
    }

    async fn post(self, body: HttpRequestBody) -> Result<FlUrlResponse, FlUrlError> {
        FlUrl::new(self).post(body).await
    }

    async fn put(self, body: HttpRequestBody) -> Result<FlUrlResponse, FlUrlError> {
        FlUrl::new(self).put(body).await
    }

    async fn delete(self) -> Result<FlUrlResponse, FlUrlError> {
        FlUrl::new(self).delete().await
    }

    async fn get_with_debug(
        self,
        request_debug_string: &mut String,
    ) -> Result<FlUrlResponse, FlUrlError> {
        FlUrl::new(self).get_with_debug(request_debug_string).await
    }

    async fn head_with_debug(
        self,
        request_debug_string: &mut String,
    ) -> Result<FlUrlResponse, FlUrlError> {
        FlUrl::new(self).head_with_debug(request_debug_string).await
    }

    async fn post_with_debug(
        self,
        body: HttpRequestBody,
        request_debug_string: &mut String,
    ) -> Result<FlUrlResponse, FlUrlError> {
        FlUrl::new(self)
            .post_with_debug(body, request_debug_string)
            .await
    }

    async fn put_with_debug(
        self,
        body: HttpRequestBody,
        request_debug_string: &mut String,
    ) -> Result<FlUrlResponse, FlUrlError> {
        FlUrl::new(self)
            .put_with_debug(body, request_debug_string)
            .await
    }

    async fn delete_with_debug(
        self,
        request_debug_string: &mut String,
    ) -> Result<FlUrlResponse, FlUrlError> {
        FlUrl::new(self)
            .delete_with_debug(request_debug_string)
            .await
    }
}

#[async_trait::async_trait(?Send)]
impl IntoFlUrl for &String {
    fn append_path_segment<'s>(self, path_segment: impl Into<StrOrString<'s>>) -> FlUrl {
        FlUrl::new(self).append_path_segment(path_segment)
    }

    fn append_query_param<'n, 'v>(
        self,
        name: impl Into<StrOrString<'n>>,
        value: Option<impl Into<StrOrString<'v>>>,
    ) -> FlUrl {
        FlUrl::new(self).append_query_param(name, value)
    }

    fn with_header<'n, 'v>(
        self,
        name: impl Into<StrOrString<'n>>,
        value: impl Into<StrOrString<'v>>,
    ) -> FlUrl {
        FlUrl::new(self).with_header(name, value)
    }

    fn append_raw_ending_to_url<'s>(self, raw: impl Into<StrOrString<'s>>) -> FlUrl {
        FlUrl::new(self).append_raw_ending_to_url(raw)
    }

    async fn get(self) -> Result<FlUrlResponse, FlUrlError> {
        FlUrl::new(self).get().await
    }

    async fn head(self) -> Result<FlUrlResponse, FlUrlError> {
        FlUrl::new(self).head().await
    }

    async fn post(self, body: HttpRequestBody) -> Result<FlUrlResponse, FlUrlError> {
        FlUrl::new(self).post(body).await
    }

    async fn put(self, body: HttpRequestBody) -> Result<FlUrlResponse, FlUrlError> {
        FlUrl::new(self).put(body).await
    }

    async fn delete(self) -> Result<FlUrlResponse, FlUrlError> {
        FlUrl::new(self).delete().await
    }

    async fn get_with_debug(
        self,
        request_debug_string: &mut String,
    ) -> Result<FlUrlResponse, FlUrlError> {
        FlUrl::new(self).get_with_debug(request_debug_string).await
    }

    async fn head_with_debug(
        self,
        request_debug_string: &mut String,
    ) -> Result<FlUrlResponse, FlUrlError> {
        FlUrl::new(self).head_with_debug(request_debug_string).await
    }

    async fn post_with_debug(
        self,
        body: HttpRequestBody,
        request_debug_string: &mut String,
    ) -> Result<FlUrlResponse, FlUrlError> {
        FlUrl::new(self)
            .post_with_debug(body, request_debug_string)
            .await
    }

    async fn put_with_debug(
        self,
        body: HttpRequestBody,
        request_debug_string: &mut String,
    ) -> Result<FlUrlResponse, FlUrlError> {
        FlUrl::new(self)
            .put_with_debug(body, request_debug_string)
            .await
    }

    async fn delete_with_debug(
        self,
        request_debug_string: &mut String,
    ) -> Result<FlUrlResponse, FlUrlError> {
        FlUrl::new(self)
            .delete_with_debug(request_debug_string)
            .await
    }
}

#[async_trait::async_trait(?Send)]
impl IntoFlUrl for String {
    fn append_path_segment<'s>(self, path_segment: impl Into<StrOrString<'s>>) -> FlUrl {
        FlUrl::new(self).append_path_segment(path_segment)
    }

    fn append_query_param<'n, 'v>(
        self,
        name: impl Into<StrOrString<'n>>,
        value: Option<impl Into<StrOrString<'v>>>,
    ) -> FlUrl {
        FlUrl::new(self).append_query_param(name, value)
    }

    fn with_header<'n, 'v>(
        self,
        name: impl Into<StrOrString<'n>>,
        value: impl Into<StrOrString<'v>>,
    ) -> FlUrl {
        FlUrl::new(self).with_header(name, value)
    }

    fn append_raw_ending_to_url<'s>(self, raw: impl Into<StrOrString<'s>>) -> FlUrl {
        FlUrl::new(self).append_raw_ending_to_url(raw)
    }

    async fn get(self) -> Result<FlUrlResponse, FlUrlError> {
        FlUrl::new(self).get().await
    }

    async fn head(self) -> Result<FlUrlResponse, FlUrlError> {
        FlUrl::new(self).head().await
    }

    async fn post(self, body: HttpRequestBody) -> Result<FlUrlResponse, FlUrlError> {
        FlUrl::new(self).post(body).await
    }

    async fn put(self, body: HttpRequestBody) -> Result<FlUrlResponse, FlUrlError> {
        FlUrl::new(self).put(body).await
    }

    async fn delete(self) -> Result<FlUrlResponse, FlUrlError> {
        FlUrl::new(self).delete().await
    }

    async fn get_with_debug(
        self,
        request_debug_string: &mut String,
    ) -> Result<FlUrlResponse, FlUrlError> {
        FlUrl::new(self).get_with_debug(request_debug_string).await
    }

    async fn head_with_debug(
        self,
        request_debug_string: &mut String,
    ) -> Result<FlUrlResponse, FlUrlError> {
        FlUrl::new(self).head_with_debug(request_debug_string).await
    }

    async fn post_with_debug(
        self,
        body: HttpRequestBody,
        request_debug_string: &mut String,
    ) -> Result<FlUrlResponse, FlUrlError> {
        FlUrl::new(self)
            .post_with_debug(body, request_debug_string)
            .await
    }

    async fn put_with_debug(
        self,
        body: HttpRequestBody,
        request_debug_string: &mut String,
    ) -> Result<FlUrlResponse, FlUrlError> {
        FlUrl::new(self)
            .put_with_debug(body, request_debug_string)
            .await
    }

    async fn delete_with_debug(
        self,
        request_debug_string: &mut String,
    ) -> Result<FlUrlResponse, FlUrlError> {
        FlUrl::new(self)
            .delete_with_debug(request_debug_string)
            .await
    }
}
