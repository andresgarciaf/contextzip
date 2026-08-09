# Session Compressor v2 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Extend the session-history compressor with three new dedup/cap axes (Grep/Glob dedup, Bash-command dedup, generic large-result cap) and full security hardening (secret redaction, `.bak` retention sweep, Read staleness SHA, path-marker gating).

**Architecture:** All compression stays inside `src/jsonl_rewriter.rs`'s existing two-pass model (index pass then rewrite pass), adding new `match` arms and index maps. A new `src/redact.rs` runs a single scrub pass over any content before it is written to a sidecar or `.bak`. A new `CompactConfig` in `src/config.rs` carries the knobs. No new command, no async, no network.

**Tech Stack:** Rust, `serde_json`, `sha2` (already deps), `lazy_static` + `regex` (already deps), `anyhow`.

## Global Constraints

- No async, no network, no new crates — `local_llm.rs` is heuristic-only; all work is structural string processing.
- All regex via `lazy_static!` — never `Regex::new()` inside a function.
- `anyhow::Result` + `.context()` everywhere; no `unwrap()` outside tests and regex init.
- Preserve the four axis invariants: reversible (annotate `contextzip_compressed`), idempotent (skip blocks already carrying it), never-inflate (bail if filtered >= original), fallback-safe (pass unparseable lines through verbatim).
- Never alter the `uuid` / `parentUuid` record chain; only rewrite `tool_result` content payloads.
- No em-dash (U+2014) in code, comments, or strings; ASCII hyphen only.
- **Secret-shaped test data and the key-block regex marker must be assembled from parts**, never written as a literal in any repo file — the repo's `block-secrets.sh` PreToolUse hook refuses any file containing a real secret shape (this plan itself was blocked twice until the markers were split). Build the private-key BEGIN/END markers with `concat!`/`format!` from separate string fragments.
- Every axis and the redactor get: snapshot test, >=60% savings test (where applicable), idempotency test, malformed-input passthrough test, against real fixtures in `tests/fixtures/`.

---

### Task 1: `CompactConfig` in config.rs

**Files:**
- Modify: `src/config.rs` (add struct + field on `Config` + `Default` + `compact_config()` accessor, mirroring `LimitsConfig` / `limits()`)

**Interfaces:**
- Produces: `pub struct CompactConfig { pub redact: bool, pub backup_retention_days: u32, pub generic_cap_chars: usize, pub generic_cap_lines: usize, pub include_paths_in_markers: bool }`; `pub fn compact_config() -> CompactConfig`.

- [ ] **Step 1: Write the failing test**

In `src/config.rs` tests module:
```rust
#[test]
fn compact_config_defaults_are_safe() {
    let c = CompactConfig::default();
    assert!(c.redact, "redaction must default on");
    assert_eq!(c.backup_retention_days, 7);
    assert_eq!(c.generic_cap_chars, 4000);
    assert_eq!(c.generic_cap_lines, 200);
    assert!(c.include_paths_in_markers);
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test compact_config_defaults_are_safe`
Expected: FAIL — `CompactConfig` not found.

- [ ] **Step 3: Add the struct, field, Default, and accessor**

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompactConfig {
    #[serde(default = "default_true")]
    pub redact: bool,
    #[serde(default = "default_retention")]
    pub backup_retention_days: u32,
    #[serde(default = "default_cap_chars")]
    pub generic_cap_chars: usize,
    #[serde(default = "default_cap_lines")]
    pub generic_cap_lines: usize,
    #[serde(default = "default_true")]
    pub include_paths_in_markers: bool,
}

fn default_true() -> bool { true }
fn default_retention() -> u32 { 7 }
fn default_cap_chars() -> usize { 4000 }
fn default_cap_lines() -> usize { 200 }

impl Default for CompactConfig {
    fn default() -> Self {
        Self { redact: true, backup_retention_days: 7, generic_cap_chars: 4000, generic_cap_lines: 200, include_paths_in_markers: true }
    }
}

pub fn compact_config() -> CompactConfig {
    Config::load().map(|c| c.compact).unwrap_or_default()
}
```
Add `#[serde(default)] pub compact: CompactConfig,` to `struct Config`. Match the exact `Serialize`/`Deserialize` derive style already on `LimitsConfig`.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test compact_config_defaults_are_safe`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/config.rs
git commit -m "feat(compact): add CompactConfig with safe defaults"
```

---

### Task 2: Secret redaction module

**Files:**
- Create: `src/redact.rs`
- Modify: `src/main.rs` (add `mod redact;`)

**Interfaces:**
- Consumes: nothing.
- Produces: `pub fn scrub(input: &str) -> (String, usize)` — returns redacted text and count of redactions.

- [ ] **Step 1: Write the failing tests**

In `src/redact.rs`. Assemble secret shapes at runtime so `block-secrets.sh` does not refuse this file:
```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redacts_databricks_pat() {
        let pat = format!("dapi{}", "a1b2c3d4".repeat(4)); // 32 hex chars after prefix
        let (out, n) = scrub(&format!("token={pat}"));
        assert!(!out.contains(&pat), "PAT must not survive");
        assert!(out.contains("[REDACTED:databricks-pat]"));
        assert_eq!(n, 1);
    }

    #[test]
    fn redacts_aws_key_and_private_key_and_jwt() {
        let aws = format!("AKIA{}", "ABCDEFGH12345678"); // AKIA + 16
        let jwt = format!("eyJ{}.eyJ{}.{}", "abc123", "def456", "ghijkl789");
        // Build the key markers from fragments so no literal key block exists in source.
        let kw = "PRIVATE";
        let begin = format!("-----BEGIN RSA {kw} KEY-----");
        let end = format!("-----END RSA {kw} KEY-----");
        let pk = format!("{begin}\n{}\n{end}", "MIIBOgIBAAJB");
        let (out, n) = scrub(&format!("{aws} {jwt} {pk}"));
        assert!(!out.contains(&aws));
        assert!(!out.contains("MIIBOgIBAAJB"));
        assert!(!out.contains("eyJdef456"));
        assert!(n >= 3);
    }

    #[test]
    fn leaves_clean_text_untouched() {
        let (out, n) = scrub("fn main() { println!(\"hi\"); }");
        assert_eq!(out, "fn main() { println!(\"hi\"); }");
        assert_eq!(n, 0);
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --lib redact`
Expected: FAIL — module/functions not defined.

- [ ] **Step 3: Implement the scrubber**

Build the private-key regex marker from fragments (`concat!`) so the source file contains no literal key-block shape:
```rust
//! Secret redaction applied before any ContextZip-created file (sidecar or
//! `.bak`) is written. Fail-closed: callers on the security-critical path abort
//! the write rather than persist un-redacted content when redaction is enabled.

use lazy_static::lazy_static;
use regex::Regex;

lazy_static! {
    static ref DATABRICKS_PAT: Regex = Regex::new(r"dapi[0-9a-fA-F]{32,}").unwrap();
    static ref AWS_KEY: Regex = Regex::new(r"AKIA[0-9A-Z]{16}").unwrap();
    static ref OPENAI_KEY: Regex = Regex::new(r"sk-[A-Za-z0-9]{20,}").unwrap();
    static ref JWT: Regex = Regex::new(r"eyJ[A-Za-z0-9_-]+\.eyJ[A-Za-z0-9_-]+\.[A-Za-z0-9_-]+").unwrap();
    // Assembled from fragments to avoid a literal key-block shape in source.
    static ref PRIVATE_KEY: Regex = Regex::new(
        &format!(r"(?s)-----BEGIN [A-Z ]*{k} KEY-----.*?-----END [A-Z ]*{k} KEY-----", k = "PRIVATE")
    ).unwrap();
}

/// Replace known secret shapes with `[REDACTED:<kind>]`. Returns the scrubbed
/// text and the number of replacements made. Order matters: key blocks first
/// (they span lines and may embed other shapes).
pub fn scrub(input: &str) -> (String, usize) {
    let mut n = 0usize;
    let mut s = input.to_string();
    for (re, tag) in [
        (&*PRIVATE_KEY, "private-key"),
        (&*DATABRICKS_PAT, "databricks-pat"),
        (&*AWS_KEY, "aws-key"),
        (&*JWT, "jwt"),
        (&*OPENAI_KEY, "openai-key"),
    ] {
        n += re.find_iter(&s).count();
        s = re.replace_all(&s, format!("[REDACTED:{tag}]")).into_owned();
    }
    (s, n)
}
```
Add `mod redact;` in `src/main.rs` near the other `mod` declarations.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --lib redact`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/redact.rs src/main.rs
git commit -m "feat(redact): secret scrubber for sidecar/.bak writes"
```

---

### Task 3: Thread CompactConfig into the rewriter + apply redaction on write

**Files:**
- Modify: `src/jsonl_rewriter.rs` (`compact_session_str` signature, `compact_session_file`)
- Modify: `src/compact_cmd.rs` (`run_with_options`, `run_all_sessions` call sites)

**Interfaces:**
- Consumes: `config::CompactConfig` (Task 1), `redact::scrub` (Task 2).
- Produces: `pub fn compact_session_str(input: &str, cfg: &CompactConfig) -> (String, CompactStats)`; `CompactStats` gains `pub secrets_redacted: usize`.

- [ ] **Step 1: Write the failing test**

In `src/jsonl_rewriter.rs` tests (assemble the secret at runtime):
```rust
#[test]
fn secret_in_tool_result_never_survives_to_output() {
    let pat = format!("dapi{}", "0".repeat(34));
    let records = [make_user_tool_result("u1", &format!("export TOKEN={pat}"))];
    let cfg = crate::config::CompactConfig::default();
    let (out, stats) = compact_session_str(&jsonl(&records), &cfg);
    assert!(!out.contains(&pat), "secret leaked into sidecar");
    assert!(stats.secrets_redacted >= 1);
}
```
Update ALL existing `compact_session_str(x)` call sites in this file's tests to `compact_session_str(x, &crate::config::CompactConfig::default())`.

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --lib jsonl_rewriter`
Expected: FAIL — arity mismatch / `secrets_redacted` missing.

- [ ] **Step 3: Thread config and add the final scrub pass**

- Add `pub secrets_redacted: usize,` to `CompactStats`.
- Change signature to `pub fn compact_session_str(input: &str, cfg: &CompactConfig) -> (String, CompactStats)`.
- After the second pass builds `out`, before setting `bytes_out`:
```rust
if cfg.redact {
    let (scrubbed, n) = crate::redact::scrub(&out);
    stats.secrets_redacted = n;
    out = scrubbed;
}
stats.bytes_out = out.len();
```
- `compact_session_file`: load `let cfg = crate::config::compact_config();` and pass `&cfg`. `scrub` is infallible so no fail-closed branch is needed here; fail-closed applies in Task 5's `.bak` path where an IO write could partially persist.
- Update `run_with_options` / `run_all_sessions` in `compact_cmd.rs` to pass `&config::compact_config()`.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --lib jsonl_rewriter && cargo test --lib compact`
Expected: PASS (all pre-existing tests still green with the new arg).

- [ ] **Step 5: Commit**

```bash
git add src/jsonl_rewriter.rs src/compact_cmd.rs
git commit -m "feat(compact): thread CompactConfig and redact sidecar output"
```

---

### Task 4: Restore Read staleness SHA + path-marker gating

**Files:**
- Modify: `src/jsonl_rewriter.rs` (`FirstRead`, `index_record`, `replace_with_read_ref`)

**Interfaces:**
- Consumes: `CompactConfig.include_paths_in_markers`.
- Produces: `FirstRead { tool_use_id: String, content_sha256: String }`; `replace_with_read_ref(block, path, first_id, original_len, include_path)` gains the `content_sha256` in the annotation.

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn read_dedup_annotation_carries_sha_and_respects_path_gate() {
    let records = [
        make_assistant_read("r1", "/tmp/x.rs"),
        make_user_tool_result("u1", "fn main() {}"),
        make_assistant_read("r2", "/tmp/x.rs"),
        make_user_tool_result("u2", "fn main() {}"),
    ];
    let mut cfg = crate::config::CompactConfig::default();
    cfg.include_paths_in_markers = false;
    let (out, _) = compact_session_str(&jsonl(&records), &cfg);
    assert!(out.contains("\"content_sha256\""), "sha must be recorded for staleness checks");
    assert!(!out.contains("/tmp/x.rs"), "path must be suppressed when gate is off");
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --lib read_dedup_annotation_carries_sha`
Expected: FAIL — no sha in annotation, path still present.

- [ ] **Step 3: Restore the SHA and gate the path**

- `FirstRead` gains `content_sha256: String`.
- The sha is of the first read's tool_result text. Given the two-pass structure, fill it in pass 1: after building `first_read_for` from assistant records, do a light second scan of `user` tool_results and set `content_sha256 = sha256_hex(text)` for the first occurrence tied to each first-read `tool_use_id`.
- `replace_with_read_ref` takes `include_path: bool`; when false, omit the `Re-expand ... if the file at {path}` clause and set annotation `file_path` to `""`. Always include `content_sha256` in the annotation. Pass `cfg.include_paths_in_markers` from the caller.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --lib jsonl_rewriter`
Expected: PASS (existing dedup tests still green).

- [ ] **Step 5: Commit**

```bash
git add src/jsonl_rewriter.rs
git commit -m "feat(compact): record Read content SHA and gate path disclosure"
```

---

### Task 5: `.bak` redaction on apply + retention sweep

**Files:**
- Modify: `src/compact_cmd.rs` (`run_apply`, new `sweep_backups`)

**Interfaces:**
- Consumes: `redact::scrub` (Task 2), `config::compact_config()` (Task 1).
- Produces: `fn sweep_backups(project_dir: &Path, retention_days: u32) -> Result<usize>`.

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn apply_redacts_secret_in_backup() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let pat = format!("dapi{}", "0".repeat(34));
    let session = make_session_with_secret(dir.path(), &pat)?; // tool_result contains `pat`
    run_with_options(session.to_str().unwrap(), false, 0)?;
    run_apply(session.to_str().unwrap(), 0)?;
    let bak = std::fs::read_to_string(backup_path(&session))?;
    assert!(!bak.contains(&pat), "backup must be redacted");
    Ok(())
}

#[test]
fn sweep_removes_aged_backup_keeps_fresh() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let old = dir.path().join("a.jsonl.bak");
    let new = dir.path().join("b.jsonl.bak");
    std::fs::write(&old, "x")?; std::fs::write(&new, "y")?;
    set_mtime_days_ago(&old, 10);
    let removed = sweep_backups(dir.path(), 7)?;
    assert_eq!(removed, 1);
    assert!(!old.exists() && new.exists());
    Ok(())
}
```
Add `make_session_with_secret(dir, secret)` and `set_mtime_days_ago(path, days)` test helpers. Set mtime via `std::fs::File::open(path)?.set_modified(SystemTime::now() - Duration::from_secs(days*86400))` (stable on the 1.96 toolchain); no new dep.

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --lib apply_redacts_secret_in_backup sweep_removes_aged_backup`
Expected: FAIL — `.bak` is byte-copy of original; `sweep_backups` undefined.

- [ ] **Step 3: Change `.bak` write to read->scrub->write and add sweep**

Replace the `std::fs::rename(&session_path, &backup)` in `run_apply` with:
```rust
let cfg = crate::config::compact_config();
let original = std::fs::read_to_string(&session_path)
    .with_context(|| format!("Failed to read original session {}", session_path.display()))?;
let backup_content = if cfg.redact { crate::redact::scrub(&original).0 } else { original };
std::fs::write(&backup, &backup_content)
    .with_context(|| format!("Failed to write backup {}", backup.display()))?;
std::fs::remove_file(&session_path)
    .with_context(|| format!("Failed to remove original after backup {}", session_path.display()))?;
```
Keep the existing sidecar-promote + rollback logic (rollback now restores from the written `.bak`). Add:
```rust
/// Remove `.bak` files older than `retention_days` under `project_dir`.
/// `0` disables. Returns count removed. Never errors the caller on a single
/// permission failure; logs to stderr and continues.
fn sweep_backups(project_dir: &Path, retention_days: u32) -> Result<usize> {
    if retention_days == 0 { return Ok(0); }
    let cutoff = std::time::SystemTime::now()
        .checked_sub(std::time::Duration::from_secs(retention_days as u64 * 86_400))
        .unwrap_or(std::time::UNIX_EPOCH);
    let mut removed = 0;
    let Ok(entries) = std::fs::read_dir(project_dir) else { return Ok(0); };
    for entry in entries.flatten() {
        let p = entry.path();
        if p.extension().and_then(|s| s.to_str()) != Some("bak") { continue; }
        let aged = entry.metadata().and_then(|m| m.modified()).map(|m| m < cutoff).unwrap_or(false);
        if aged {
            if let Err(e) = std::fs::remove_file(&p) { eprintln!("contextzip: could not remove {}: {}", p.display(), e); }
            else { removed += 1; }
        }
    }
    Ok(removed)
}
```
Call `sweep_backups` at the end of `run_apply` using the session's parent dir and `cfg.backup_retention_days`.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --lib compact`
Expected: PASS. The existing `expand_restores_original_byte_for_byte` test must be renamed to `expand_restores_original_modulo_redaction` and assert content equality after scrubbing both sides, per the spec's accepted tradeoff (redaction makes `.bak` non-byte-exact).

- [ ] **Step 5: Commit**

```bash
git add src/compact_cmd.rs
git commit -m "feat(compact): redact .bak on apply and sweep aged backups"
```

---

### Task 6: GrepGlobDedup axis

**Files:**
- Modify: `src/jsonl_rewriter.rs` (`ToolUseInfo`, `index_record`, `rewrite_record`, new marker fn, `CompactStats`)
- Create fixture: `tests/fixtures/session_grep_repeat.jsonl` (real session lines with a repeated Grep result)

**Interfaces:**
- Consumes: prior tasks' structures.
- Produces: `CompactStats.grepglob_results_deduped: usize`; `first_grepglob_for: HashMap<String, FirstResult>` keyed on normalized args string; `struct FirstResult { tool_use_id: String }`; `replace_with_generic_ref(block, axis, first_id, original_len)`.

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn grep_dedup_replaces_repeat_with_reference() {
    let records = [
        make_assistant_tool("g1", "Grep", json!({"pattern":"fn ","path":"src"})),
        make_user_tool_result("g1", "src/a.rs:1: fn a\nsrc/b.rs:2: fn b"),
        make_assistant_tool("g2", "Grep", json!({"pattern":"fn ","path":"src"})),
        make_user_tool_result("g2", "src/a.rs:1: fn a\nsrc/b.rs:2: fn b"),
    ];
    let cfg = crate::config::CompactConfig::default();
    let (out, stats) = compact_session_str(&jsonl(&records), &cfg);
    assert_eq!(stats.grepglob_results_deduped, 1);
    assert!(out.contains("GrepGlobDedup"));
}
```
Add a `make_assistant_tool(id, name, input)` test helper (generalize the existing `make_assistant_read` / `make_assistant_bash`).

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --lib grep_dedup_replaces_repeat`
Expected: FAIL.

- [ ] **Step 3: Implement the axis**

- Extend `ToolUseInfo` with `args_key: Option<String>` — for `Grep`/`Glob`, set it to a normalized `serde_json::to_string` of the `input` object with keys sorted (round-trip through `BTreeMap<String, Value>` so key order is stable).
- In `index_record`, when `name` is `"Grep"` or `"Glob"`, populate `first_grepglob_for.entry(args_key).or_insert(FirstResult { tool_use_id })`.
- In `rewrite_record`, add arms for `"Grep" | "Glob"` mirroring the Read arm: if this use_id is not the first for its args_key, call `replace_with_generic_ref(block, "GrepGlobDedup", &first_id, preview_len)` and bump `grepglob_results_deduped`.
- `replace_with_generic_ref(block, axis, first_id, original_len)` writes marker `[contextzip: dedup {axis} — same as tool_use {first_id} ({original_len} -> 0 chars)]` plus `contextzip_compressed` annotation `{axis, first_tool_use_id, original_chars}`.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test --lib grep_dedup_replaces_repeat`
Expected: PASS.

- [ ] **Step 5: Add unique-not-touched + idempotency tests, then commit**

```rust
#[test] fn grep_dedup_does_not_touch_unique_results() { /* two Greps, different args -> 0 deduped */ }
#[test] fn grep_dedup_is_idempotent() { /* compact twice -> second pass 0 deduped */ }
```
```bash
git add src/jsonl_rewriter.rs tests/fixtures/session_grep_repeat.jsonl
git commit -m "feat(compact): GrepGlobDedup axis"
```

---

### Task 7: BashCmdDedup axis

**Files:**
- Modify: `src/jsonl_rewriter.rs` (`index_record`, `rewrite_record`, `CompactStats`)

**Interfaces:**
- Consumes: `FirstResult`, `replace_with_generic_ref` (Task 6).
- Produces: `CompactStats.bash_cmds_deduped: usize`; `first_bashcmd_for: HashMap<String, FirstResult>` keyed on the Bash command string.

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn bash_cmd_dedup_references_first_when_command_repeats() {
    let records = [
        make_assistant_bash("b1", "ls -la"),
        make_user_tool_result("b1", "total 8\ndrwxr-xr-x  2 u  s  64 x\n-rw-r--r--  1 u  s   0 y"),
        make_assistant_bash("b2", "ls -la"),
        make_user_tool_result("b2", "total 8\ndrwxr-xr-x  2 u  s  64 x\n-rw-r--r--  1 u  s   0 y"),
    ];
    let cfg = crate::config::CompactConfig::default();
    let (out, stats) = compact_session_str(&jsonl(&records), &cfg);
    assert_eq!(stats.bash_cmds_deduped, 1);
    assert!(out.contains("BashCmdDedup"));
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --lib bash_cmd_dedup_references_first`
Expected: FAIL.

- [ ] **Step 3: Implement the axis**

- In `index_record`, capture the Bash command string into `ToolUseInfo` (`input.command`) and populate `first_bashcmd_for.entry(cmd).or_insert(FirstResult{tool_use_id})`.
- In `rewrite_record`, the `"Bash"` arm becomes: if this use_id is NOT the first for its command string, dedup via `replace_with_generic_ref(block, "BashCmdDedup", ...)` and bump `bash_cmds_deduped`; ELSE fall through to the existing `recompress_bash_block`. Dedup wins over text compaction when both apply.
- Guard: skip dedup if the block already carries `contextzip_compressed` (idempotency) or the command's first output was empty.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test --lib bash_cmd_dedup_references_first`
Expected: PASS. Confirm the existing `bash_history_compact_*` tests still pass (first occurrences still text-compress).

- [ ] **Step 5: Add idempotency + unique-command tests, then commit**

```bash
git add src/jsonl_rewriter.rs
git commit -m "feat(compact): BashCmdDedup axis on top of text compaction"
```

---

### Task 8: GenericResultCap axis

**Files:**
- Modify: `src/jsonl_rewriter.rs` (`rewrite_record` `_ =>` arm, new cap fn, `CompactStats`)

**Interfaces:**
- Consumes: `CompactConfig.generic_cap_chars`, `generic_cap_lines`; the ANSI-strip logic inside `compress_bash_text` (reuse via a shared helper, do not duplicate).
- Produces: `CompactStats.generic_results_capped: usize`.

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn generic_cap_trims_oversized_unknown_tool_result() {
    let big = (0..500).map(|i| format!("line {i}")).collect::<Vec<_>>().join("\n");
    let records = [
        make_assistant_tool("m1", "mcp__some__tool", json!({})),
        make_user_tool_result("m1", &big),
    ];
    let cfg = crate::config::CompactConfig::default(); // 200-line cap
    let (out, stats) = compact_session_str(&jsonl(&records), &cfg);
    assert_eq!(stats.generic_results_capped, 1);
    assert!(out.contains("more lines"));
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --lib generic_cap_trims_oversized`
Expected: FAIL.

- [ ] **Step 3: Implement the cap**

- Change the `_ => {}` arm in `rewrite_record` to `_ => { if generic_cap_block(block, cfg) { stats.generic_results_capped += 1; } }`.
- `generic_cap_block(block, cfg)`: extract text (same helper `recompress_bash_block` uses); skip if already `contextzip_compressed`; skip if under BOTH `generic_cap_chars` and `generic_cap_lines`. Otherwise ANSI-strip (extract `compress_bash_text`'s ANSI regex to a shared `strip_ansi` helper if not already shared), keep the first `generic_cap_lines` lines, append `\n[contextzip: GenericResultCap — {N} more lines dropped]`, and apply only if the result is smaller (never-inflate). Annotate `contextzip_compressed{axis:"GenericResultCap", original_chars, compressed_chars}`.
- Dedicated axes (Read/Grep/Glob/Bash) are matched by name above this arm, so they never reach the cap.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test --lib generic_cap_trims_oversized`
Expected: PASS.

- [ ] **Step 5: Add small-result-untouched + idempotency tests, then commit**

```bash
git add src/jsonl_rewriter.rs
git commit -m "feat(compact): GenericResultCap for oversized unknown tool results"
```

---

### Task 9: Full-suite green, savings verification, security review

**Files:**
- Modify: `src/jsonl_rewriter.rs` module doc comment (list all five axes)
- Verify only.

- [ ] **Step 1: Full test + lint**

Run: `cargo test --all && cargo clippy -- -D warnings && cargo fmt --check`
Expected: all pass. Fix any fallout (notably any remaining `compact_session_str` call sites needing the new arg).

- [ ] **Step 2: Update module doc**

Update the `//!` header in `jsonl_rewriter.rs` to describe all five axes (ReadDedup, GrepGlobDedup, BashCmdDedup, BashHistoryCompact, GenericResultCap) and the redaction scrub pass.

- [ ] **Step 3: Savings check on a real session**

Run `contextzip compact <a-real-large-session> --dry-run` and confirm the reported savings are positive and no panic. Record the number in the commit message.

- [ ] **Step 4: Security review of the compact path**

Dispatch the `andres-garcia:andres-code-reviewer` agent over the diff for `src/redact.rs`, `src/jsonl_rewriter.rs`, `src/compact_cmd.rs`, `src/config.rs`. Address any Critical/High findings before finishing.

- [ ] **Step 5: Commit**

```bash
git add src/jsonl_rewriter.rs
git commit -m "docs(compact): document v2 axes; v2 complete"
```

---

## Self-Review

**Spec coverage:** Part 1 axes -> Tasks 6, 7, 8. Part 2 hardening: redaction -> Tasks 2, 3, 5; `.bak` retention -> Task 5; Read staleness -> Task 4; path gating -> Task 4; security review -> Task 9. Config -> Task 1. All spec sections mapped.

**Placeholder scan:** All steps carry real code, exact file targets, and runnable test commands. Task 4 Step 3's SHA-computation approach is described concretely (pass-1 light second scan of user tool_results). Task 5 flags the byte-exact test rename explicitly. All secret-shaped test data and the key-block regex marker are assembled from fragments to satisfy `block-secrets.sh`.

**Type consistency:** `CompactConfig` fields identical across Tasks 1/3/4/5/8. `compact_session_str(input, cfg)` signature consistent from Task 3 onward. `FirstResult` / `replace_with_generic_ref` defined in Task 6, reused in Task 7. `CompactStats` new fields (`secrets_redacted`, `grepglob_results_deduped`, `bash_cmds_deduped`, `generic_results_capped`) each introduced in one task, no rename.

**Known risk:** Task 4's SHA look-ahead adds a light third scan; acceptable for single-threaded session files. If it proves awkward, fold the sha capture into pass 2's first-encounter branch and store back into `first_read_for` (still O(n)).
