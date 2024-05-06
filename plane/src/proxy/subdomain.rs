use super::rewriter::RequestRewriterError;
use crate::types::ClusterName;

// If a cluster name does not specify a port, :443 is implied.
// Most browsers will not specify it, but some (e.g. the `ws` websocket client in Node.js)
// will, so we strip it.
const HTTPS_PORT_SUFFIX: &str = ":443";

/// Returns Ok(Some(subdomain)) if a subdomain is found.
/// Returns Ok(None) if no subdomain is found, but the host header matches the cluster name.
/// Returns Err(RequestRewriterError::InvalidHostHeader) if the host header does not
/// match the cluster name.
pub fn subdomain_from_host<'a>(
    host: &'a str,
    cluster: &ClusterName,
) -> Result<Option<&'a str>, RequestRewriterError> {
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
            Err(RequestRewriterError::InvalidHostHeader)
        }
    } else {
        tracing::warn!(host, "Host header does not end in cluster name.");
        Err(RequestRewriterError::InvalidHostHeader)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

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
        assert_eq!(
            subdomain_from_host(host, &cluster),
            Err(RequestRewriterError::InvalidHostHeader)
        );
    }

    #[test]
    fn invalid_suffix() {
        let host = "abc.abc.com";
        let cluster = ClusterName::from_str("example.com").unwrap();
        assert_eq!(
            subdomain_from_host(host, &cluster),
            Err(RequestRewriterError::InvalidHostHeader)
        );
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
        assert_eq!(
            subdomain_from_host(host, &cluster),
            Err(RequestRewriterError::InvalidHostHeader)
        );
    }

    #[test]
    fn port_must_match() {
        let host = "foobar.myhost:8080";
        let cluster = ClusterName::from_str("myhost").unwrap();
        assert_eq!(
            subdomain_from_host(host, &cluster),
            Err(RequestRewriterError::InvalidHostHeader)
        );
    }

    #[test]
    fn port_443_optional() {
        let host = "foobar.myhost:443";
        let cluster = ClusterName::from_str("myhost").unwrap();
        assert_eq!(subdomain_from_host(host, &cluster), Ok(Some("foobar")));
    }
}
