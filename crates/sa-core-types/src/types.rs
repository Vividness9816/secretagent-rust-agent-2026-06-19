use serde::{Deserialize, Serialize};

/// On-disk/wire schema version for the canonical types. Bump when a field is
/// added or its meaning changes (ADR invariant #2 / SQLite-canonical lineage).
pub const SCHEMA_VERSION: u32 = 1;

/// Source-of-truth tag for any content entering the agent's context.
///
/// Locked now as a non-optional field on the canonical message/tool types so a
/// future contributor cannot construct tool output without stating its origin.
/// The `Tainted<T>` wrapper that makes "untrusted output is never promotable to
/// an instruction" a *compile error* arrives in Phase 2, when `sa-core`'s
/// injection guard is its real consumer (ADR invariant #3).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Provenance {
    /// Operator- or system-authored content. May carry instructions.
    Trusted,
    /// Content from a tool, connector, or fetched resource. Data, never commands.
    Untrusted { source: String },
}

/// A conversation message. `provenance` is non-optional by construction.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Message {
    pub role: String,
    pub content: String,
    pub provenance: Provenance,
}

/// A tool invocation + its (untrusted) result lineage. `provenance` is
/// non-optional by construction.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolCall {
    pub tool: String,
    pub input: String,
    pub provenance: Provenance,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_output_must_carry_provenance_and_round_trips() {
        // A tool result is Untrusted by construction — it cannot be built without provenance.
        let tc = ToolCall {
            tool: "web.fetch".into(),
            input: "https://x".into(),
            provenance: Provenance::Untrusted {
                source: "web.fetch".into(),
            },
        };
        let json = serde_json::to_string(&tc).unwrap();
        let back: ToolCall = serde_json::from_str(&json).unwrap();
        assert_eq!(
            back.provenance,
            Provenance::Untrusted {
                source: "web.fetch".into()
            }
        );
        assert_eq!(SCHEMA_VERSION, 1);
    }

    #[test]
    fn trusted_and_untrusted_are_distinct_after_round_trip() {
        let m = Message {
            role: "user".into(),
            content: "hi".into(),
            provenance: Provenance::Trusted,
        };
        let back: Message = serde_json::from_str(&serde_json::to_string(&m).unwrap()).unwrap();
        assert_eq!(back.provenance, Provenance::Trusted);
        assert_ne!(
            Provenance::Trusted,
            Provenance::Untrusted { source: "x".into() }
        );
    }
}
