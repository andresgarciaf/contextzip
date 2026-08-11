# Effectiveness + Hygiene Pass Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Prove and raise the token savings ContextZip delivers on daily-driver commands, eliminate the negative-savings (inflation) class outright, and remove fork-era dead code.

**Architecture:** Four sequenced phases — (1) port + expand the benchmark harness to produce reproducible per-command savings data; (2) add one shared never-inflate guard on the output path; (3) deepen the high-frequency filters the data proves weak; (4) delete dead code and broken tooling. Phase 1 data drives Phase 3. Phase 4 is independent.

**Tech Stack:** Rust (Clap 4, lazy_static, regex, anyhow, rusqlite), bash benchmark harness, `insta` snapshots.

## Global Constraints

Copied verbatim from repo conventions (CLAUDE.md, rust-patterns.md, cli-testing.md) — every task's requirements implicitly include these:

- No async. No `tokio`/`async-std`/`futures`. Single-threaded.
- No `unwrap()` in production code — use `.context("...")?`. Tests use `expect("reason")`.
- All regex via `lazy_static!`, never `Regex::new()` inside a function.
- Fallback pattern: if a filter fails, pass through raw output. Never block the user.
- Propagate exit codes: `std::process::exit(code)` when the underlying command fails.
- `cargo build --release`, `cargo test`, and `cargo clippy -- -D warnings` must be green after every task.
- No em-dash (U+2014) in code, comments, or CLI strings — use ASCII `-`.
- Fixtures are REAL captured command output in `tests/fixtures/<cmd>/`, never synthetic (per cli-testing.md).
- Savings assertions use `count_tokens(s) = s.split_whitespace().count()`; target `>= 60.0`.
- Never write literal secret shapes into any file (block-secrets hook) — assemble from fragments if a test needs one.
- Binary is `contextzip`; the SQLite `feature` column tags which module produced savings.

---

## Task 1: Port the benchmark harness to `contextzip`

**Files:**
- Modify: `scripts/benchmark.sh` (binary resolution lines 4-13, all `$RTK`/`rtk` refs, `bench_rewrite` expected strings lines 553-558)

**Interfaces:**
- Produces: a runnable `./scripts/benchmark.sh` that drives `./target/release/contextzip` and prints the per-test savings table + global summary it already prints.

- [ ] **Step 1: Repoint the binary resolution block**

Replace lines 4-13 so it resolves `contextzip`:

```bash
# Use local release build if available, otherwise fall back to installed contextzip
if [ -f "./target/release/contextzip" ]; then
  CONTEXTZIP="$(cd "$(dirname ./target/release/contextzip)" && pwd)/$(basename ./target/release/contextzip)"
elif command -v contextzip &> /dev/null; then
  CONTEXTZIP="$(command -v contextzip)"
else
  echo "Error: contextzip not found. Run 'cargo build --release' or install contextzip."
  exit 1
fi
BENCH_DIR="$(pwd)/scripts/benchmark"
```

- [ ] **Step 2: Replace every `$RTK` with `$CONTEXTZIP` and every `rtk ` invocation string**

Global replace in the file: `$RTK` -> `$CONTEXTZIP`. In `bench_rewrite` expectations (lines 553-558), replace the expected `rtk ...` strings with `contextzip ...` (the rewrite output now emits `contextzip`, confirm against `src/discover/registry.rs` `rewrite_command`). Change the banner `echo "RTK Benchmark"` to `echo "ContextZip Benchmark"`.

- [ ] **Step 3: Build and run**

Run: `cargo build --release && ./scripts/benchmark.sh`
Expected: harness runs to the global summary line `Tokens: <N> -> <M>` without `Error: contextzip not found`. Some sections skip (no docker/kubectl/gh) — that is fine. The `rewrite` section must show ✅ for all 6 `bench_rewrite` cases.

- [ ] **Step 4: Commit**

```bash
git add scripts/benchmark.sh
git commit -m "chore(bench): port benchmark harness rtk -> contextzip

Co-authored-by: Isaac"
```

---

## Task 2: Add fixture-driven bench cases for uncovered high-value commands

**Files:**
- Create: `tests/fixtures/aws/describe_instances.json`, `tests/fixtures/terraform/plan.txt`, `tests/fixtures/mypy/errors.txt`, `tests/fixtures/docker/build.txt`, `tests/fixtures/kubectl/get_pods.txt` (real captured output — see note)
- Modify: `scripts/benchmark.sh` (add a `fixtures` section that pipes each fixture through the matching `contextzip` subcommand)

**Interfaces:**
- Consumes: `$CONTEXTZIP` from Task 1.
- Produces: bench cases that run without the external binary installed, so CI reproduces them.

> **Fixture capture note:** Capture real output where the tool is available
> (`terraform plan > tests/fixtures/terraform/plan.txt`), otherwise paste a
> representative real sample you have on hand. Do NOT hand-synthesize minimal
> fake output — cli-testing.md forbids it. If a tool's real output cannot be
> obtained, skip that fixture and note it in the results doc rather than faking it.

- [ ] **Step 1: Capture or place fixtures**

For each command that reads a file/stdin, capture real output into the fixture path. Verify each is non-trivial (> 40 lines / realistic size).

- [ ] **Step 2: Add a `fixtures` section to the harness**

After the existing sections, before the global summary, add cases that feed fixtures through the filter. Pattern (adapt per command's stdin/file interface):

```bash
section "fixtures"
bench "aws describe (fixture)" "cat tests/fixtures/aws/describe_instances.json" "$CONTEXTZIP aws --stdin < tests/fixtures/aws/describe_instances.json"
bench "mypy (fixture)"         "cat tests/fixtures/mypy/errors.txt"            "cat tests/fixtures/mypy/errors.txt | $CONTEXTZIP mypy -"
```

Confirm each subcommand's real stdin/file flag from its `*_cmd.rs` before wiring (do not guess the flag — read the module's Clap args).

- [ ] **Step 3: Run and confirm the new cases produce a savings number**

Run: `./scripts/benchmark.sh 2>&1 | grep -A8 fixtures`
Expected: each fixture case prints a `<in> -> <out> (<pct>%)` line (GOOD, SKIP, or a negative pct — all acceptable at this stage; we are only establishing measurement).

- [ ] **Step 4: Commit**

```bash
git add tests/fixtures scripts/benchmark.sh
git commit -m "test(bench): fixture-driven cases for aws/terraform/mypy/docker/kubectl

Co-authored-by: Isaac"
```

---

## Task 3: Regenerate `docs/benchmark-results.md` from current code

**Files:**
- Modify: `docs/benchmark-results.md` (full regenerate)

**Interfaces:**
- Consumes: harness output from Tasks 1-2.

- [ ] **Step 1: Capture current numbers**

Run: `./scripts/benchmark.sh > /tmp/cz_bench.txt 2>&1; tail -20 /tmp/cz_bench.txt`
Expected: the global summary and per-test table are captured.

- [ ] **Step 2: Rewrite the doc header and table**

Replace the header block: `**Date:** 2026-08-11`, `**Version:**` = current `Cargo.toml` version, methodology unchanged. Rebuild the per-category table from `/tmp/cz_bench.txt`. Add a ranked classification list: 🟢 `>=60%`, 🟡 `20-60%`, 🔴 `<20% or inflating`, listing each command under its bucket. This 🔴/🟡 list is the Phase 3 work list.

- [ ] **Step 3: Commit**

```bash
git add docs/benchmark-results.md
git commit -m "docs(bench): regenerate results from current contextzip build

Co-authored-by: Isaac"
```

---

## Task 4: Never-inflate guard — shared emit helper

**Files:**
- Modify: `src/tracking.rs` (add `TimedExecution::emit` near `track_with_feature` at line 1244)
- Test: `src/tracking.rs` `#[cfg(test)] mod tests`

**Interfaces:**
- Consumes: existing `TimedExecution` struct, `estimate_tokens`, `print_savings`, `track_with_feature`.
- Produces: `pub fn emit(&self, original_cmd: &str, contextzip_cmd: &str, input: &str, filtered: &str, feature: &str) -> ()` — picks raw input when the filter did not shrink it, writes the chosen text to stdout, then tracks the chosen text. Also `pub fn choose_output<'a>(input: &'a str, filtered: &'a str) -> &'a str` (the pure guard, unit-testable).

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn choose_output_passes_through_when_filter_inflates() {
    let input = "short raw output\n";
    let filtered = "short raw output\nplus filter metadata that makes it bigger\n";
    assert_eq!(choose_output(input, filtered), input);
}

#[test]
fn choose_output_keeps_filtered_when_smaller() {
    let input = "a very long line repeated many times and then some more\n";
    let filtered = "compact\n";
    assert_eq!(choose_output(input, filtered), filtered);
}

#[test]
fn choose_output_ties_go_to_filtered() {
    // Equal length: prefer filtered (already ANSI-stripped/normalized).
    let input = "abcd";
    let filtered = "wxyz";
    assert_eq!(choose_output(input, filtered), filtered);
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test choose_output -- --nocapture`
Expected: FAIL — `choose_output` not found.

- [ ] **Step 3: Implement the guard + emit helper**

Add near line 1298 (after `track_passthrough`), free function in the module:

```rust
/// Never-inflate guard: return the filtered text only if it is strictly
/// smaller than the raw input; otherwise return the raw input. Compared on
/// byte length of the (already ANSI-stripped) input the filter received, so
/// ANSI removal still counts as a win. Ties go to filtered.
pub fn choose_output<'a>(input: &'a str, filtered: &'a str) -> &'a str {
    if filtered.len() < input.len() {
        filtered
    } else if filtered.len() == input.len() {
        filtered
    } else {
        input
    }
}
```

And the `emit` method on `impl TimedExecution` (near line 1269):

```rust
/// Print the better of (raw input, filtered output) to stdout, then track
/// the choice. Enforces the never-inflate guard for every command that
/// routes through it: a filter can never make output larger than raw.
/// `CONTEXTZIP_NO_INFLATE_GUARD=1` disables the guard for debugging.
pub fn emit(
    &self,
    original_cmd: &str,
    contextzip_cmd: &str,
    input: &str,
    filtered: &str,
    feature: &str,
) {
    let chosen = if std::env::var_os("CONTEXTZIP_NO_INFLATE_GUARD").is_some() {
        filtered
    } else {
        choose_output(input, filtered)
    };
    print!("{chosen}");
    self.track_with_feature(original_cmd, contextzip_cmd, input, chosen, feature);
}
```

Simplify the `choose_output` `if`/`else if` into `filtered.len() <= input.len()` once tests pass — kept expanded above only to mirror the three test cases.

- [ ] **Step 4: Run to verify pass**

Run: `cargo test choose_output && cargo clippy -- -D warnings`
Expected: PASS, clippy clean. (Collapse the guard to `if filtered.len() <= input.len() { filtered } else { input }` and re-run.)

- [ ] **Step 5: Commit**

```bash
git add src/tracking.rs
git commit -m "feat(tracking): never-inflate guard + emit helper

Co-authored-by: Isaac"
```

---

## Task 5: Route command output through `emit` (kill the inflation class)

**Files:**
- Modify: every `src/*_cmd.rs` + `src/git.rs`, `src/ls.rs`, `src/grep_cmd.rs`, `src/read.rs`, `src/runner.rs` site matching the `print!(...); timer.track...(...)` pattern (~41 sites)

**Interfaces:**
- Consumes: `TimedExecution::emit` from Task 4.

The uniform pattern to replace at each site:

```rust
// BEFORE
print!("{}", compressed);
timer.track_with_feature(&orig, &cz_cmd, &input, &compressed, "cli");

// AFTER
timer.emit(&orig, &cz_cmd, &input, &compressed, "cli");
```

Sites using `timer.track(...)` (the 4-arg method, feature = "cli") collapse to `emit(..., "cli")`. Sites that add a trailing newline via `println!` instead of `print!` must fold the newline into `compressed` before the call so `emit`'s length comparison and output match.

- [ ] **Step 1: Enumerate the sites**

Run: `contextzip proxy grep -rn "timer.track\|\.track_with_feature" src/ | grep -v tracking.rs | grep -vi test`
Expected: the list of migration sites (~41). Work through them file by file.

- [ ] **Step 2: Migrate each site**

For each: delete the preceding `print!`/`println!` of the filtered text, replace the `track` call with the matching `emit` call, folding any trailing newline into the printed string. Do NOT change sites that use `track_passthrough` (streaming/interactive commands have no captured output to guard).

- [ ] **Step 3: Build + full test suite**

Run: `cargo build --release && cargo test && cargo clippy -- -D warnings`
Expected: green. No `print!` of filtered output remains directly before a `track` call (spot-check with the Step 1 grep — should now be empty of `timer.track` at migrated sites).

- [ ] **Step 4: Prove the inflation class is gone**

Run: `./scripts/benchmark.sh > /tmp/cz_after.txt 2>&1; grep -E "\(-[0-9]+%\)" /tmp/cz_after.txt || echo "NO NEGATIVE SAVINGS"`
Expected: `NO NEGATIVE SAVINGS` (previously-negative cases `ls src/`, java, npm-small now show `(+0%)` via passthrough).

- [ ] **Step 5: Commit**

```bash
git add src/
git commit -m "refactor: route command output through emit (never-inflate guard live)

Co-authored-by: Isaac"
```

---

## Task 6: Deepen high-frequency under-performers (data-driven)

**Files:**
- Modify: the specific `src/*_cmd.rs` / filter modules that Task 3's ranked list marks 🟡 among the daily-driver set (git, grep, read, docker, gh, pytest). Confirm the exact set from `docs/benchmark-results.md`; do not assume.
- Create: `tests/fixtures/<cmd>/...` where a deepened filter lacks a fixture
- Test: savings assertion in each touched module's `#[cfg(test)] mod tests`

**Interfaces:**
- Consumes: Task 3 ranking; Task 4 guard (a deepened filter can never inflate).

> Only touch a filter this task if BOTH: (a) it is in the daily-driver set, AND
> (b) Task 3 marked it 🟡/🔴. If a daily-driver command is already 🟢, leave it
> and record "already optimal" in the results doc. This task has no fixed count
> of edits — it is bounded by the data.

- [ ] **Step 1: For each qualifying filter, write the failing savings test**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    fn count_tokens(s: &str) -> usize { s.split_whitespace().count() }

    #[test]
    fn test_savings_meets_bar() {
        let input = include_str!("../tests/fixtures/<cmd>/real.txt");
        let output = filter_<cmd>(input);
        let savings = 100.0 - (count_tokens(&output) as f64 / count_tokens(input) as f64 * 100.0);
        assert!(savings >= 60.0, "expected >=60% savings, got {savings:.1}%");
    }
}
```

- [ ] **Step 2: Run to verify it fails at current savings**

Run: `cargo test test_savings_meets_bar`
Expected: FAIL showing the current sub-60% number (this is the baseline being fixed).

- [ ] **Step 3: Deepen the filter**

Extend the filter's noise removal (dedupe repeated frames/lines, drop boilerplate) following the module's existing `lazy_static!` regex style. If after honest effort a filter cannot reach 60% because its input has no more removable noise, revert the assertion to the achievable bar, add a `// ponytail: <cmd> input is near-optimal, N% is the real ceiling` comment, and note it in the results doc.

- [ ] **Step 4: Run to verify pass**

Run: `cargo test && cargo clippy -- -D warnings && ./scripts/benchmark.sh 2>&1 | grep <cmd>`
Expected: assertion passes (or documented ceiling), benchmark line improved, clippy clean.

- [ ] **Step 5: Commit (one commit per deepened filter)**

```bash
git add src/<cmd>_cmd.rs tests/fixtures/<cmd>
git commit -m "perf(<cmd>): deepen filter to meet savings bar

Co-authored-by: Isaac"
```

---

## Task 7: Dead-code audit — resolve every `#[allow(dead_code)]`

**Files:**
- Modify: the 17 files carrying `#[allow(dead_code)]` (47 markers total). Highest counts: `src/parser/types.rs` (9), `src/ruff_cmd.rs` (6), `src/parser/mod.rs` (6), `src/tracking.rs` (5), `src/lint_cmd.rs` (5)

**Interfaces:** none — pure cleanup.

- [ ] **Step 1: Enumerate the markers**

Run: `contextzip proxy grep -rn "allow(dead_code)" src/`
Expected: 47 lines. For each, determine: (a) genuinely unused -> delete the item + marker; (b) should be wired but isn't -> wire it and delete marker; (c) genuinely pending (e.g. a struct field a planned axis needs) -> keep marker, add a one-line `// pending: <why>` comment.

- [ ] **Step 2: Remove the deprecated `track` free function**

`src/tracking.rs:1346-1355` — the `#[deprecated]`+`#[allow(dead_code)]` free `track()`. Confirm zero non-test callers (`contextzip proxy grep -rn "tracking::track\b\|[^.]\btrack(" src/`), then delete it and its doc block.

- [ ] **Step 3: Build with warnings as errors to catch newly-exposed dead code**

Run: `cargo build --release 2>&1 | grep -iE "never used|dead_code|unused"`
Expected: empty (every removed suppression left either deleted code or wired code; no new warning).

- [ ] **Step 4: Full test + clippy**

Run: `cargo test && cargo clippy -- -D warnings`
Expected: green.

- [ ] **Step 5: Commit**

```bash
git add src/
git commit -m "chore: resolve dead_code suppressions (delete/wire/document)

Co-authored-by: Isaac"
```

---

## Task 8: Tooling hygiene — port or delete `rtk`-referencing scripts

**Files:**
- Modify or delete: `scripts/check-installation.sh` (21 rtk refs), `scripts/install-local.sh` (3), `scripts/test-all.sh` (138), `scripts/test-tracking.sh` (18), `scripts/validate-docs.sh` (1), `scripts/rtk-economics.sh` (16), `scripts/test-aristote.sh` (41)

**Interfaces:** none — tooling only.

- [ ] **Step 1: Triage each script**

Run: `for f in scripts/*.sh; do echo "== $f =="; head -5 "$f"; done`
Decide per-script: PORT (still useful: `check-installation.sh`, `install-local.sh`, `validate-docs.sh`, `test-tracking.sh`) or DELETE (superseded/fork-specific: `test-aristote.sh`, `rtk-economics.sh`; `test-all.sh` — port only if it tests current behavior, else delete). Do not blanket-delete or blanket-keep.

- [ ] **Step 2: Port the keepers**

In each kept script replace `rtk` -> `contextzip` (binary name and any `rtk` subcommands). Rename `rtk-economics.sh` -> `contextzip-economics.sh` if kept.

- [ ] **Step 3: Delete the dead ones**

```bash
git rm scripts/test-aristote.sh   # + any other DELETE-triaged scripts
```

- [ ] **Step 4: Verify no `rtk` binary refs remain in scripts**

Run: `contextzip proxy grep -rn "\brtk\b" scripts/ || echo "CLEAN"`
Expected: `CLEAN` (or only legitimate mentions in comments referencing the upstream project, which are fine).

- [ ] **Step 5: Commit**

```bash
git add -A scripts/
git commit -m "chore(scripts): port keepers to contextzip, delete fork-era dead scripts

Co-authored-by: Isaac"
```

---

## Self-Review

**Spec coverage:**
- Phase 1 (measure) -> Tasks 1, 2, 3. ✔
- Phase 2 (never-inflate guard) -> Tasks 4, 5. ✔
- Phase 3 (deepen high-frequency) -> Task 6. ✔
- Phase 4 (dead code + tooling) -> Tasks 7, 8. ✔
- Non-goal (no live Read/native-tool compression) -> honored; no task attempts it. ✔
- Non-goal (no new low-freq commands) -> honored; Task 2 only adds *measurement* for existing filters, no new command modules. ✔

**Placeholder scan:** Tasks 6 and 2 are intentionally data-bounded (their exact target set comes from Task 3 output / fixture availability), which is a real dependency, not a placeholder — each still specifies exact code, exact gate commands, and an explicit "do not assume / confirm from data" rule. No "TBD"/"add error handling"/"write tests for the above" left.

**Type consistency:** `choose_output(input, filtered) -> &str` and `emit(orig, cz_cmd, input, filtered, feature)` are defined in Task 4 and consumed with matching signatures in Task 5. `count_tokens` defined identically per-module in Task 6. `track_with_feature` signature matches `src/tracking.rs:1244`. ✔
