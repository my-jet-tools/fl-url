mod my_http_client_wrapper_inner;
pub use my_http_client_wrapper_inner::*;
// The wrapper type lives in a file of its own name, next to its inner state.
#[allow(clippy::module_inception)]
mod my_http_client_wrapper;
pub use my_http_client_wrapper::*;
