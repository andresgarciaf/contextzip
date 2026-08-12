# Deferred minors - Effectiveness + Hygiene Pass (2026-08-11)

Non-blocking follow-ups from the branch's final review.

**All three resolved 2026-08-12** on branch `fix/deferred-minors`:

- **[RESOLVED] benchmark.sh cosmetic RTK strings**: stale `RTK` stdout column header,
  debug diff table headers, `/tmp/rtk_*` temp file names, and `tracking.rs`
  `/tmp/rtk_test_custom.db` all renamed to ContextZip/`/tmp/cz_*`. Developer-only, no
  runtime impact.
- **[RESOLVED] main.rs TOML-path tee hint** (src/main.rs): the tee hint was printed via a
  naked `println!` AFTER `emit`, bypassing the never-inflate guard. Now folded into
  `filtered_out` before `emit`, so it routes through the guard and is tracked - consistent
  with every other tee-hint site.
- **[RESOLVED] go_cmd duplicated output composition** (src/go_cmd.rs run_test/run_build/run_vet):
  the three sites duplicated the "filtered + optional tee hint + newline" logic (run_test even
  lacked the empty-filtered guard). Extracted a single `compose_output(filtered, hint)` helper
  with a unit test; all three now route through it.
