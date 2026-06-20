use serde::Deserialize;
use std::path::{Path, PathBuf};

/// The platform-agnostic execution policy: what a tool may reach. Default = deny-all
/// (empty lists), the strict-by-default floor of ADR-20260620. Pure + serializable +
/// OS-agnostic, so the deny-corpus below runs on every CI leg including Windows.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Policy {
    pub egress_allow: Vec<String>,
    pub read_roots: Vec<PathBuf>,
    pub write_roots: Vec<PathBuf>,
}

/// Exact-host allow-list. No suffix matching: "notexample.com" must NOT match "example.com".
pub fn egress_allowed(p: &Policy, host: &str) -> bool {
    p.egress_allow.iter().any(|h| h == host)
}

/// True only if `path`, after lexical `..`/`.` normalization, stays within an allowed
/// root. Rejects traversal without touching the filesystem — identical on Windows/Linux,
/// no symlink resolution (deferred until a symlink threat is real).
pub fn path_allowed(p: &Policy, path: &Path, write: bool) -> bool {
    let norm = match normalize(path) {
        Some(n) => n,
        None => return false,
    };
    let roots = if write { &p.write_roots } else { &p.read_roots };
    roots.iter().any(|r| {
        normalize(r)
            .map(|rn| norm.starts_with(&rn))
            .unwrap_or(false)
    })
}

/// Lexical normalization: resolve `.`/`..`; return None if it escapes above the root.
fn normalize(path: &Path) -> Option<PathBuf> {
    let mut out = PathBuf::new();
    for c in path.components() {
        use std::path::Component::*;
        match c {
            ParentDir => {
                if !out.pop() {
                    return None;
                }
            }
            CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    Some(out)
}

/// Side-effectful / irreversible tools require approval (strict-by-default). MCP tools are
/// namespaced "server::tool"; strip the prefix so a remote tool named like a side-effectful
/// builtin gets the SAME approval gate (spec: "same approval rules as built-in tools") — a
/// remote `evil::write_file` must not slip past a bare-name match.
pub fn approval_required(tool: &str) -> bool {
    let bare = tool.rsplit("::").next().unwrap_or(tool);
    matches!(bare, "write_file" | "shell" | "execute_code")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn policy() -> Policy {
        Policy {
            egress_allow: vec!["example.com".into(), "api.github.com".into()],
            read_roots: vec![PathBuf::from("/work")],
            write_roots: vec![PathBuf::from("/work/out")],
        }
    }

    #[test]
    fn egress_default_denies_unlisted_hosts() {
        let p = policy();
        assert!(egress_allowed(&p, "example.com"));
        assert!(!egress_allowed(&p, "evil.test"));
        assert!(!egress_allowed(&p, "notexample.com"));
    }

    #[test]
    fn path_traversal_and_unlisted_roots_are_denied() {
        let p = policy();
        assert!(path_allowed(&p, &PathBuf::from("/work/a.txt"), false));
        assert!(!path_allowed(
            &p,
            &PathBuf::from("/work/../etc/shadow"),
            false
        ));
        assert!(!path_allowed(&p, &PathBuf::from("/etc/passwd"), false));
        assert!(path_allowed(&p, &PathBuf::from("/work/out/r.txt"), true));
        assert!(!path_allowed(&p, &PathBuf::from("/work/a.txt"), true));
    }

    #[test]
    fn side_effectful_tools_require_approval() {
        assert!(approval_required("write_file"));
        assert!(approval_required("execute_code"));
        assert!(!approval_required("read_file"));
        assert!(!approval_required("fetch"));
    }

    #[test]
    fn namespaced_mcp_tools_get_the_same_approval_gate() {
        // A remote MCP tool named like a side-effectful builtin must NOT evade the gate.
        assert!(approval_required("evil::write_file"));
        assert!(approval_required("evil::execute_code"));
        assert!(approval_required("evil::shell"));
        assert!(approval_required("a::b::write_file")); // only the last segment matters
                                                        // read-only-named remote tools follow the same name convention as builtins
        assert!(!approval_required("rose::search"));
        assert!(!approval_required("evil::read_file"));
    }
}
