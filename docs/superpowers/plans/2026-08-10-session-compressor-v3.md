# Session Compressor v3 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add four metadata-level compression axes to the session compressor — SidecarDedup, MediaReference, SignatureDrop, McpJsonCompact — gated behind an `--aggressive` flag, targeting the ~92% of session bytes v2 never touched.

**Architecture:** v2's engine rewrites only `message.content[].tool_result` blocks. v3 adds a sibling metadata rewriter that processes top-level record fields (`toolUseResult`, thinking `signature`, image `source.data`) plus one MCP tool_result content handler. All four axes are gated by `CompactConfig.aggressive` (default false); v2 axes stay on by default. Same four invariants: reversible (annotate `contextzip_compressed`), idempotent, never-inflate, fallback-safe. `expand` restores byte-for-byte from `.bak`.

**Tech Stack:** Rust, `serde_json`, `sha2`, `lazy_static`+`regex`, `anyhow` (all existing deps).

## Global Constraints

- No async, no network, no new crates. All regex via `lazy_static!`.
- `anyhow::Result` + `.context()`; no `unwrap()` outside tests/regex-init.
- No em-dash (U+2014) in code/comments/strings — ASCII hyphen only.
- Preserve invariants: reversible, idempotent (skip anything already carrying a v3 annotation), never-inflate (bail if the rewrite is not smaller), fallback-safe (unparseable lines pass through verbatim).
- Never alter the `uuid`/`parentUuid` chain. v3 rewrites field VALUES (sidecar payload, base64 data, signature) but never removes records.
- The four v3 axes run ONLY when `cfg.aggressive` is true. When false, `compact_session_str` behaves exactly as v2.
- Secret-shaped test data assembled at runtime (block-secrets hook). clippy `-D warnings` and `cargo fmt --check` must stay clean.
- Real session fixtures live under `~/.claude/projects/*/*.jsonl`; copy small realistic excerpts into `tests/fixtures/` (do NOT read from the live dir in tests). Scrub any secrets from copied fixtures.

## Empirical grounding (from 98 real sessions, 96.8 MB)

- `content[].source.data` (base64 images): 10.6 MB. `image` blocks, `source.type=base64`, `media_type=image/*`.
- top-level `toolUseResult` sidecar: ~18 MB across 6,872 records; subkeys include `stdout`, `originalFile`, `file.content`, `file.base64`, `structuredPatch`, `content`, `type`.
- thinking-block `signature`: 5.5 MB; block shape `{type:"thinking", thinking, signature}`.
- `mcp__*` tool_result content: ~0.4 MB, JSON strings often double-encoded `{"result":"{\"...\"}"}`.

---

### Task 1: Add `aggressive` flag to CompactConfig + CLI

**Files:**
- Modify: `src/config.rs` (add `aggressive: bool` field, default false, to `CompactConfig`)
- Modify: `src/main.rs` (add `--aggressive` flag to the `Compact` command; thread into the config used by compact)
- Modify: `src/compact_cmd.rs` (`run_with_options` accepts an `aggressive` override)

**Interfaces:**
- Produces: `CompactConfig.aggressive: bool` (default false via `#[serde(default)]`); `compact_cmd::run_with_options(target, dry_run, aggressive, verbose)`.

- [ ] **Step 1: Write the failing test** (in `src/config.rs` tests)
```rust
#[test]
fn compact_config_aggressive_defaults_off() {
    assert!(!CompactConfig::default().aggressive, "aggressive must default OFF");
}
```

- [ ] **Step 2: Run** `cargo test compact_config_aggressive_defaults_off` — expect FAIL (no field).

- [ ] **Step 3: Implement.** Add to `CompactConfig`:
```rust
    #[serde(default)]
    pub aggressive: bool,
```
Add `aggressive: false` to the `Default` impl. In `src/main.rs`'s `Compact` subcommand struct add:
```rust
        /// Enable aggressive metadata axes (sidecar dedup, media referencing,
        /// signature drop, MCP JSON compaction). Off by default: these touch
        /// Claude Code internal fields and are only safe to `apply` after a
        /// resume test. `compact` (sidecar-only) is always safe.
        #[arg(long)]
        aggressive: bool,
```
Thread it: the compact handler builds its `CompactConfig` by loading `config::compact_config()` then setting `.aggressive |= cli_aggressive`. Update `run_with_options` signature to take `aggressive: bool` and OR it into the loaded config before passing to `compact_session_str`. Update `run_all_sessions` and all call sites (grep `run_with_options`).

- [ ] **Step 4: Run** the test — expect PASS. `cargo build` clean.

- [ ] **Step 5: Commit** `feat(compact): add --aggressive flag (default off) for v3 metadata axes`

---

### Task 2: Metadata-rewrite plumbing + SignatureDrop axis

**Files:**
- Modify: `src/jsonl_rewriter.rs` (`compact_session_str` second pass; new `rewrite_record_metadata`; `CompactStats`)

**Interfaces:**
- Produces: `fn rewrite_record_metadata(record: &mut Value, cfg: &CompactConfig, stats: &mut CompactStats)` called in the second pass after `rewrite_record`, ONLY when `cfg.aggressive`. `CompactStats.signatures_dropped: usize`.

- [ ] **Step 1: Write the failing test**
```rust
#[test]
fn signature_dropped_only_when_aggressive() {
    let rec = json!({"type":"assistant","uuid":"a1","message":{"content":[
        {"type":"thinking","thinking":"reasoning here","signature":"AAAABBBBCCCCDDDD_long_sig_value_xxxxxxxxxxxx"}
    ]}});
    let input = format!("{}\n", serde_json::to_string(&rec).unwrap());

    // aggressive OFF -> untouched
    let off = CompactConfig::default();
    let (out_off, s_off) = compact_session_str(&input, &off);
    assert!(out_off.contains("AAAABBBBCCCC"), "signature must survive when not aggressive");
    assert_eq!(s_off.signatures_dropped, 0);

    // aggressive ON -> dropped + annotated with sha
    let on = CompactConfig { aggressive: true, ..Default::default() };
    let (out_on, s_on) = compact_session_str(&input, &on);
    assert!(!out_on.contains("AAAABBBBCCCC"), "signature must be dropped when aggressive");
    assert!(out_on.contains("contextzip_sig"), "must annotate the dropped signature for expand");
    assert_eq!(s_on.signatures_dropped, 1);
}
```

- [ ] **Step 2: Run** `cargo test --lib signature_dropped_only_when_aggressive` — expect FAIL.

- [ ] **Step 3: Implement.**
- Add `pub signatures_dropped: usize` to `CompactStats`.
- In the second pass of `compact_session_str`, after the `rewrite_record(...)` call, add:
```rust
    if cfg.aggressive {
        rewrite_record_metadata(&mut record, cfg, &mut stats);
    }
```
- New fn `rewrite_record_metadata`: walk `record["message"]["content"]` array (if present); for each block with `type == "thinking"` carrying a non-empty `signature`, compute `sha = sha256_hex(sig)`, remove the `signature` field, and set a sibling `contextzip_sig` field on the block = `json!({"sha256": sha, "len": sig.len()})`. Increment `signatures_dropped`. Idempotency: if the block already has `contextzip_sig` (or has no `signature`), skip.

- [ ] **Step 4: Run** the test — expect PASS.

- [ ] **Step 5: Commit** `feat(compact): SignatureDrop axis (aggressive) drops replay-only thinking signatures`

---

### Task 3: MediaReference axis

**Files:**
- Modify: `src/jsonl_rewriter.rs` (`rewrite_record_metadata`, `CompactStats`)

**Interfaces:**
- Produces: `CompactStats.media_referenced: usize`. Applies to `image` blocks in `message.content` (`source.type == "base64"`) and to `toolUseResult.file.base64`.

- [ ] **Step 1: Write the failing test**
```rust
#[test]
fn media_referenced_when_aggressive() {
    let data = "X".repeat(5000); // stand-in base64 payload
    let rec = json!({"type":"user","uuid":"u1","message":{"content":[
        {"type":"image","source":{"type":"base64","media_type":"image/png","data": data}}
    ]}});
    let input = format!("{}\n", serde_json::to_string(&rec).unwrap());

    let on = CompactConfig { aggressive: true, ..Default::default() };
    let (out, s) = compact_session_str(&input, &on);
    assert!(!out.contains(&"X".repeat(5000)), "base64 data must be replaced");
    assert!(out.contains("contextzip_media"), "must annotate sha for expand");
    assert_eq!(s.media_referenced, 1);
    // never-inflate: a tiny image is left alone
    let tiny = json!({"type":"user","message":{"content":[
        {"type":"image","source":{"type":"base64","media_type":"image/png","data":"AA"}}]}});
    let (out2, s2) = compact_session_str(&format!("{}\n", tiny), &on);
    assert_eq!(s2.media_referenced, 0);
    assert!(out2.contains("\"data\":\"AA\""));
}
```

- [ ] **Step 2: Run** — expect FAIL.

- [ ] **Step 3: Implement.** In `rewrite_record_metadata`:
- Add `pub media_referenced: usize` to `CompactStats`.
- For each `image` block with `source.type == "base64"` and a `data` string: compute `sha = sha256_hex(data)`; build marker object `source.data = "[contextzip: media sha256=<sha> <N> bytes]"` and set sibling `contextzip_media = {"sha256":sha,"bytes":N,"media_type":<mt>}`. NEVER-INFLATE: only apply if the marker+annotation length < original data length; else skip. Idempotency: skip if `contextzip_media` present.
- Also handle `record["toolUseResult"]["file"]["base64"]` the same way (replace with marker + `contextzip_media` sibling under `file`). This is a top-level field, not in message.content.

- [ ] **Step 4: Run** — expect PASS.

- [ ] **Step 5: Commit** `feat(compact): MediaReference axis (aggressive) replaces inlined base64 with a sha marker`

---

### Task 4: SidecarDedup axis

**Files:**
- Modify: `src/jsonl_rewriter.rs` (`rewrite_record_metadata`, `CompactStats`)

**Interfaces:**
- Produces: `CompactStats.sidecars_deduped: usize`. Targets the top-level `toolUseResult` object when its payload duplicates the record's own `message.content` tool_result.

- [ ] **Step 1: Write the failing test**
```rust
#[test]
fn sidecar_deduped_when_byte_equal_and_aggressive() {
    let body = "line one\nline two\nline three".repeat(20);
    let rec = json!({
        "type":"user","uuid":"u1",
        "message":{"content":[{"type":"tool_result","tool_use_id":"t1","content": body.clone()}]},
        "toolUseResult":{"type":"text","stdout": body.clone()}
    });
    let input = format!("{}\n", serde_json::to_string(&rec).unwrap());
    let on = CompactConfig { aggressive: true, ..Default::default() };
    let (out, s) = compact_session_str(&input, &on);
    assert_eq!(s.sidecars_deduped, 1);
    assert!(out.contains("contextzip_ref"), "sidecar collapses to a reference");
    // The message.content copy is still present (only the duplicate sidecar collapses)
    let v: serde_json::Value = serde_json::from_str(out.lines().next().unwrap()).unwrap();
    assert!(v["message"]["content"][0]["content"].as_str().unwrap().contains("line one"));
}

#[test]
fn sidecar_left_intact_when_differs() {
    let rec = json!({
        "type":"user",
        "message":{"content":[{"type":"tool_result","tool_use_id":"t1","content":"AAA"}]},
        "toolUseResult":{"type":"text","stdout":"DIFFERENT CONTENT ENTIRELY that is quite a bit longer than AAA"}
    });
    let input = format!("{}\n", serde_json::to_string(&rec).unwrap());
    let on = CompactConfig { aggressive: true, ..Default::default() };
    let (out, s) = compact_session_str(&input, &on);
    assert_eq!(s.sidecars_deduped, 0, "differing sidecar must be left intact");
    assert!(out.contains("DIFFERENT CONTENT"));
}
```

- [ ] **Step 2: Run** — expect FAIL.

- [ ] **Step 3: Implement.** In `rewrite_record_metadata`:
- Add `pub sidecars_deduped: usize` to `CompactStats`.
- If `record["toolUseResult"]` exists AND `record["message"]["content"]` has a `tool_result` block: extract the sidecar's primary payload text (try `toolUseResult.stdout`, then `.originalFile`, then `.content`, then `.file.content` — the first present string) and the message tool_result's text (reuse the existing text-extraction helper). If they are byte-equal AND the sidecar payload is large enough that replacing it saves space (never-inflate): replace `record["toolUseResult"]` with `json!({"contextzip_ref":"message.content","contextzip_compressed":{"axis":"SidecarDedup"}})`. Increment `sidecars_deduped`. If they differ, leave `toolUseResult` untouched. Idempotency: skip if `toolUseResult.contextzip_ref` already present.
- Keep it conservative: only collapse when byte-equal. Do NOT collapse partial matches.

- [ ] **Step 4: Run** — expect PASS.

- [ ] **Step 5: Commit** `feat(compact): SidecarDedup axis (aggressive) collapses duplicate toolUseResult`

---

### Task 5: McpJsonCompact axis

**Files:**
- Modify: `src/jsonl_rewriter.rs` (`rewrite_record` — the tool_result content match; `CompactStats`)

**Interfaces:**
- Produces: `CompactStats.mcp_results_compacted: usize`. Applies to `tool_result` blocks whose tool name starts with `mcp__`.

- [ ] **Step 1: Write the failing test**
```rust
#[test]
fn mcp_json_compacted_when_aggressive() {
    // double-encoded {"result":"{...}"} with whitespace
    let inner = r#"{  "email" : "a@b.com" ,  "team" : "Pacifico"  }"#;
    let payload = serde_json::json!({"result": inner}).to_string();
    let records = [
        make_assistant_tool("m1", "mcp__sts__ping", json!({})),
        make_user_tool_result("m1", &payload),
    ];
    let on = CompactConfig { aggressive: true, ..Default::default() };
    let (out, s) = compact_session_str(&jsonl(&records), &on);
    assert_eq!(s.mcp_results_compacted, 1);
    // minified: no double-space, inner unwrapped
    assert!(!out.contains("  \"email\""));
    // aggressive OFF -> untouched by MCP axis (may still hit GenericResultCap if huge, but this is small)
    let (out_off, s_off) = compact_session_str(&jsonl(&records), &CompactConfig::default());
    assert_eq!(s_off.mcp_results_compacted, 0);
}
```

- [ ] **Step 2: Run** — expect FAIL.

- [ ] **Step 3: Implement.** In `rewrite_record`'s tool-name match, add an arm (guarded by `cfg.aggressive`) for names starting with `mcp__`, placed so it does not shadow the dedicated Read/Grep/Glob/Bash arms (MCP names never collide with those). In the arm: extract the tool_result text; try `serde_json::from_str::<Value>`; if it parses AND has a single string `result` field that itself parses as JSON, unwrap it (replace outer with the inner parsed value); then re-serialize compact (no whitespace). NEVER-INFLATE: only replace if smaller. Annotate `contextzip_compressed{axis:"McpJsonCompact", original_chars, compressed_chars}`. Idempotency: skip if already annotated. If the text is not valid JSON, leave it (fallback-safe).
- Note: `rewrite_record` must now receive/consult `cfg.aggressive` (it already receives `cfg`).

- [ ] **Step 4: Run** — expect PASS.

- [ ] **Step 5: Commit** `feat(compact): McpJsonCompact axis (aggressive) minifies and unwraps MCP JSON`

---

### Task 6: expand restores v3 axes + full gate + resume test

**Files:**
- Modify: `src/jsonl_rewriter.rs` module doc (document the four aggressive axes + gate)
- Verify + manual resume test.

**Interfaces:** none new.

- [ ] **Step 1: Confirm expand round-trips.** v3 axes are reversible via `.bak` (the original bytes). Add one integration-style test in `src/compact_cmd.rs` tests: build a session with a thinking signature + a base64 image + a duplicate sidecar, run `run_with_options(..., aggressive=true, ...)` then `run_apply` then `run_expand`, and assert the expanded file scrubbed-equals the original (modulo redaction, mirroring `expand_restores_original_modulo_redaction`). This proves `.bak` restores the aggressive-compacted session.

- [ ] **Step 2: Update module doc.** Extend the `//!` header of `jsonl_rewriter.rs` to list the four aggressive axes (SidecarDedup, MediaReference, SignatureDrop, McpJsonCompact), state they are gated behind `--aggressive`/`compact.aggressive` and default off, and note they touch Claude-Code-internal fields (safe to `compact`, resume-tested before `apply`).

- [ ] **Step 3: Full gate.** Run `cargo test --all`, `cargo clippy -- -D warnings`, `cargo fmt --check`. All clean.

- [ ] **Step 4: MANDATORY resume test (gating).** Copy a real large session to a temp dir. Run the debug binary `cargo run -- compact <copy> --aggressive` then `... apply <copy>`. Then verify Claude Code can still open/resume that session (either: open it in the Claude Code UI and confirm it loads without error, OR run a JSONL-structural validation that every record still parses and the uuid/parentUuid chain is intact). Record the result in the commit message. If resume BREAKS on any axis, that axis must be documented as apply-unsafe and the plan escalates to the human. `expand` must fully restore regardless — verify it does.

- [ ] **Step 5: Commit** `docs(compact): document v3 aggressive axes; v3 complete` with the resume-test result noted.

---

## Self-Review

**Spec coverage:** SidecarDedup -> Task 4; MediaReference -> Task 3; SignatureDrop -> Task 2; McpJsonCompact -> Task 5; aggressive gate -> Task 1 (+ every axis guarded); reversibility/`.bak` -> Task 6 Step 1; resume-test gating step -> Task 6 Step 4; module doc -> Task 6 Step 2.

**Placeholder scan:** All steps carry real code and runnable commands. Test data (signatures, base64 stand-ins) assembled inline; MCP double-encoding built with `serde_json::json!`. No literal secrets.

**Type consistency:** `CompactConfig.aggressive` introduced Task 1, consumed Tasks 2-5. `rewrite_record_metadata(record, cfg, stats)` defined Task 2, extended Tasks 3-4. `CompactStats` new fields (`signatures_dropped`, `media_referenced`, `sidecars_deduped`, `mcp_results_compacted`) each introduced once. `run_with_options` gains `aggressive` in Task 1 — all call sites updated there.

**Known risks:** (1) The resume test (Task 6 Step 4) is the real gate on whether the aggressive axes are `apply`-safe; if Claude Code reads `toolUseResult`/`signature`/`source.data` on resume, some axes may need to stay compact-only. `.bak`/`expand` always recovers. (2) MediaReference/SidecarDedup operate on top-level fields outside `message.content` — the metadata rewriter must handle records that have `toolUseResult` but no message content, and vice versa, without panicking (guard every `get`).
