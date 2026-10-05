//! The session cookie: `HttpOnly` (page scripts cannot read it),
//! `SameSite=Strict`, `Path=/`, host-only (no `Domain`), and `Secure` when
//! [`secure`] says so. Its name carries dux's listening port, because cookies
//! are not isolated by port and two dux servers on one machine must not sign
//! each other's browsers out.

use axum::http::{HeaderMap, HeaderValue};
use dux_core::config::CookieSecure;

/// The cookie's name for a dux listening on `port`.
pub(crate) fn name(port: u16) -> String {
    format!("dux_session_{port}")
}

/// Whether the cookie is marked `Secure`: per `cookie_secure`, and on `auto`
/// only when dux knows the browser reached it over HTTPS, which today is a
/// request through a confirmed `tailscale serve` HTTPS route. An arbitrary
/// `X-Forwarded-Proto` is never believed.
pub(crate) fn secure(setting: CookieSecure, https_serve_route: bool) -> bool {
    match setting {
        CookieSecure::Always => true,
        CookieSecure::Never => false,
        CookieSecure::Auto => https_serve_route,
    }
}

/// The `Set-Cookie` value that hands a browser its session.
pub(crate) fn set(port: u16, value: &str, secure: bool) -> HeaderValue {
    let text = format!(
        "{}={value}; Path=/; HttpOnly; SameSite=Strict{}",
        name(port),
        if secure { "; Secure" } else { "" }
    );
    HeaderValue::from_str(&text).expect("a token and a port are header-safe")
}

/// The `Set-Cookie` value that makes a browser forget its session: the same
/// name, path and flags, an empty value, and an expiry in the past.
pub(crate) fn clear(port: u16, secure: bool) -> HeaderValue {
    let text = format!(
        "{}=; Path=/; HttpOnly; SameSite=Strict; Max-Age=0; \
         Expires=Thu, 01 Jan 1970 00:00:00 GMT{}",
        name(port),
        if secure { "; Secure" } else { "" }
    );
    HeaderValue::from_str(&text).expect("header-safe")
}

/// Every non-empty value of the cookie named for `port` in a request, in the
/// order the browser sent them. The caller tries them all (decided, after
/// review): cookies are not isolated by port, so a page on another port of
/// the same host can plant one with this name and a narrower `Path`, which the
/// browser then sends FIRST, and reading only the first would hide the real
/// session behind it.
///
/// Parsed byte-wise, one pair at a time (decided, after review): another app
/// on the same host can set a cookie whose value is UTF-8, and reading the
/// header as text would throw away every pair with it, the session included.
/// A pair that is not ASCII is skipped on its own.
pub(crate) fn read_all(headers: &HeaderMap, port: u16) -> Vec<String> {
    let wanted = name(port);
    headers
        .get_all(axum::http::header::COOKIE)
        .iter()
        .flat_map(|value| value.as_bytes().split(|byte| *byte == b';'))
        .map(<[u8]>::trim_ascii)
        .filter(|pair| pair.is_ascii())
        .filter_map(|pair| std::str::from_utf8(pair).ok()?.split_once('='))
        .filter(|(key, value)| *key == wanted && !value.is_empty())
        .map(|(_, value)| value.to_string())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_cookie_is_host_only_http_only_strict_and_secure_only_when_known() {
        let set = set(3890, "abc", false);
        let text = set.to_str().unwrap();
        assert_eq!(
            text,
            "dux_session_3890=abc; Path=/; HttpOnly; SameSite=Strict"
        );
        assert!(!text.to_ascii_lowercase().contains("domain"));
        assert!(
            super::set(3890, "abc", true)
                .to_str()
                .unwrap()
                .ends_with("; Secure")
        );
        let clear = clear(3890, false);
        assert!(clear.to_str().unwrap().contains("Max-Age=0"));
        assert!(clear.to_str().unwrap().starts_with("dux_session_3890=;"));
    }

    #[test]
    fn secure_follows_the_setting_and_never_a_forwarded_header() {
        assert!(secure(CookieSecure::Always, false));
        assert!(!secure(CookieSecure::Never, true));
        assert!(secure(CookieSecure::Auto, true));
        assert!(!secure(CookieSecure::Auto, false));
    }

    #[test]
    fn the_cookie_is_read_by_its_own_port_name() {
        let mut headers = HeaderMap::new();
        headers.append(
            axum::http::header::COOKIE,
            "dux_session_4000=other; theme=dark".parse().unwrap(),
        );
        headers.append(
            axum::http::header::COOKIE,
            "dux_session_3890=; dux_session_3890=mine".parse().unwrap(),
        );
        assert_eq!(read_all(&headers, 3890), ["mine"]);
        assert_eq!(read_all(&headers, 4000), ["other"]);
        assert!(read_all(&headers, 5000).is_empty());
        headers.append(
            axum::http::header::COOKIE,
            "dux_session_3890=planted".parse().unwrap(),
        );
        assert_eq!(read_all(&headers, 3890), ["mine", "planted"]);
    }
}
