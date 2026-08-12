# Deferred minors - Effectiveness + Hygiene Pass (2026-08-11)

Non-blocking follow-ups from the branch's final review. None gate merge.

- **benchmark.sh cosmetic RTK strings**: stale `RTK` stdout column header (scripts/benchmark.sh:174),
  debug diff table header (~144-145), `/tmp/rtk_*` temp file names, and `tracking.rs:1488`
  `/tmp/rtk_test_custom.db`. Developer-only, no runtime impact.
- **main.rs TOML-path tee hint** (~src/main.rs:1173): the tee hint is printed via a naked
  `println!` AFTER `emit`, so it bypasses the never-inflate guard. Blast radius is a one-line
  file-path hint on command failure. Fold the hint into `filtered_out` before `emit` if touched.
- **go_cmd empty filtered_out** (src/go_cmd.rs run_build/run_vet): pass empty `filtered_out` to
  `emit`; behaviorally correct (choose_output returns the empty output) but an early return would
  read cleaner.
