# Session Compressor v3 — Outline (Approach B, deferred)

Date: 2026-08-09
Status: Outline only. Depends on v2 (axes + hardening) shipping first.
Scope: Semantic per-tool handlers layered on the v2-hardened compressor.

## Premise

v2 lands the safe, high-ROI dedup axes plus the full security base (redaction, `.bak`
retention, Read staleness, path gating). v3 adds per-tool *structural-semantic* handlers
that understand a tool's output shape to compress further than the generic cap. Still
**no network, no async, no LLM** — heuristic/structural only.

Each handler replaces the `GenericResultCap` fallback for its tool type and must honor the
same four invariants (reversible, idempotent, never-inflate, fallback-safe) and pass
through `redact::scrub` before any write.

## Handlers

| Axis | Mechanic | Reuse |
|------|----------|-------|
| `McpResultCompact` | Minify JSON tool_results; prune known-verbose/duplicative fields; keep a structural skeleton for large arrays (first N + `(M more)`), never drop keys silently. Non-JSON MCP output falls back to the generic cap. | serde_json |
| `WebFetchCompact` | Route HTML tool_results through the existing `web_cmd::extract_content()` to keep main/article text and drop chrome. | `src/web_cmd.rs` |
| `TaskOutputCompact` | Line-cap + tail-drop oversized subagent final outputs; preserve the head (the answer) and a bounded tail. | shares cap helpers with v2 |

## Open questions to resolve when v3 is planned

- MCP field-pruning list: which fields are safe to drop is server-specific. Start with a
  conservative allowlist of universally-verbose fields; make the list config-driven.
- WebFetch: session tool_results may not carry the original HTML (Claude Code may store
  already-extracted text). Verify against real fixtures before building the handler.
- Task output: confirm the record shape for subagent final outputs in the session JSONL.

## Risk note

Each handler is a new correctness + security surface over untrusted session content. v3 is
deliberately scoped as its own spec → plan → implementation cycle so each handler gets its
own fixtures, tests, and security review rather than being bundled.
