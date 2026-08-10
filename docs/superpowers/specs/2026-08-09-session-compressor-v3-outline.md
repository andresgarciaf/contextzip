# Session Compressor v3 — Design (data-driven rewrite)

Date: 2026-08-10
Status: Approved design. Supersedes the original Approach-B outline.
Scope: Metadata-level session compression on the v2-hardened engine.

## Why this replaces the original outline

The original v3 outline proposed semantic handlers for MCP / WebFetch / Task
tool_results (Approach B). Measurement against 98 real sessions (96.8 MB) killed
most of it:

- **All `message.content` tool_result content is only 8% of session bytes** — that
  is v2's entire target surface. WebFetch results are already-extracted markdown
  (no HTML to extract; v2's GenericResultCap already caps them). Task/subagent
  outputs are low-volume (5 TaskOutput records, 31 KB) and 67 of 204 Agent results
  are identical launch boilerplate.
- **The other ~92% lives in metadata v2 never touches.** Bytes by JSON key-path:
  `content[].source.data` (base64 images) 10.6 MB; the top-level `toolUseResult`
  sidecar (`originalFile`/`stdout`/`file.content`/`file.base64`/`structuredPatch`)
  ~18 MB and present on 6,872 records; thinking-block `signature` 5.5 MB.

So "best in class" = go after the sidecar duplication, base64 media, and
signatures — roughly an order of magnitude more savings than v2 + the original v3
combined — plus keep a small MCP JSON handler.

## Architecture

Same engine and contract as v2. v2 rewrote only `message.content[].tool_result`
blocks; v3 extends the two-pass rewriter to also process **top-level record
fields** (`toolUseResult`, thinking `signature`, image `source.data`) and adds one
tool_result content handler (MCP). Every axis keeps v2's four invariants —
reversible (annotate `contextzip_compressed`), idempotent, never-inflate,
fallback-safe — and `expand` restores byte-for-byte from `.bak`.

## Axes

| Axis | Target | Mechanic | Est. gross |
|------|--------|----------|-----------:|
| `SidecarDedup` | top-level `toolUseResult` (6,872 records) | When its payload is byte-equal to the same record's `message.content` tool_result, replace the sidecar copy with a `{contextzip_ref}` marker; if they differ, leave it. | ~18 MB |
| `MediaReference` | `content[].source.data`, `toolUseResult.file.base64` | Replace base64 image data with `[contextzip: image sha256=… N bytes]` + annotation carrying the sha. | ~11 MB |
| `SignatureDrop` | thinking-block `signature` | Remove replay-only crypto signature; annotate presence + sha so `expand` restores. | ~5.5 MB |
| `McpJsonCompact` | `mcp__*` tool_result content | Minify JSON, unwrap double-encoded `{"result":"{...}"}`, prune known-verbose fields. | ~0.4 MB |

## Safety model (the load-bearing decision)

`toolUseResult`, `source.data`, and `signature` are Claude-Code-INTERNAL fields,
not model-facing context. `compact` (writes a sidecar only) is always safe. The
risk is at `apply`, which promotes the compacted file to the live
`session.jsonl` — Claude Code may read these fields on session RESUME.

Therefore:

1. **These four v3 axes default OFF.** They run only under `compact --aggressive`
   (or `compact.aggressive = true`). v2's axes remain on by default. A new
   `CompactConfig.aggressive: bool` (default false) gates them.
2. **Mandatory resume test before ship (plan gating step):** empirically apply a
   compacted-aggressive session and confirm Claude Code can still open/resume it.
   If resume breaks, the axis that breaks it stays behind an explicit opt-in with a
   documented warning; `.bak`/`expand` always recovers regardless.
3. Reversibility bar: full round-trip via `.bak` (same as v2). MediaReference and
   SignatureDrop annotate a sha of what they removed so `expand` restores from
   `.bak` and can verify integrity.

## Out of scope

- WebFetch/Task dedicated handlers (v2 already covers; low volume).
- `attachment.*` compression (~7 MB) — deferred to a possible v4; attachments have
  a distinct record type and lifecycle, worth its own spec.
- Annotation-only expand (no `.bak`) — still v-future.

## Testing

Per v2's bar: each axis gets snapshot + savings + idempotency + malformed-input
tests against real session fixtures; SidecarDedup gets a "differing sidecar left
intact" test; MediaReference/SignatureDrop get "sha recorded + expand restores"
tests; the aggressive gate gets an on/off test. Plus the one manual apply+resume
round-trip gating step.
