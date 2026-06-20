# SecretAgent

A self-hosted, autonomous AI agent daemon — a single self-contained binary.

> **Status:** Phase 0 (foundation). See `docs/superpowers/plans/` for the build plan
> and `~/.claude/second-brain/decisions/ADR-20260619-secretagent-founding-architecture.md`
> for the founding architecture decision.

## Heritage & differences

SecretAgent is an **independent Rust reimplementation**, not a fork of Hermes Agent.
It reimplements observable behavior; it copies no upstream source. It intentionally
diverges in three ways:

- **Security-first defaults** — vault-only credentials (never plaintext `.env`),
  sandboxed execution, strict-by-default, tool output treated as untrusted data.
- **Zero-friction install** — a single self-contained binary; no interpreter, no venv,
  no shell-rc mutation. (On Linux that binary is fully static musl; on macOS/Windows it
  is a native single executable linking only OS libraries.)
- **Honest provenance** — this section; an open `NOTICE`; issues are never silently
  edited or deleted.

See `NOTICE` for upstream credits.

## License

MIT.
