use crate::Ping;

impl Ping {
    pub fn version_matches(&self, req: &semver::VersionReq) -> bool {
        let Some(version) = self.version.as_ref().and_then(|v| v.parse().ok()) else {
            return false;
        };
        version_matches(&version, req)
    }
}

fn version_matches(version: &semver::Version, req: &semver::VersionReq) -> bool {
    if req.matches(version) {
        return true;
    }

    // This custom matching logic is needed, because semver cannot compare different version with pre-release tags
    let mut version_without_pre = version.clone();
    version_without_pre.pre = "".parse().unwrap();
    for comp in &req.comparators {
        if comp.matches(version) {
            continue;
        }

        // If major & minor & patch are the same, this means there is a mismatch on the pre-release tag
        if comp.major == version.major
            && comp.minor.is_some_and(|m| m == version.minor)
            && comp.patch.is_some_and(|p| p == version.patch)
        {
            return false;
        }

        // Otherwise, compare without pre-release tags
        let mut comp_without_pre = comp.clone();
        comp_without_pre.pre = "".parse().unwrap();
        if !comp_without_pre.matches(&version_without_pre) {
            return false;
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::version_matches;
    use semver::VersionReq;

    #[test]
    pub fn test_version_matches() {
        let req: VersionReq = ">=1.0.1-rc3, <2.0.0".parse().unwrap();

        assert!(version_matches(&"1.0.1-rc3".parse().unwrap(), &req));
        assert!(version_matches(&"1.0.1".parse().unwrap(), &req));
        assert!(version_matches(&"1.0.2".parse().unwrap(), &req));
        assert!(version_matches(&"1.0.2-rc1".parse().unwrap(), &req));

        assert!(!version_matches(&"1.0.1-rc1".parse().unwrap(), &req));
        assert!(!version_matches(&"1.0.0-rc1".parse().unwrap(), &req));
        assert!(!version_matches(&"1.0.0".parse().unwrap(), &req));
        assert!(!version_matches(&"2.0.0-rc1".parse().unwrap(), &req));
        assert!(!version_matches(&"2.0.0".parse().unwrap(), &req));
    }
}
