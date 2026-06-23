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

/// True only if `path` stays within an allowed root. Lexical `..`/`.` normalization is the
/// cross-platform floor (rejects traversal without touching the filesystem). For WRITES it ALSO
/// resolves symlinks (ADR-20260621 4d): if the target's longest existing ancestor canonicalizes
/// outside the write root, the write is denied — closing a symlinked-write-root escape on the
/// unattended path. ponytail: write path only; reads can adopt the same `resolves_within` helper
/// if a read-symlink exfil threat becomes real.
pub fn path_allowed(p: &Policy, path: &Path, write: bool) -> bool {
    let norm = match normalize(path) {
        Some(n) => n,
        None => return false,
    };
    let roots = if write { &p.write_roots } else { &p.read_roots };
    let lexically_ok = roots.iter().any(|r| {
        normalize(r)
            .map(|rn| norm.starts_with(&rn))
            .unwrap_or(false)
    });
    if !lexically_ok {
        return false;
    }
    if !write {
        return true;
    }
    // Defense-in-depth for writes. When the filesystem entries exist, a symlinked ancestor must
    // not resolve outside the canonicalized write root. When nothing exists yet there is no
    // symlink to exploit, so the lexical pass stands (keeps the pure cross-platform deny-corpus
    // valid — those roots don't exist on the test box).
    if roots.iter().any(|r| resolves_within(r, path)) {
        return true;
    }
    // No root could be canonicalized (e.g. roots absent from disk) → trust the lexical pass.
    roots.iter().all(|r| std::fs::canonicalize(r).is_err())
}

/// True if `target`'s longest existing ancestor, canonicalized (resolving symlinks) and rejoined
/// with the non-existing remainder, stays within the canonicalized `root`. Returns false if
/// `root` cannot be canonicalized (the caller decides the fallback).
fn resolves_within(root: &Path, target: &Path) -> bool {
    let croot = match std::fs::canonicalize(root) {
        Ok(c) => c,
        Err(_) => return false,
    };
    // Walk up until an existing ancestor canonicalizes, then re-append the non-existing tail.
    let mut existing = target.to_path_buf();
    let mut remainder: Vec<std::ffi::OsString> = Vec::new();
    loop {
        if let Ok(c) = std::fs::canonicalize(&existing) {
            let mut resolved = c;
            for part in remainder.iter().rev() {
                resolved.push(part);
            }
            return resolved.starts_with(&croot);
        }
        match existing.file_name() {
            Some(name) => {
                remainder.push(name.to_os_string());
                if !existing.pop() {
                    return false;
                }
            }
            None => return false,
        }
    }
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
    matches!(
        bare,
        "write_file" | "shell" | "execute_code" | "activate_skill"
    )
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

    #[cfg(unix)]
    #[test]
    fn a_symlinked_write_root_cannot_escape_via_canonicalize() {
        use std::os::unix::fs::symlink;
        let tmp = tempfile::tempdir().unwrap();
        let safe = tmp.path().join("safe");
        let secret = tmp.path().join("secret");
        std::fs::create_dir_all(&safe).unwrap();
        std::fs::create_dir_all(&secret).unwrap();
        // A symlink INSIDE the write root pointing out of it.
        let escape = safe.join("escape");
        symlink(&secret, &escape).unwrap();
        let p = Policy {
            write_roots: vec![safe.clone()],
            ..Default::default()
        };
        // Lexically `safe/escape/x` starts_with `safe`, but it resolves into `secret` → deny.
        assert!(
            !path_allowed(&p, &escape.join("x"), true),
            "symlinked escape must be denied"
        );
        // A genuine path inside the real root is still allowed.
        assert!(path_allowed(&p, &safe.join("ok.txt"), true));
    }

    #[test]
    fn skill_activation_requires_approval_without_namespace_collision() {
        assert!(approval_required("activate_skill"));
        // a remote MCP tool named to dodge the gate still strips to the gated last segment
        assert!(approval_required("evil::activate_skill"));
        // a bare, unrelated "activate" must NOT be gated (no over-broad collision)
        assert!(!approval_required("activate"));
        assert!(!approval_required("rose::activate"));
    }
}
