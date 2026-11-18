//! A url or a header with a byte that must not be put on the wire: a new line at
//! the end of a value read from a file, a space in a path.
//!
//! my-http-client refuses to put these into its request line and its header block —
//! it used to panic on them. Both the url and the header values come from settings as
//! often as from code, so the request has to fail with an error — in every mode.
#![cfg(not(target_arch = "wasm32"))]

use flurl::{FlUrl, FlUrlError, FlUrlMode, HttpVerb};
use my_http_client::RequestBodyStream;
use my_http_utils::macros::MyHttpInput;

const MODES: [FlUrlMode; 3] = [
    FlUrlMode::Http1Hyper,
    FlUrlMode::Http1NoHyper,
    FlUrlMode::H2,
];

// Nothing listens here, and nothing has to: every request below is refused while it
// is being built.
const URL: &str = "http://127.0.0.1:1";

#[tokio::test]
async fn a_new_line_in_the_host_fails_the_request() {
    for url in [
        "http://127.0.0.1:1\n",
        "http://127.0.0.1:1\r\n",
        "127.0.0.1:1\n",
        "http://127.0.0.1\n:1",
    ] {
        for mode in MODES {
            let result = FlUrl::new(url)
                .update_mode(mode)
                .append_path_segment("api")
                .get()
                .await;

            assert!(result.is_err(), "{url:?} {mode:?}");
        }
    }
}

#[tokio::test]
async fn a_forbidden_byte_in_the_path_fails_the_request() {
    for url in [
        "http://127.0.0.1:1/a b",
        "http://127.0.0.1:1/a\nb",
        "http://127.0.0.1:1/p?a=b c",
        "http://127.0.0.1:1/p\0",
    ] {
        for mode in MODES {
            let result = FlUrl::new(url).update_mode(mode).get().await;

            assert!(result.is_err(), "{url:?} {mode:?}");
        }
    }
}

#[tokio::test]
async fn a_raw_ending_with_a_space_fails_the_request() {
    for mode in MODES {
        let result = FlUrl::new(URL)
            .update_mode(mode)
            .append_raw_ending_to_url("/a b?c=d")
            .get()
            .await;

        assert!(result.is_err(), "{mode:?}");
    }
}

// The own http/1.1 implementation is the one that used to panic, so it is the one
// whose error is pinned down.
#[tokio::test]
async fn the_own_http1_mode_names_the_url_and_the_byte() {
    let err = FlUrl::new("http://127.0.0.1:1\n")
        .update_mode(FlUrlMode::Http1NoHyper)
        .get()
        .await
        .expect_err("the request must be refused");

    match err {
        FlUrlError::InvalidUrl(message) => assert_eq!(
            message,
            "Invalid url 'http://127.0.0.1:1\\n': the host contains forbidden control byte 0x0a"
        ),
        other => panic!("unexpected error {other:?}"),
    }

    let err = FlUrl::new("http://127.0.0.1:1/a b")
        .update_mode(FlUrlMode::Http1NoHyper)
        .get()
        .await
        .expect_err("the request must be refused");

    match err {
        FlUrlError::InvalidUrl(message) => assert_eq!(
            message,
            "Invalid url 'http://127.0.0.1:1/a b': the path and query contain forbidden byte 0x20"
        ),
        other => panic!("unexpected error {other:?}"),
    }
}

#[tokio::test]
async fn a_header_value_with_a_new_line_fails_the_request() {
    for mode in MODES {
        let err = FlUrl::new(URL)
            .update_mode(mode)
            .with_header("Authorization", "Bearer token123\n")
            .get()
            .await
            .expect_err("the request must be refused");

        match err {
            FlUrlError::RequestBuild(message) => assert_eq!(
                message,
                "Value of header 'Authorization' contains forbidden control byte 0x0a",
                "{mode:?}"
            ),
            other => panic!("{mode:?}: unexpected error {other:?}"),
        }
    }
}

#[tokio::test]
async fn an_invalid_header_name_fails_the_request() {
    for name in ["", "X Name", "X-Name:"] {
        for mode in MODES {
            let err = FlUrl::new(URL)
                .update_mode(mode)
                .with_header(name, "value")
                .get()
                .await
                .expect_err("the request must be refused");

            assert!(
                matches!(err, FlUrlError::RequestBuild(_)),
                "{name:?} {mode:?}: {err:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_request_with_a_body_checks_its_headers_too() {
    for mode in MODES {
        let err = FlUrl::new(URL)
            .update_mode(mode)
            .with_header("X-Api-Key", "secret\r\n")
            .post(flurl::body::HttpRequestBody::as_json(&["payload"]))
            .await
            .expect_err("the request must be refused");

        assert!(
            matches!(err, FlUrlError::RequestBuild(_)),
            "{mode:?}: {err:?}"
        );
    }
}

#[tokio::test]
async fn a_streamed_request_checks_its_headers_too() {
    let (_publisher, body) = RequestBodyStream::<Vec<u8>>::new(4);

    let err = FlUrl::new(URL)
        .with_header("X-Api-Key", "secret\n")
        .post_request_streamed(body, None)
        .await
        .expect_err("the request must be refused");

    assert!(matches!(err, FlUrlError::RequestBuild(_)), "{err:?}");
}

#[derive(MyHttpInput)]
struct GetWithApiKey {
    #[http_header(name = "X-Api-Key", description = "Api key")]
    api_key: String,
}

// A header field of a request model goes through the same door as `with_header`.
#[tokio::test]
async fn a_model_header_with_a_new_line_fails_the_request() {
    for mode in MODES {
        let err = FlUrl::new(URL)
            .update_mode(mode)
            .execute_request(
                HttpVerb::Get,
                GetWithApiKey {
                    api_key: "secret\n".to_string(),
                },
            )
            .await
            .expect_err("the request must be refused");

        assert!(
            matches!(err, FlUrlError::RequestBuild(_)),
            "{mode:?}: {err:?}"
        );
    }
}
