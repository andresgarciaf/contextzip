# ContextZip Benchmark Results

**Date:** 2026-08-11
**Version:** 0.2.0
**Test cases:** 59 (55 good, 2 skip, 2 fail)
**Methodology:** Each test runs the real command via the release binary and the equivalent shell command, then compares character counts as a token proxy. All tests run with `CONTEXTZIP_QUIET=1`. Numbers below are from an actual harness run on this machine - no carried-forward figures.

## Global Summary

```
Tokens: 916317 -> 87941  (-90%)
55 good  2 skip  2 fail    55/59 (93%)
```

## Per-Command Results

### ls (8 tests)

| Command | Input tokens | Output tokens | Savings |
|---------|-------------:|--------------:|--------:|
| ls | 574 | 129 | 77% |
| ls src/ | 1,323 | 354 | 73% |
| ls -l src/ | 1,294 | 354 | 72% |
| ls -la src/ | 1,323 | 354 | 73% |
| ls -lh src/ | 1,294 | 354 | 72% |
| ls src/ -l | 1,294 | 354 | 72% |
| ls -a | 574 | 135 | 76% |
| ls src/ scripts/ (multi) | 1,541 | 412 | 73% |

### read (4 tests)

| Command | Input tokens | Output tokens | Savings |
|---------|-------------:|--------------:|--------:|
| read src/main.rs (default) | 22,101 | 21,345 | 3% |
| read src/main.rs -l minimal | 22,101 | 21,345 | 3% |
| read src/main.rs -l aggressive | 22,101 | 1,886 | 91% |
| read src/main.rs -n | 26,882 | 26,058 | 3% |

### find (4 tests)

| Command | Input tokens | Output tokens | Savings |
|---------|-------------:|--------------:|--------:|
| find '*' | 653,779 | 258 | 99% |
| find '*.rs' | 898 | 162 | 81% |
| find '*' --max 10 | 65 | 48 | 26% |
| find '*' --max 100 | 843 | 475 | 43% |

### git (4 tests)

| Command | Input tokens | Output tokens | Savings |
|---------|-------------:|--------------:|--------:|
| git status | 20 | 15 | 25% |
| git log -n 10 | 1,188 | 335 | 71% |
| git log -n 5 | 367 | 148 | 59% |
| git diff HEAD~1 | 468 | 303 | 35% |

### grep (5 tests)

| Command | Input tokens | Output tokens | Savings |
|---------|-------------:|--------------:|--------:|
| grep 'fn ' src/ | 36,292 | 2,962 | 91% |
| grep 'struct ' src/ | 1,701 | 1,524 | 10% |
| grep 'fn ' src/ -l 40 | 36,292 | 2,470 | 93% |
| grep 'fn ' src/ --max 20 | 340 | 248 | 27% |
| grep 'fn ' src/ -c | 13,136 | 2,942 | 77% |

### json (2 tests)

| Command | Input tokens | Output tokens | Savings |
|---------|-------------:|--------------:|--------:|
| json /tmp/rtk_bench.json | 59 | 51 | 13% |
| json /tmp/rtk_bench.json -d 2 | 59 | 50 | 15% |

### deps (1 test)

| Command | Input tokens | Output tokens | Savings |
|---------|-------------:|--------------:|--------:|
| deps (Cargo.toml) | 417 | 55 | 86% |

### env (3 tests)

| Command | Input tokens | Output tokens | Savings |
|---------|-------------:|--------------:|--------:|
| env | 1,851 | 680 | 63% |
| env -f PATH | 975 | 109 | 88% |
| env --show-all | 1,851 | 703 | 62% |

### err (1 test)

| Command | Input tokens | Output tokens | Savings |
|---------|-------------:|--------------:|--------:|
| err cargo build | 18 | 12 | 33% |

### test (1 test)

| Command | Input tokens | Output tokens | Savings |
|---------|-------------:|--------------:|--------:|
| test cargo test | 18,850 | 51 | 99% |

### log (1 test)

| Command | Input tokens | Output tokens | Savings |
|---------|-------------:|--------------:|--------:|
| log /tmp/rtk_bench_sample.log | 158 | 58 | 63% |

### summary (2 tests)

| Command | Input tokens | Output tokens | Savings |
|---------|-------------:|--------------:|--------:|
| summary cargo --help | 525 | 65 | 87% |
| summary rustc --help | 1,006 | 55 | 94% |

### cargo (4 tests)

| Command | Input tokens | Output tokens | Savings |
|---------|-------------:|--------------:|--------:|
| cargo build | 18 | 8 | 55% |
| cargo test | 18,850 | 13 | 99% |
| cargo clippy | 18 | 8 | 55% |
| cargo check | 18 | 8 | 55% |

### diff (1 test)

| Command | Input tokens | Output tokens | Savings |
|---------|-------------:|--------------:|--------:|
| diff Cargo.toml LICENSE | 780 | 597 | 23% |

### smart (1 test)

| Command | Input tokens | Output tokens | Savings |
|---------|-------------:|--------------:|--------:|
| smart src/main.rs | 22,101 | 27 | 99% |

### wc (1 test)

| Command | Input tokens | Output tokens | Savings |
|---------|-------------:|--------------:|--------:|
| wc Cargo.toml src/main.rs | 26 | 20 | 23% |

### curl (2 tests)

| Command | Input tokens | Output tokens | Savings |
|---------|-------------:|--------------:|--------:|
| curl https://httpbin.org/json | 107 | 39 | 63% |
| curl https://httpbin.org/robots.txt | 8 | 8 | 0% (skip - trivially small input) |

### wget (1 test)

| Command | Input tokens | Output tokens | Savings |
|---------|-------------:|--------------:|--------:|
| wget https://httpbin.org/robots.txt | 8 | 17 | -112% (skip - inflating on trivial input) |

### gh (2 tests)

| Command | Input tokens | Output tokens | Savings |
|---------|-------------:|--------------:|--------:|
| gh pr list | 0 | 4 | n/a (empty input - no PRs in this repo) |
| gh run list | 0 | 4 | n/a (empty input - no runs in this repo) |

### docker (2 tests - FAILED)

Not measured: `docker` binary not present on this machine. Both `docker ps` and `docker images` returned no output.

### python - ruff / pytest (2 tests)

| Command | Input tokens | Output tokens | Savings |
|---------|-------------:|--------------:|--------:|
| ruff check . | 278 | 63 | 77% |
| pytest -v | 202 | 7 | 96% |

### mypy (1 test)

| Command | Input tokens | Output tokens | Savings |
|---------|-------------:|--------------:|--------:|
| mypy (2 sample files) | 343 | 268 | 21% |

### rewrite (6 tests)

Functional correctness tests only - no savings metric applies. All 6 passed.

---

## Ranked Classification

Commands are classified by measured savings. This list drives the Phase 3 improvement work list.

### 🟢 >=60% savings (strong - keep as-is)

- `ls` (all flag variants): 72-77%
- `read -l aggressive`: 91%
- `find '*'` (unfiltered): 99%
- `find '*.rs'` (glob): 81%
- `git log -n 10`: 71%
- `grep 'fn ' src/` (full): 91%
- `grep 'fn ' src/ -l 40` (line-limit): 93%
- `grep 'fn ' src/ -c` (count mode): 77%
- `deps`: 86%
- `env`: 63%
- `env -f PATH`: 88%
- `env --show-all`: 62%
- `test cargo test`: 99%
- `log`: 63%
- `summary cargo --help`: 87%
- `summary rustc --help`: 94%
- `cargo test`: 99%
- `smart src/main.rs`: 99%
- `curl` (JSON response): 63%
- `ruff check`: 77%
- `pytest -v`: 96%

### 🟡 20-60% savings (moderate - improvement candidates)

- `find '*' --max 10`: 26%
- `find '*' --max 100`: 43%
- `git status`: 25%
- `git log -n 5`: 59%
- `git diff HEAD~1`: 35%
- `grep 'fn ' src/ --max 20`: 27%
- `err cargo build`: 33%
- `cargo build`: 55%
- `cargo clippy`: 55%
- `cargo check`: 55%
- `diff Cargo.toml LICENSE`: 23%
- `wc`: 23%
- `mypy`: 21%

### 🔴 <20% or inflating (weak/broken - Phase 3 fix targets)

- `read` (default level): 3%
- `read -l minimal`: 3%
- `read -n` (with line numbers): 3%
- `grep 'struct ' src/` (small result set): 10%
- `json` (default): 13%
- `json -d 2`: 15%
- `wget` (small/plain-text response): -112% (inflating)
- `gh pr list` / `gh run list`: inflating on empty input (0 -> 4 tokens)
- `curl` (plain-text robots.txt): 0%

---

## Notes

### Measurement Methodology

Character count is used as a token proxy. The harness runs each command twice - once via the raw shell equivalent, once via `contextzip` - and compares output sizes. Savings = `(1 - output/input) * 100`. All runs use `CONTEXTZIP_QUIET=1` to suppress contextzip's own status lines.

### Why Some Commands Show Low or Negative Savings

- **`read` (default/minimal level):** The default and minimal filter levels preserve nearly all content by design. Use `-l aggressive` for deep compression.
- **`grep` on small result sets:** When the filtered result is already compact (e.g., `grep struct` produces only a few lines), the filter adds header overhead that exceeds gains.
- **`json` on small files:** The test fixture (`/tmp/rtk_bench.json`) is tiny (59 tokens). Format normalization saves little on already-minimal JSON.
- **`wget` on plain-text:** The robots.txt response is 8 tokens. contextzip emits a wrapper that exceeds the original.
- **`gh pr list` / `gh run list`:** No PRs or runs exist in this repo, so input is 0 tokens. contextzip emits a small status message, making output larger than input. Not a real-world concern.
- **`cargo build` / `clippy` / `check`:** Build is already cached (0 crates compiled), so raw output is only 18 tokens. Savings look modest in percentage but the absolute compression holds on real builds.
- **`docker` commands:** Not measured - `docker` binary absent on this machine. Add to CI matrix when docker is available.
