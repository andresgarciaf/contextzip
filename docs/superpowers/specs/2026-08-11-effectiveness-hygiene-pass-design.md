# Effectiveness + Hygiene Pass — Design

**Date:** 2026-08-11
**Status:** Approved (design)
**Branch:** `feat/effectiveness-hygiene-pass`

## Goal

Maximize the token reduction ContextZip actually delivers on the commands it runs
every day, and remove dead weight. The tool already has broad coverage (63
auto-rewrite patterns, 35 filter modules); the problem is not breadth, it is that
we have almost no data on which filters actually save tokens, several are known to
*inflate*, and the fork left behind dead code and broken tooling.

This is a data-first effectiveness pass, not a feature-expansion pass.

## Non-goals (explicitly out of scope)

- **Live compression of native tool results (Read, Grep, WebFetch, MCP).** Verified
  against the current Claude Code hook contract: `PostToolUse` cannot rewrite a tool
  result before it enters context. Only `PreToolUse` `updatedInput` exists, which only
  helps when changing the *input* changes the *output* (i.e. Bash). Native-tool
  payloads are only reachable by the resume-time `compact`/`apply` engine, which
  already exists. Not buildable here; do not attempt deny-and-redirect hacks.
- **New low-frequency commands** (gradle, podman, sbt, raw jq). Breadth is already
  solved; the marginal token win does not justify the surface.

## Background (measured, 2026-08-11)

- `scripts/benchmark.sh` still shells out to the `rtk` binary (pre-fork), so it does
  not run against `contextzip` at all.
- `scripts/benchmark/` (its input corpus) is empty; the numbers in
  `docs/benchmark-results.md` (dated 2026-03-19, v0.1.0) are **not reproducible** and
  predate all v2/v3 work.
- The harness only exercises ~20 of the 63 auto-rewrite patterns.
- Only 8 of 35 filter modules carry a savings assertion; 2 fixtures exist total.
- `benchmark-results.md` itself names the losers: `npm` 2-8%, eslint-small 2-10%,
  rust-panic 2-7%, java IOException **-12%**, `ls src/` **-56%**.
- 47 `#[allow(dead_code)]` markers suppress unused-code warnings across src.
- 8 scripts still call the `rtk` binary and are broken since the fork
  (`test-all.sh` alone has 138 refs).

## Phases

Sequenced deliberately: measure -> guard -> deepen -> clean. Phase 1 data drives
Phase 3. Phase 4 is independent and can land at any point.

### Phase 1 — Measurement (port + expand the harness)

Make current, reproducible savings data exist.

- Port `scripts/benchmark.sh`: `rtk` -> `contextzip`, `RTK` var -> `CONTEXTZIP`,
  prefer `./target/release/contextzip`.
- Expand coverage from ~20 to the full auto-rewrite set. Every pattern that has a
  Rust filter module gets at least one bench case driven from a **checked-in
  fixture** (real captured output, not synthetic), so results reproduce in CI.
  Commands needing an external binary (docker, kubectl, terraform, helm) run from a
  captured fixture file piped to `contextzip <cmd>`, not a live invocation.
- Fixtures live in `tests/fixtures/<cmd>/` following the existing `dotnet/` pattern.
- Output: regenerate `docs/benchmark-results.md` from current code, with a ranked
  table classifying each pattern 🟢 >=60% / 🟡 20-60% / 🔴 <20% or inflating.

Acceptance: `./scripts/benchmark.sh` runs green against `contextzip`, every
Rust-backed pattern has >=1 fixture-driven case, the doc is regenerated and dated
today with the current version.

### Phase 2 — Never-inflate guard (biggest safe win)

Kill the entire negative-savings class in one shared helper instead of tuning 63
filters. This mirrors the invariant the session compactor already enforces.

- Add one shared guard on the command output path: after a filter produces output,
  if `output.len() >= input.len()`, emit the raw input instead. Measured on the
  post-ANSI-strip input so ANSI removal still counts as a win.
- Applies uniformly to all `*_cmd` filters via the common output/tracking path (the
  same place `tracking::record` is called), not per-module.
- Tracking records the passthrough so `gain` reflects reality (0% saved, not a
  fabricated number).
- A `CONTEXTZIP_NO_INFLATE_GUARD=1` escape hatch for debugging is optional, not
  required.

Acceptance: re-run Phase 1 harness — no pattern shows negative savings; the
previously-negative cases (`ls src/`, java IOException, npm small) now show ~0% via
passthrough rather than inflation. A unit test asserts a filter that would inflate a
tiny input returns the input unchanged.

### Phase 3 — Deepen high-frequency under-performers

Only the daily-driver commands that Phase 1 proves weak. No speculative tuning.

- Candidate set (confirm against Phase 1 data, do not assume): git, grep, read,
  docker, gh, pytest — whichever land 🟡.
- For each confirmed weak high-frequency filter, deepen the filter logic to reach
  🟢 where the input genuinely contains removable noise; where it does not (filter is
  already near-optimal), leave it and note why in the results doc.
- Each deepened filter gets a savings assertion (>=60% on its fixture) per the repo's
  existing cli-testing rule, closing the "8 of 35 have assertions" gap for the
  commands that matter most.

Acceptance: every high-frequency command in the candidate set is 🟢 or documented as
already-optimal; each has a fixture + savings assertion.

### Phase 4 — Dead-code deprecation + tooling hygiene (independent)

- Audit all 47 `#[allow(dead_code)]` markers. For each: delete if genuinely unused,
  wire up if it should be called, or keep with a one-line comment naming why it is
  pending. Remove the blanket suppression where code is deleted.
- The 8 `rtk`-referencing scripts: port the ones still useful
  (`check-installation.sh`, `install-local.sh`), delete the dead ones
  (`test-aristote.sh`, `rtk-economics.sh`, and any superseded test scripts) — decide
  per-script during implementation, do not blanket-delete.
- Stale `rtk` refs in `src/`: fix only genuine bugs (e.g. user-facing strings/help);
  leave refs that are legitimate test fixtures asserting historical output.
- `cargo build --release` and `cargo clippy -- -D warnings` stay clean throughout;
  after removing suppressions, no new warnings may appear.

Acceptance: no `#[allow(dead_code)]` remains without a justifying comment; no script
references a non-existent `rtk` binary; clippy clean.

## Testing

- Per repo `cli-testing.md`: fixture-driven snapshot + savings assertions.
- `cargo test` and `cargo clippy -- -D warnings` green after every phase.
- The benchmark harness is the phase-1/2 acceptance gate and becomes CI-runnable.

## Risks

- **Fixture drift** — checked-in fixtures can go stale vs. real tool output. Mitigated
  by capturing from real commands once and treating them as regression anchors, not
  live truth.
- **Over-deletion in Phase 4** — some `dead_code` is genuinely pending wiring. Mitigated
  by per-item decision, not blanket removal.
- **Guard masking a real regression** — the never-inflate guard could hide a filter
  that silently broke into passthrough. Mitigated by tracking recording passthroughs
  so `gain`/benchmark surfaces a filter that suddenly stops saving.
