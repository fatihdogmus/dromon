use std::fmt;

use serde::{Deserialize, Serialize};

/// An absolute POSIX path on the remote side, always normalized: it starts with `/`, has no
/// empty, `.` or `..` components, and no trailing slash (except the root itself).
///
/// Windows OpenSSH servers expose drives as `/C:/...`, which this type handles as ordinary
/// components.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(into = "String", from = "String")]
pub struct RemotePath(String);

impl RemotePath {
    pub fn root() -> Self {
        RemotePath("/".into())
    }

    /// Parses and normalizes `s`. A relative path is taken relative to the root.
    pub fn new(s: &str) -> Self {
        let mut parts: Vec<&str> = Vec::new();
        for part in s.split('/') {
            match part {
                "" | "." => {}
                ".." => {
                    parts.pop();
                }
                p => parts.push(p),
            }
        }
        if parts.is_empty() {
            Self::root()
        } else {
            RemotePath(format!("/{}", parts.join("/")))
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn is_root(&self) -> bool {
        self.0 == "/"
    }

    /// The parent folder, or `None` at the root.
    pub fn parent(&self) -> Option<RemotePath> {
        if self.is_root() {
            return None;
        }
        let idx = self.0.rfind('/').unwrap_or(0);
        Some(if idx == 0 { Self::root() } else { RemotePath(self.0[..idx].to_owned()) })
    }

    /// The last component, or `None` at the root.
    pub fn file_name(&self) -> Option<&str> {
        if self.is_root() { None } else { self.0.rsplit('/').next() }
    }

    /// Appends `name`, which may itself contain slashes or `..`; the result is normalized.
    /// An absolute `name` replaces the path, as in `Path::join`.
    pub fn join(&self, name: &str) -> RemotePath {
        if name.starts_with('/') {
            RemotePath::new(name)
        } else {
            RemotePath::new(&format!("{}/{}", self.0, name))
        }
    }

    /// The components from the root down, without the root itself.
    pub fn components(&self) -> impl Iterator<Item = &str> {
        self.0.split('/').filter(|p| !p.is_empty())
    }

    /// Every ancestor from the root down to (and including) this path. Used for breadcrumbs.
    pub fn ancestors_from_root(&self) -> Vec<RemotePath> {
        let mut out = vec![Self::root()];
        let mut cur = String::new();
        for part in self.components() {
            cur.push('/');
            cur.push_str(part);
            out.push(RemotePath(cur.clone()));
        }
        out
    }

    /// True when `self` is `other` or lies inside it.
    pub fn starts_with(&self, other: &RemotePath) -> bool {
        other.is_root()
            || self.0 == other.0
            || (self.0.starts_with(&other.0) && self.0.as_bytes().get(other.0.len()) == Some(&b'/'))
    }
}

impl fmt::Display for RemotePath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl fmt::Debug for RemotePath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "RemotePath({:?})", self.0)
    }
}

impl From<RemotePath> for String {
    fn from(p: RemotePath) -> String {
        p.0
    }
}

impl From<String> for RemotePath {
    fn from(s: String) -> Self {
        RemotePath::new(&s)
    }
}

impl From<&str> for RemotePath {
    fn from(s: &str) -> Self {
        RemotePath::new(s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes() {
        assert_eq!(RemotePath::new("").as_str(), "/");
        assert_eq!(RemotePath::new("/").as_str(), "/");
        assert_eq!(RemotePath::new("//var//www/").as_str(), "/var/www");
        assert_eq!(RemotePath::new("/var/./www/../log").as_str(), "/var/log");
        assert_eq!(RemotePath::new("/../..").as_str(), "/");
        assert_eq!(RemotePath::new("home/me").as_str(), "/home/me");
        assert_eq!(RemotePath::new("/C:/Users").as_str(), "/C:/Users");
    }

    #[test]
    fn parent_and_name() {
        let p = RemotePath::new("/var/www/site");
        assert_eq!(p.file_name(), Some("site"));
        assert_eq!(p.parent().unwrap().as_str(), "/var/www");
        assert_eq!(RemotePath::new("/var").parent().unwrap().as_str(), "/");
        assert_eq!(RemotePath::root().parent(), None);
        assert_eq!(RemotePath::root().file_name(), None);
    }

    #[test]
    fn join() {
        let p = RemotePath::new("/var/www");
        assert_eq!(p.join("site").as_str(), "/var/www/site");
        assert_eq!(p.join("../log").as_str(), "/var/log");
        assert_eq!(p.join("/etc").as_str(), "/etc");
        assert_eq!(RemotePath::root().join("a").as_str(), "/a");
    }

    #[test]
    fn ancestors_and_prefix() {
        let p = RemotePath::new("/var/www");
        let a: Vec<_> = p.ancestors_from_root().into_iter().map(String::from).collect();
        assert_eq!(a, ["/", "/var", "/var/www"]);
        assert!(p.starts_with(&RemotePath::new("/var")));
        assert!(p.starts_with(&RemotePath::root()));
        assert!(p.starts_with(&p));
        assert!(!RemotePath::new("/varnish").starts_with(&RemotePath::new("/var")));
    }
}
