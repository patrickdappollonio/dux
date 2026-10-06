//! The bearer token a command-line client sends in `Authorization`. A browser
//! never sets this header on its own, so a token here was put there on purpose.

use axum::http::HeaderMap;

/// Every token presented as `Authorization: Bearer <token>`, in the order the
/// headers came. Parsed from the raw bytes, one header at a time (as the
/// cookie is): a header that is not ASCII is skipped on its own and never hides
/// another. The scheme's case does not matter.
pub(crate) fn read_all(headers: &HeaderMap) -> Vec<String> {
    headers
        .get_all(axum::http::header::AUTHORIZATION)
        .iter()
        .map(|value| value.as_bytes().trim_ascii())
        .filter(|value| value.is_ascii())
        .filter_map(|value| std::str::from_utf8(value).ok())
        .filter_map(|value| {
            let (scheme, token) = value.split_once(' ')?;
            scheme
                .eq_ignore_ascii_case("bearer")
                .then(|| token.trim().to_string())
        })
        .filter(|token| !token.is_empty())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    fn headers(values: &[&[u8]]) -> HeaderMap {
        let mut map = HeaderMap::new();
        for value in values {
            map.append(
                axum::http::header::AUTHORIZATION,
                HeaderValue::from_bytes(value).unwrap(),
            );
        }
        map
    }

    #[test]
    fn only_a_bearer_scheme_with_a_token_is_read_and_every_such_header_counts() {
        let map = headers(&[
            b"Bearer first",
            b"bearer   second ",
            b"Basic dXNlcjpwYXNz",
            b"Bearer",
            b"Bearer ",
            b"Bearer caf\xc3\xa9",
            b"BEARER third",
        ]);
        assert_eq!(read_all(&map), vec!["first", "second", "third"]);
        assert!(read_all(&HeaderMap::new()).is_empty());
    }
}
