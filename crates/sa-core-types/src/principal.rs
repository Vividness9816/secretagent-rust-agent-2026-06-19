use crate::types::Provenance;

/// Who is driving a run. The dangerous capability — dispatching a side-effectful tool
/// with no per-tool grant, or auto-activating a draft skill — is reachable ONLY from
/// `Operator`. `Remote` carries no field or method that yields it, so "a remote message
/// auto-approved a side-effect" is *unrepresentable* (the `Tainted<T>` precedent).
/// ADR-20260621 M1.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Principal {
    /// Local CLI/TTY operator. `auto_approve` is the `--yes` standing consent — valid only
    /// because a human is attending the run.
    Operator { auto_approve: bool },
    /// A connector-sourced sender (untrusted). Never auto-approves ad-hoc; reaches a
    /// side-effectful tool ONLY via the run's frozen, operator-armed `allow_list`.
    Remote { connector: String, sender: String },
}

/// One run's trust context: who is asking + the side-effect tools pre-authorized for this
/// binding/job. `allow_list` is empty for an `Operator` (which uses `auto_approve` instead);
/// for a `Remote` it is the operator-armed, frozen set (a connector binding or a cron job).
#[derive(Debug, Clone)]
pub struct RunContext {
    pub principal: Principal,
    pub allow_list: Vec<String>,
}

impl RunContext {
    /// Local CLI run. `auto_approve` = `--yes`.
    pub fn operator(auto_approve: bool) -> Self {
        Self {
            principal: Principal::Operator { auto_approve },
            allow_list: Vec::new(),
        }
    }

    /// Connector-/cron-driven run with a frozen, operator-armed side-effect allow-list.
    pub fn remote(
        connector: impl Into<String>,
        sender: impl Into<String>,
        allow_list: Vec<String>,
    ) -> Self {
        Self {
            principal: Principal::Remote {
                connector: connector.into(),
                sender: sender.into(),
            },
            allow_list,
        }
    }

    pub fn is_operator(&self) -> bool {
        matches!(self.principal, Principal::Operator { .. })
    }

    /// May this run dispatch a side-effectful tool (one `approval_required` flags)?
    /// `Operator`: iff `--yes`. `Remote`: iff the tool's BARE name (MCP `server::` stripped,
    /// matching `approval_required`) is in the frozen `allow_list`. NEVER via ad-hoc consent.
    pub fn may_run_side_effect(&self, tool: &str) -> bool {
        let bare = tool.rsplit("::").next().unwrap_or(tool);
        match &self.principal {
            Principal::Operator { auto_approve } => *auto_approve,
            Principal::Remote { .. } => self.allow_list.iter().any(|t| t == bare),
        }
    }

    /// May this run auto-activate the intent-bound draft skill it authored? `Operator` + `--yes`
    /// ONLY. `Remote` NEVER (M1/M2): a remote sender controls `slug(task)` and must not flip trust.
    pub fn may_auto_activate_skill(&self) -> bool {
        matches!(self.principal, Principal::Operator { auto_approve: true })
    }

    /// May this run WRITE durable memory (skills / prefs / user_model)? `Operator` ONLY (M2).
    /// Default-deny: a future 3rd principal is non-persisting until explicitly opted in here.
    pub fn may_persist(&self) -> bool {
        self.is_operator()
    }

    /// Provenance to stamp this run's user input. `Operator` → `Trusted`; `Remote` →
    /// `Untrusted { source }` (so connector input flows through the unchanged injection guard).
    pub fn provenance(&self) -> Provenance {
        match &self.principal {
            Principal::Operator { .. } => Provenance::Trusted,
            Principal::Remote { connector, sender } => Provenance::Untrusted {
                source: format!("{connector}:{sender}"),
            },
        }
    }

    /// Short, secret-free label for the audit log: `"operator"` | `"remote:telegram:<sender>"`.
    pub fn audit_label(&self) -> String {
        match &self.principal {
            Principal::Operator { .. } => "operator".to_string(),
            Principal::Remote { connector, sender } => format!("remote:{connector}:{sender}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn operator_yes_may_run_side_effects_and_persist() {
        let ctx = RunContext::operator(true);
        assert!(ctx.is_operator());
        assert!(ctx.may_run_side_effect("write_file"));
        assert!(ctx.may_auto_activate_skill());
        assert!(ctx.may_persist());
        assert_eq!(ctx.provenance(), Provenance::Trusted);
    }

    #[test]
    fn operator_strict_denies_side_effects_but_may_persist() {
        let ctx = RunContext::operator(false);
        assert!(!ctx.may_run_side_effect("write_file"));
        assert!(!ctx.may_auto_activate_skill());
        assert!(ctx.may_persist()); // an attended operator still learns skills
        assert_eq!(ctx.provenance(), Provenance::Trusted);
    }

    #[test]
    fn remote_never_auto_approves_persists_or_activates() {
        let ctx = RunContext::remote("telegram", "12345", vec![]);
        assert!(!ctx.is_operator());
        assert!(!ctx.may_run_side_effect("write_file"));
        assert!(!ctx.may_auto_activate_skill());
        assert!(!ctx.may_persist()); // M2: a remote run writes no durable memory
        assert_eq!(
            ctx.provenance(),
            Provenance::Untrusted {
                source: "telegram:12345".into()
            }
        );
        assert_eq!(ctx.audit_label(), "remote:telegram:12345");
    }

    #[test]
    fn remote_reaches_a_side_effect_only_via_a_frozen_grant() {
        let ctx = RunContext::remote("cron", "job7", vec!["write_file".into()]);
        assert!(ctx.may_run_side_effect("write_file")); // operator-armed grant
        assert!(!ctx.may_run_side_effect("execute_code")); // not granted → denied
                                                           // even with a grant, a remote run never writes durable memory or auto-activates
        assert!(!ctx.may_persist());
        assert!(!ctx.may_auto_activate_skill());
    }

    #[test]
    fn remote_grant_strips_mcp_namespace_like_approval_required() {
        // A frozen grant for "write_file" must also cover the namespaced "evil::write_file"
        // form, matching how approval_required strips `::` (no bypass via namespacing).
        let ctx = RunContext::remote("telegram", "1", vec!["write_file".into()]);
        assert!(ctx.may_run_side_effect("evil::write_file"));
        // and a remote without the grant is denied even on the namespaced name
        let ctx2 = RunContext::remote("telegram", "1", vec![]);
        assert!(!ctx2.may_run_side_effect("evil::write_file"));
    }
}
