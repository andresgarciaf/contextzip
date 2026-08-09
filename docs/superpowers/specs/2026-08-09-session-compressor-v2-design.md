# Session Compressor v2 — Design

Date: 2026-08-09
Status: Approved design, pending spec review
Scope: Extend the existing session-history compressor (`compact`/`apply`/`expand`) with
Approach A axes and full security/compliance hardening. Approach B (semantic per-tool
handlers) is deferred to v3 — see the companion v3 outline.

## Background

ContextZip already ships a reversible session-history compressor:

- `contextzip compact <session>` writes a `<session>.jsonl.compressed` sidecar.
- `contextzip apply` atomically swaps the sidecar in, backing the original up to `.bak`.
- `contextzip expand` rolls back from `.bak`.

The rewrite engine is `src/jsonl_rewriter.rs`. Today it touches only `user` records'
`tool_result` blocks and handles exactly two tool types:

| Tool | Treatment |
|------|-----------|
| `Read` | Dedup: 2nd+ read of same `file_path` collapses to a reference marker |
| `Bash` | Text recompress: ANSI strip, blank-run drop, repeat-line tally, 200-line cap |
| everything else | pass-through, untouched |

Every axis honors four invariants, which v2 preserves unchanged:

1. **Reversible** — original never mutated in place; annotation `contextzip_compressed`
   records the axis + metadata.
2. **Idempotent** — a block already carrying `contextzip_compressed` is skipped.
3. **Never inflate** — if the filtered result is not smaller, leave the block alone.
4. **Fallback-safe** — unparseable JSONL lines pass through verbatim.

## Non-negotiable constraints (from repo rules)

- **No async, no network.** `local_llm.rs` is heuristic-only; no HTTP crate is present.
  All compression is structural/heuristic string work. No LLM summarization.
- `lazy_static!` for all regex. `anyhow::Result` + `.context()`. No `unwrap()` outside
  tests/regex-init. Fallback to raw on any filter failure.

## v2 scope

### Part 1 — New compression axes (Approach A)

All three are added as new arms in `rewrite_record`'s `match info.name` plus the
matching index entries. No new command, no new top-level module for the axes themselves.

| Axis | Mechanic | Model |
|------|----------|-------|
| `GrepGlobDedup` | 2nd+ identical Grep/Glob `tool_result` collapses to a reference marker, keyed on the tool_use's normalized `input` args (pattern + path + flags). | Mirror of `ReadDedup` |
| `BashCmdDedup` | When the same Bash *command string* recurs and its output repeats, reference the first occurrence instead of re-compressing text. Layers above today's `BashHistoryCompact`; dedup wins when both apply. | Mirror of `ReadDedup`, keyed on command string |
| `GenericResultCap` | Any *other* `tool_result` whose text exceeds `compact.generic_cap_chars` gets ANSI-strip + line-cap + explicit `(N more lines)` tail marker. One blunt rule; deliberately no per-tool semantics (that is v3). | New, simple |

Indexing changes (`index_record`):
- Extend the tool_use index to capture Grep/Glob normalized-args key and Bash command
  string, not just Read `file_path`.
- Add `first_grepglob_for: HashMap<ArgsKey, FirstResult>` and
  `first_bashcmd_for: HashMap<CmdString, FirstResult>` mirroring `first_read_for`.

Ordering rule in `rewrite_record`: dedup axes run before the text/cap axes for the same
block, and `GenericResultCap` only fires for tool names not handled by a dedicated axis.

### Part 2 — Security / compliance hardening (full)

This ships in v2 because v2 adds new sidecar-writing paths, and it is the base v3 builds on.

1. **Secret redaction (gating).** New `src/redact.rs`. Before *any* sidecar or `.bak`
   write, scan content and replace matches with `[REDACTED:<kind>]`:
   - Databricks PAT: `dapi` + hex.
   - AWS access key id: `AKIA[0-9A-Z]{16}`.
   - Private key blocks: `-----BEGIN ... PRIVATE KEY-----` … `-----END ...-----`.
   - JWT: three base64url segments separated by dots.
   - OpenAI-style: `sk-` + long token.
   All patterns are `lazy_static!` regexes. **Compliance tradeoff (explicit):** redacting
   a `.bak` means restore is no longer byte-exact. We accept non-byte-exact restore in
   exchange for never persisting a secret to disk in a ContextZip-created file. The
   `expand` byte-exactness test is updated to a "content-equal modulo redaction" assertion.
   Redaction is on by default; `compact.redact = false` opts out for users who need
   byte-exact `.bak`.
2. **`.bak` retention.** New `[compact]` config section with `backup_retention_days`
   (default 7). On `compact` and `apply`, sweep `.bak` files older than the window under
   the session's project dir. `0` disables the sweep.
3. **Read staleness.** Restore `FirstRead.content_sha256` (removed at
   `jsonl_rewriter.rs:133-135`). `expand`/dedup-reference logic can then detect a changed
   file before serving cached read content. The dedup marker already names the path; the
   sha lets a future consumer verify freshness rather than silently trusting the cache.
4. **Path disclosure.** Dedup markers embed absolute paths for usability. Keep, but gate
   behind `compact.include_paths_in_markers` (default true) so a stricter deployment can
   suppress absolute paths.
5. **Security review.** After implementation, run the `andres-garcia:andres-code-reviewer`
   agent (security lens) over the full compact path: `jsonl_rewriter.rs`, `redact.rs`,
   `compact_cmd.rs`.

### Config additions (`src/config.rs`)

New `CompactConfig` on `Config`:

```rust
pub struct CompactConfig {
    pub redact: bool,                    // default true
    pub backup_retention_days: u32,      // default 7
    pub generic_cap_chars: usize,        // default 4000
    pub generic_cap_lines: usize,        // default 200
    pub include_paths_in_markers: bool,  // default true
}
```

## Data flow (unchanged shape)

```
compact_session_str(input)
  pass 1: index_record  -> tool_use_index
                           first_read_for / first_grepglob_for / first_bashcmd_for
  pass 2: rewrite_record -> per tool_result block:
                              ReadDedup | GrepGlobDedup | BashCmdDedup
                              | BashHistoryCompact | GenericResultCap
          redact::scrub  -> applied to final serialized output before write
```

Redaction is applied as a final scrub pass over the produced sidecar text (and over the
original bytes when writing `.bak` in `apply`), so it covers every axis and any
pass-through content uniformly, in one place.

## Error handling

- Any axis that errors on a block leaves that block unchanged (existing pattern).
- Redaction failure (regex/never expected) → log to stderr, write the un-redacted content
  only if `redact = false`; otherwise abort the write with a clear error rather than
  persisting a possibly-secret-bearing file. (Fail closed on the security-critical path.)
- Retention sweep failure (permission) → warn on stderr, continue; never blocks compaction.

## Testing

Per mandated CLI-testing rules, against real session fixtures under `tests/fixtures/`:

- Each new axis: snapshot test, ≥60% token-savings test, idempotency test, malformed-input
  passthrough test.
- `GrepGlobDedup` / `BashCmdDedup`: unique-result-not-touched test (mirror of the existing
  `read_dedup_does_not_touch_unique_reads`).
- Redaction: "a planted `dapi…`/`AKIA…`/private-key never appears in the sidecar or `.bak`"
  test; "`redact=false` preserves bytes" test.
- Retention: sweep removes an aged `.bak`, keeps a fresh one, `0` disables.
- Read staleness: `expand` detects a changed sha and does not silently serve stale content.

## Explicitly out of scope (→ v3)

- Semantic per-tool handlers: `McpResultCompact`, `WebFetchCompact`, `TaskOutputCompact`.
- Reconstructing the original from `contextzip_compressed` annotations alone (annotation-only
  expand with no `.bak`).
