use plane_client::types::ClusterName;
use std::str::FromStr;

/// Maps a requested domain name to a cluster name.
///
/// This can be configured to operate in two ways.
///
/// If the `cname_zone` is `None`, the server assumes that it is the authoritative
/// DNS server for the cluster. It expects requests of the form
/// _acme-challenge.<cluster name>, and will strip the _acme-challenge prefix.
///
/// If the `cname_zone` is `Some(_)`, the server will expect requests of the form
/// <cluster name>.<cname_zone>, and will strip the <cname_zone> suffix.
///
/// Note that the _acme-challenge. prefix is not expected in the case where
/// `cname_zone` is `Some(_)`. The _acme-challenge record should be pointed
/// directly to the <cluster name>.<cname_zone> record via a CNAME record.
///
/// In production, the `cname_zone` should always be set (and the CLI enforces
/// this), but for testing it is useful to be able to run without a `cnbame_zone`
pub struct NameToCluster {
    cname_zone: Option<String>,
}

impl NameToCluster {
    pub fn new(cname_zone: Option<String>) -> Self {
        Self { cname_zone }
    }

    pub fn cluster_name(&self, name: &str) -> Option<ClusterName> {
        let name = name.strip_suffix('.').unwrap_or(name);

        match &self.cname_zone {
            Some(cname_zone) => {
                let name = name.strip_suffix(cname_zone)?;
                let name = name.strip_suffix('.').unwrap_or(name);
                ClusterName::from_str(name).ok()
            }
            None => {
                let name = name.strip_prefix("_acme-challenge.")?;
                ClusterName::from_str(name).ok()
            }
        }
    }
}

#[cfg(test)]
mod test {
    use plane_client::types::ClusterName;
    use std::str::FromStr;

    #[test]
    fn test_no_cname_zone() {
        let name_to_cluster = super::NameToCluster::new(None);

        assert_eq!(
            name_to_cluster.cluster_name("foo.bar.baz"),
            None,
            "No cluster name should be returned for a domain without a _acme-challenge prefix."
        );

        assert_eq!(
            name_to_cluster.cluster_name("_acme-challenge.foo.bar.baz"),
            Some(ClusterName::from_str("foo.bar.baz").unwrap())
        );

        assert_eq!(
            name_to_cluster.cluster_name("_acme-challenge.foo.bar.baz."),
            Some(ClusterName::from_str("foo.bar.baz").unwrap())
        );
    }

    #[test]
    fn test_cname_zone() {
        let name_to_cluster = super::NameToCluster::new(Some("example.com".to_string()));

        assert_eq!(
            name_to_cluster.cluster_name("foo.bar.baz"),
            None,
            "No match for a domain that lacks the cname zone."
        );

        assert_eq!(
            name_to_cluster.cluster_name("foo.bar.baz.example.com"),
            Some(ClusterName::from_str("foo.bar.baz").unwrap())
        );

        assert_eq!(
            name_to_cluster.cluster_name("foo.bar.baz.example.com."),
            Some(ClusterName::from_str("foo.bar.baz").unwrap())
        );
    }
}
