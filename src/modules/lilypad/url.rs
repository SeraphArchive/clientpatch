//! Pure URL/origin rewrite helpers for the lilypad API redirect.

/// Trim a base/origin to scheme+authority with no trailing slash.
pub fn origin(base: &str) -> &str {
    base.trim_end_matches('/')
}

/// Join an api_base ("http://host:port/") with a request path-and-query
/// ("/api/...") into a full URL, avoiding a doubled or missing slash.
pub fn rebuild_url(api_base: &str, path_and_query: &str) -> String {
    let base = origin(api_base);
    if path_and_query.is_empty() {
        format!("{base}/")
    } else if path_and_query.starts_with('/') {
        format!("{base}{path_and_query}")
    } else {
        format!("{base}/{path_and_query}")
    }
}

/// The official game-API origins. A request under any of these is rewritten onto
/// the configured LilyPad origin; everything else is left alone.
pub const OFFICIAL_API_ORIGINS: [&str; 3] = [
    "https://api.heaven-burns-red.wfs.games",
    "https://api-ap.heaven-burns-red.wfs.games",
    "https://api-kr.heaven-burns-red.wfs.games",
];

/// If `input` is under an official API origin, return it rebuilt onto
/// `replacement_origin` (path + query preserved); otherwise `None`. The boundary
/// check (remainder empty or starting with `/`) rejects look-alike hosts such as
/// `api.heaven-burns-red.wfs.games.evil.test`.
pub fn rewrite_official_api_url(input: &str, replacement_origin: &str) -> Option<String> {
    for official in OFFICIAL_API_ORIGINS {
        if let Some(path_and_query) = input.strip_prefix(official) {
            if path_and_query.is_empty() || path_and_query.starts_with('/') {
                return Some(rebuild_url(replacement_origin, path_and_query));
            }
        }
    }
    None
}

/// True if `input` is already under our LilyPad origin (scheme+authority).
/// Used to mark the redirect LIVE when the game never hit an official host
/// (BaseUri was overwritten before the first request).
pub fn is_our_origin(input: &str, origin: &str) -> bool {
    let origin = origin.trim_end_matches('/');
    if origin.is_empty() {
        return false;
    }
    input == origin
        || input.starts_with(origin)
            && input.as_bytes().get(origin.len()) == Some(&b'/')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rebuild_joins_without_double_slash() {
        assert_eq!(
            rebuild_url("http://127.0.0.1:8443/", "/api/app/start"),
            "http://127.0.0.1:8443/api/app/start"
        );
        assert_eq!(
            rebuild_url("https://lily.example.com", "/api/user/pull?x=1"),
            "https://lily.example.com/api/user/pull?x=1"
        );
    }

    #[test]
    fn rebuild_handles_pathless_and_relative() {
        assert_eq!(rebuild_url("http://h/", "api/x"), "http://h/api/x");
        assert_eq!(rebuild_url("http://h", ""), "http://h/");
    }

    #[test]
    fn origin_strips_trailing_slashes() {
        assert_eq!(origin("http://127.0.0.1:8443/"), "http://127.0.0.1:8443");
        assert_eq!(origin("https://h"), "https://h");
    }

    #[test]
    fn rewrites_official_api_hosts() {
        assert_eq!(
            rewrite_official_api_url(
                "https://api.heaven-burns-red.wfs.games/api/app/start?x=1",
                "http://127.0.0.1:8443/"
            )
            .as_deref(),
            Some("http://127.0.0.1:8443/api/app/start?x=1")
        );
        assert_eq!(
            rewrite_official_api_url(
                "https://api-ap.heaven-burns-red.wfs.games/api/user/pull",
                "https://lily.example.com"
            )
            .as_deref(),
            Some("https://lily.example.com/api/user/pull")
        );
    }

    #[test]
    fn leaves_non_api_hosts_alone() {
        assert!(rewrite_official_api_url(
            "https://heaven-burns-red-assets.akamaized.net/prod/assets/x",
            "http://127.0.0.1:8443/"
        )
        .is_none());
        assert!(rewrite_official_api_url(
            "https://api.heaven-burns-red.wfs.games.evil.test/api/app/start",
            "http://127.0.0.1:8443/"
        )
        .is_none());
    }

    #[test]
    fn recognizes_our_origin() {
        assert!(is_our_origin(
            "http://127.0.0.1:8443/api/app/start",
            "http://127.0.0.1:8443/"
        ));
        assert!(is_our_origin("http://127.0.0.1:8443", "http://127.0.0.1:8443/"));
        assert!(!is_our_origin(
            "http://127.0.0.1:8443.evil.test/x",
            "http://127.0.0.1:8443"
        ));
        assert!(!is_our_origin(
            "https://api.heaven-burns-red.wfs.games/api/x",
            "http://127.0.0.1:8443"
        ));
    }
}
