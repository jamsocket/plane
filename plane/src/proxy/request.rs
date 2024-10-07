use crate::types::{BearerToken, ClusterName};
use dynamic_proxy::hyper::http::uri::{self, PathAndQuery};
use std::str::FromStr;

// If a cluster name does not specify a port, :443 is implied.
// Most browsers will not specify it, but some (e.g. the `ws` websocket client in Node.js)
// will, so we strip it.
const HTTPS_PORT_SUFFIX: &str = ":443";

/// Returns Ok(Some(subdomain)) if a subdomain is found.
/// Returns Ok(None) if no subdomain is found, but the host header matches the cluster name.
/// Returns Err(()) if the host header does not
/// match the cluster name.
pub fn subdomain_from_host<'a>(
    host: &'a str,
    cluster: &ClusterName,
) -> Result<Option<&'a str>, ()> {
    let host = if let Some(host) = host.strip_suffix(HTTPS_PORT_SUFFIX) {
        host
    } else {
        host
    };

    if let Some(subdomain) = host.strip_suffix(cluster.as_str()) {
        if subdomain.is_empty() {
            // Subdomain exactly matches cluster name.
            Ok(None)
        } else if let Some(subdomain) = subdomain.strip_suffix('.') {
            Ok(Some(subdomain))
        } else {
            Err(())
        }
    } else {
        tracing::warn!(host, "Host header does not end in cluster name.");
        Err(())
    }
}

/// Removes a connection string from the URI and returns it.
/// If no connection string is found, returns None.
pub fn get_and_maybe_remove_bearer_token(parts: &mut uri::Parts) -> Option<BearerToken> {
    let path_and_query = parts.path_and_query.clone()?;

    let full_path = path_and_query.path().strip_prefix('/')?;

    // Split the incoming path into the token and the path to proxy to. If there is no slash, the token is
    // the full incoming path, and the path to proxy to is just `/`.
    let (token, path) = match full_path.split_once('/') {
        Some((token, path)) => (token, path),
        None => (full_path, ""),
    };

    if token.is_empty() {
        return None;
    }

    let token = BearerToken::from(token.to_string());

    if token.is_static() {
        // We don't rewrite the URL if using a static token.
        return Some(token);
    }

    let query = path_and_query
        .query()
        .map(|query| format!("?{}", query))
        .unwrap_or_default();

    parts.path_and_query = Some(
        PathAndQuery::from_str(format!("/{}{}", path, query).as_str())
            .expect("Path and query is valid."),
    );

    Some(token)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;
    use uri::Uri;

    #[test]
    fn no_subdomains() {
        let host = "foo.bar.baz";
        let cluster = ClusterName::from_str("foo.bar.baz").unwrap();
        assert_eq!(subdomain_from_host(host, &cluster), Ok(None));
    }

    #[test]
    fn valid_subdomain() {
        let host = "foobar.example.com";
        let cluster = ClusterName::from_str("example.com").unwrap();
        assert_eq!(subdomain_from_host(host, &cluster), Ok(Some("foobar")));
    }

    #[test]
    fn valid_suffix_no_dot() {
        let host = "foobarexample.com";
        let cluster = ClusterName::from_str("example.com").unwrap();
        assert_eq!(subdomain_from_host(host, &cluster), Err(()));
    }

    #[test]
    fn invalid_suffix() {
        let host = "abc.abc.com";
        let cluster = ClusterName::from_str("example.com").unwrap();
        assert_eq!(subdomain_from_host(host, &cluster), Err(()));
    }

    #[test]
    fn allowed_port() {
        let host = "foobar.myhost:8080";
        let cluster = ClusterName::from_str("myhost:8080").unwrap();
        assert_eq!(subdomain_from_host(host, &cluster), Ok(Some("foobar")));
    }

    #[test]
    fn port_required() {
        let host = "foobar.myhost";
        let cluster = ClusterName::from_str("myhost:8080").unwrap();
        assert_eq!(subdomain_from_host(host, &cluster), Err(()));
    }

    #[test]
    fn port_must_match() {
        let host = "foobar.myhost:8080";
        let cluster = ClusterName::from_str("myhost").unwrap();
        assert_eq!(subdomain_from_host(host, &cluster), Err(()));
    }

    #[test]
    fn port_443_optional() {
        let host = "foobar.myhost:443";
        let cluster = ClusterName::from_str("myhost").unwrap();
        assert_eq!(subdomain_from_host(host, &cluster), Ok(Some("foobar")));
    }

    #[test]
    fn test_get_and_maybe_remove_bearer_token() {
        let url = Uri::from_str("https://example.com/foo/bar").unwrap();
        let mut parts = url.into_parts();
        assert_eq!(
            get_and_maybe_remove_bearer_token(&mut parts),
            Some(BearerToken::from("foo".to_string()))
        );
        assert_eq!(
            parts.path_and_query,
            Some(PathAndQuery::from_str("/bar").unwrap())
        );
    }

    #[test]
    fn test_get_and_maybe_remove_bearer_token_ends_no_slash() {
        let url = Uri::from_str("https://example.com/foo").unwrap();
        let mut parts = url.into_parts();
        assert_eq!(
            get_and_maybe_remove_bearer_token(&mut parts),
            Some(BearerToken::from("foo".to_string()))
        );
        assert_eq!(
            parts.path_and_query,
            Some(PathAndQuery::from_str("/").unwrap())
        );
    }

    #[test]
    fn test_get_and_maybe_remove_bearer_token_ends_in_slash() {
        let url = Uri::from_str("https://example.com/foo/").unwrap();
        let mut parts = url.into_parts();
        assert_eq!(
            get_and_maybe_remove_bearer_token(&mut parts),
            Some(BearerToken::from("foo".to_string()))
        );
        assert_eq!(
            parts.path_and_query,
            Some(PathAndQuery::from_str("/").unwrap())
        );
    }

    #[test]
    fn test_get_and_maybe_remove_bearer_token_no_token() {
        let url = Uri::from_str("https://example.com/").unwrap();
        let mut parts = url.into_parts();
        assert_eq!(get_and_maybe_remove_bearer_token(&mut parts), None);
        assert_eq!(
            parts.path_and_query,
            Some(PathAndQuery::from_str("/").unwrap())
        );
    }

    #[test]
    fn test_get_and_maybe_remove_bearer_token_static_token() {
        let url = Uri::from_str("https://example.com/s.foo/bar").unwrap();
        let mut parts = url.into_parts();
        assert_eq!(
            get_and_maybe_remove_bearer_token(&mut parts),
            Some(BearerToken::from("s.foo".to_string()))
        );
        assert_eq!(
            parts.path_and_query,
            Some(PathAndQuery::from_str("/s.foo/bar").unwrap())
        );
    }
}
