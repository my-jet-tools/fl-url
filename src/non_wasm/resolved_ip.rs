//! `scheme://<server-name>@<ip>[:port]/path` — a url that names both the host the
//! request is for and the address to reach it at, so no DNS lookup happens.
//!
//! Only the ip is taken out of the url. The server name stays its host, so it is
//! still what goes into the Host header, h2's `:authority` and the TLS SNI /
//! certificate check; the ip is used solely to open the socket.

use std::borrow::Cow;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

use rust_extensions::remote_endpoint::Scheme;

use crate::FlUrlError;

/// Splits `https://domain.com@15.0.0.5:8443/path` into `https://domain.com:8443/path`
/// and `15.0.0.5`. A url with no '@' in its authority comes back unchanged, with
/// no ip.
pub(crate) fn extract_resolved_ip(url: &str) -> Result<(Cow<'_, str>, Option<IpAddr>), FlUrlError> {
    let authority_start = match url.find("://") {
        Some(index) => {
            // A unix-socket url carries a file path where the host would be, and a
            // file name may legally contain '@'.
            if let Some(scheme) = Scheme::try_parse(&url[..index]) {
                if scheme.is_unix_socket() {
                    return Ok((Cow::Borrowed(url), None));
                }
            }
            index + 3
        }
        None => 0,
    };

    let rest = &url[authority_start..];
    // '@' further on belongs to the path or the query, not to the authority.
    let authority_len = rest
        .find(['/', '?', '#'])
        .unwrap_or(rest.len());
    let authority = &rest[..authority_len];

    let Some((server_name, ip_and_port)) = authority.split_once('@') else {
        return Ok((Cow::Borrowed(url), None));
    };

    if server_name.is_empty() {
        return Err(invalid_url(url, "the server name before '@' is empty"));
    }

    if server_name.contains(':') {
        return Err(invalid_url(
            url,
            "the part before '@' must be a bare server name; the port goes after the ip",
        ));
    }

    let Some((ip, port)) = parse_ip_and_port(ip_and_port) else {
        return Err(invalid_url(
            url,
            "expected an ip address after '@' (ipv6 in brackets), optionally followed by :port",
        ));
    };

    let mut result = String::with_capacity(url.len());
    result.push_str(&url[..authority_start]);
    result.push_str(server_name);
    result.push_str(port);
    result.push_str(&rest[authority_len..]);

    Ok((Cow::Owned(result), Some(ip)))
}

/// `15.0.0.5`, `15.0.0.5:8443`, `[::1]`, `[::1]:8443` → the ip and the `:port`
/// suffix as written (empty when there is none).
fn parse_ip_and_port(src: &str) -> Option<(IpAddr, &str)> {
    let (ip, port) = match src.strip_prefix('[') {
        Some(bracketed) => {
            let end = bracketed.find(']')?;
            let ip: Ipv6Addr = bracketed[..end].parse().ok()?;
            (IpAddr::V6(ip), &bracketed[end + 1..])
        }
        None => {
            let ip_len = src.find(':').unwrap_or(src.len());
            let ip: Ipv4Addr = src[..ip_len].parse().ok()?;
            (IpAddr::V4(ip), &src[ip_len..])
        }
    };

    if !port.is_empty() {
        port.strip_prefix(':')?.parse::<u16>().ok()?;
    }

    Some((ip, port))
}

fn invalid_url(url: &str, reason: &str) -> FlUrlError {
    FlUrlError::InvalidUrl(format!("Invalid url '{}': {}", url, reason))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn extract(url: &str) -> (String, Option<IpAddr>) {
        let (url, ip) = extract_resolved_ip(url).unwrap();
        (url.into_owned(), ip)
    }

    #[test]
    fn server_name_and_ipv4() {
        assert_eq!(
            extract("https://domain.com@15.0.0.5/xxx/fff"),
            (
                "https://domain.com/xxx/fff".to_string(),
                Some("15.0.0.5".parse().unwrap())
            )
        );
    }

    #[test]
    fn port_after_the_ip_stays_with_the_url() {
        assert_eq!(
            extract("https://domain.com@15.0.0.5:8443/xxx?a=b"),
            (
                "https://domain.com:8443/xxx?a=b".to_string(),
                Some("15.0.0.5".parse().unwrap())
            )
        );
    }

    #[test]
    fn ipv6_in_brackets() {
        assert_eq!(
            extract("https://domain.com@[2001:db8::1]:8443/xxx"),
            (
                "https://domain.com:8443/xxx".to_string(),
                Some("2001:db8::1".parse().unwrap())
            )
        );
        assert_eq!(
            extract("http://domain.com@[::1]"),
            ("http://domain.com".to_string(), Some("::1".parse().unwrap()))
        );
    }

    #[test]
    fn no_path_and_no_scheme() {
        assert_eq!(
            extract("https://domain.com@15.0.0.5"),
            (
                "https://domain.com".to_string(),
                Some("15.0.0.5".parse().unwrap())
            )
        );
        assert_eq!(
            extract("domain.com@15.0.0.5:8080/xxx"),
            (
                "domain.com:8080/xxx".to_string(),
                Some("15.0.0.5".parse().unwrap())
            )
        );
    }

    #[test]
    fn query_right_after_the_authority() {
        assert_eq!(
            extract("https://domain.com@15.0.0.5?email=a@b.com"),
            (
                "https://domain.com?email=a@b.com".to_string(),
                Some("15.0.0.5".parse().unwrap())
            )
        );
    }

    #[test]
    fn at_sign_outside_the_authority_is_left_alone() {
        for url in [
            "https://domain.com/users/@me",
            "https://domain.com?email=a@b.com",
            "https://domain.com:8443/a@b",
            "unix:///var/run/a@b.sock",
            "http+unix://var/run/a@b.sock",
            "/var/run/a@b.sock",
        ] {
            let (result, ip) = extract_resolved_ip(url).unwrap();
            assert!(matches!(result, Cow::Borrowed(_)), "{url}");
            assert_eq!(result, url);
            assert_eq!(ip, None, "{url}");
        }
    }

    #[test]
    fn invalid_forms_are_rejected() {
        for url in [
            "https://@15.0.0.5/xxx",
            "https://domain.com:443@15.0.0.5/xxx",
            "https://user:password@15.0.0.5/xxx",
            "https://domain.com@backend.internal/xxx",
            "https://domain.com@15.0.0.5:abc/xxx",
            "https://domain.com@15.0.0.5:99999/xxx",
            "https://domain.com@::1/xxx",
            "https://domain.com@[::1/xxx",
            "https://a@b@15.0.0.5/xxx",
        ] {
            assert!(
                matches!(extract_resolved_ip(url), Err(FlUrlError::InvalidUrl(_))),
                "{url} must be rejected"
            );
        }
    }
}
