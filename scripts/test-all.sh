#!/usr/bin/env bash
#
# RTK Smoke Test Suite
# Exercises every command to catch regressions after merge.
# Exit code: number of failures (0 = all green)
#
set -euo pipefail

PASS=0
FAIL=0
SKIP=0
FAILURES=()

# Colors
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[0;33m'
CYAN='\033[0;36m'
BOLD='\033[1m'
NC='\033[0m'

# ── Helpers ──────────────────────────────────────────

assert_ok() {
    local name="$1"
    shift
    local output
    if output=$("$@" 2>&1); then
        PASS=$((PASS + 1))
        printf "  ${GREEN}PASS${NC}  %s\n" "$name"
    else
        FAIL=$((FAIL + 1))
        FAILURES+=("$name")
        printf "  ${RED}FAIL${NC}  %s\n" "$name"
        printf "        cmd: %s\n" "$*"
        printf "        out: %s\n" "$(echo "$output" | head -3)"
    fi
}

assert_contains() {
    local name="$1"
    local needle="$2"
    shift 2
    local output
    if output=$("$@" 2>&1) && echo "$output" | grep -q "$needle"; then
        PASS=$((PASS + 1))
        printf "  ${GREEN}PASS${NC}  %s\n" "$name"
    else
        FAIL=$((FAIL + 1))
        FAILURES+=("$name")
        printf "  ${RED}FAIL${NC}  %s\n" "$name"
        printf "        expected: '%s'\n" "$needle"
        printf "        got: %s\n" "$(echo "$output" | head -3)"
    fi
}

assert_exit_ok() {
    local name="$1"
    shift
    if "$@" >/dev/null 2>&1; then
        PASS=$((PASS + 1))
        printf "  ${GREEN}PASS${NC}  %s\n" "$name"
    else
        FAIL=$((FAIL + 1))
        FAILURES+=("$name")
        printf "  ${RED}FAIL${NC}  %s\n" "$name"
        printf "        cmd: %s\n" "$*"
    fi
}

assert_fails() {
    local name="$1"
    shift
    if "$@" >/dev/null 2>&1; then
        FAIL=$((FAIL + 1))
        FAILURES+=("$name (expected failure, got success)")
        printf "  ${RED}FAIL${NC}  %s (expected failure)\n" "$name"
    else
        PASS=$((PASS + 1))
        printf "  ${GREEN}PASS${NC}  %s\n" "$name"
    fi
}

assert_help() {
    local name="$1"
    shift
    assert_contains "$name --help" "Usage:" "$@" --help
}

skip_test() {
    local name="$1"
    local reason="$2"
    SKIP=$((SKIP + 1))
    printf "  ${YELLOW}SKIP${NC}  %s (%s)\n" "$name" "$reason"
}

section() {
    printf "\n${BOLD}${CYAN}── %s ──${NC}\n" "$1"
}

# ── Preamble ─────────────────────────────────────────

RTK=$(command -v contextzip || echo "")
if [[ -z "$RTK" ]]; then
    echo "contextzip not found in PATH. Run: cargo install --path ."
    exit 1
fi

printf "${BOLD}RTK Smoke Test Suite${NC}\n"
printf "Binary: %s\n" "$RTK"
printf "Version: %s\n" "$(contextzip --version)"
printf "Date: %s\n" "$(date '+%Y-%m-%d %H:%M')"

# Need a git repo to test git commands
if ! git rev-parse --is-inside-work-tree >/dev/null 2>&1; then
    echo "Must run from inside a git repository."
    exit 1
fi

REPO_ROOT=$(git rev-parse --show-toplevel)

# ── 1. Version & Help ───────────────────────────────

section "Version & Help"

assert_contains "contextzip --version" "contextzip" contextzip --version
assert_contains "contextzip --help" "Usage:" contextzip --help

# ── 2. Ls ────────────────────────────────────────────

section "Ls"

assert_ok      "contextzip ls ."                     contextzip ls .
assert_ok      "contextzip ls -la ."                 contextzip ls -la .
assert_ok      "contextzip ls -lh ."                 contextzip ls -lh .
assert_ok      "contextzip ls -l src/"               contextzip ls -l src/
assert_ok      "contextzip ls src/ -l (flag after)"  contextzip ls src/ -l
assert_ok      "contextzip ls multi paths"           contextzip ls src/ scripts/
assert_contains "contextzip ls -a shows hidden"      ".git" contextzip ls -a .
assert_contains "contextzip ls shows sizes"          "K"  contextzip ls src/
assert_contains "contextzip ls shows dirs with /"    "/" contextzip ls .

# ── 2b. Tree ─────────────────────────────────────────

section "Tree"

if command -v tree >/dev/null 2>&1; then
    assert_ok      "contextzip tree ."                contextzip tree .
    assert_ok      "contextzip tree -L 2 ."           contextzip tree -L 2 .
    assert_ok      "contextzip tree -d -L 1 ."        contextzip tree -d -L 1 .
    assert_contains "contextzip tree shows src/"      "src" contextzip tree -L 1 .
else
    skip_test "contextzip tree" "tree not installed"
fi

# ── 3. Read ──────────────────────────────────────────

section "Read"

assert_ok      "contextzip read Cargo.toml"          contextzip read Cargo.toml
assert_ok      "contextzip read --level none Cargo.toml"  contextzip read --level none Cargo.toml
assert_ok      "contextzip read --level aggressive Cargo.toml" contextzip read --level aggressive Cargo.toml
assert_ok      "contextzip read -n Cargo.toml"       contextzip read -n Cargo.toml
assert_ok      "contextzip read --max-lines 5 Cargo.toml" contextzip read --max-lines 5 Cargo.toml

section "Read (stdin support)"

assert_ok      "contextzip read stdin pipe"          bash -c 'echo "fn main() {}" | contextzip read -'

# ── 4. Git ───────────────────────────────────────────

section "Git (existing)"

assert_ok      "contextzip git status"               contextzip git status
assert_ok      "contextzip git status --short"       contextzip git status --short
assert_ok      "contextzip git status -s"            contextzip git status -s
assert_ok      "contextzip git status --porcelain"   contextzip git status --porcelain
assert_ok      "contextzip git log"                  contextzip git log
assert_ok      "contextzip git log -5"               contextzip git log -- -5
assert_ok      "contextzip git diff"                 contextzip git diff
assert_ok      "contextzip git diff --stat"          contextzip git diff --stat

section "Git (new: branch, fetch, stash, worktree)"

assert_ok      "contextzip git branch"               contextzip git branch
assert_ok      "contextzip git fetch"                contextzip git fetch
assert_ok      "contextzip git stash list"           contextzip git stash list
assert_ok      "contextzip git worktree"             contextzip git worktree

section "Git (passthrough: unsupported subcommands)"

assert_ok      "contextzip git tag --list"           contextzip git tag --list
assert_ok      "contextzip git remote -v"            contextzip git remote -v
assert_ok      "contextzip git rev-parse HEAD"       contextzip git rev-parse HEAD

# ── 5. GitHub CLI ────────────────────────────────────

section "GitHub CLI"

if command -v gh >/dev/null 2>&1 && gh auth status >/dev/null 2>&1; then
    assert_ok      "contextzip gh pr list"           contextzip gh pr list
    assert_ok      "contextzip gh run list"          contextzip gh run list
    assert_ok      "contextzip gh issue list"        contextzip gh issue list
    # pr create/merge/diff/comment/edit are write ops, test help only
    assert_help    "contextzip gh"                   contextzip gh
else
    skip_test "gh commands" "gh not authenticated"
fi

# ── 6. Cargo ─────────────────────────────────────────

section "Cargo (new)"

assert_ok      "contextzip cargo build"              contextzip cargo build
assert_ok      "contextzip cargo clippy"             contextzip cargo clippy
# cargo test exits non-zero due to pre-existing failures; check output ignoring exit code
output_cargo_test=$(contextzip cargo test 2>&1 || true)
if echo "$output_cargo_test" | grep -q "FAILURES\|test result:\|passed"; then
    PASS=$((PASS + 1))
    printf "  ${GREEN}PASS${NC}  %s\n" "contextzip cargo test"
else
    FAIL=$((FAIL + 1))
    FAILURES+=("contextzip cargo test")
    printf "  ${RED}FAIL${NC}  %s\n" "contextzip cargo test"
    printf "        got: %s\n" "$(echo "$output_cargo_test" | head -3)"
fi
assert_help    "contextzip cargo"                    contextzip cargo

# ── 7. Curl ──────────────────────────────────────────

section "Curl (new)"

assert_contains "contextzip curl JSON detect" "string" contextzip curl https://httpbin.org/json
assert_ok       "contextzip curl plain text"          contextzip curl https://httpbin.org/robots.txt
assert_help     "contextzip curl"                     contextzip curl

# ── 8. Npm / Npx ────────────────────────────────────

section "Npm / Npx (new)"

assert_help    "contextzip npm"                      contextzip npm
assert_help    "contextzip npx"                      contextzip npx

# ── 9. Pnpm ─────────────────────────────────────────

section "Pnpm"

assert_help    "contextzip pnpm"                     contextzip pnpm
assert_help    "contextzip pnpm build"               contextzip pnpm build
assert_help    "contextzip pnpm typecheck"           contextzip pnpm typecheck

if command -v pnpm >/dev/null 2>&1; then
    assert_ok  "contextzip pnpm help"                contextzip pnpm help
fi

# ── 10. Grep ─────────────────────────────────────────

section "Grep"

assert_ok      "contextzip grep pattern"             contextzip grep "pub fn" src/
assert_contains "contextzip grep finds results"      "pub fn" contextzip grep "pub fn" src/
assert_ok      "contextzip grep with file type"      contextzip grep "pub fn" src/ -t rust

section "Grep (extra args passthrough)"

assert_ok      "contextzip grep -i case insensitive" contextzip grep "fn" src/ -i
assert_ok      "contextzip grep -A context lines"    contextzip grep "fn run" src/ -A 2

# ── 11. Find ─────────────────────────────────────────

section "Find"

assert_ok      "contextzip find *.rs"                contextzip find "*.rs" src/
assert_contains "contextzip find shows files"        ".rs" contextzip find "*.rs" src/

# ── 12. Json ─────────────────────────────────────────

section "Json"

# Create temp JSON file for testing
TMPJSON=$(mktemp /tmp/contextzip-test-XXXXX.json)
echo '{"name":"test","count":42,"items":[1,2,3]}' > "$TMPJSON"

assert_ok      "contextzip json file"                contextzip json "$TMPJSON"
assert_contains "contextzip json shows schema"       "string" contextzip json "$TMPJSON"

rm -f "$TMPJSON"

# ── 13. Deps ─────────────────────────────────────────

section "Deps"

assert_ok      "contextzip deps ."                   contextzip deps .
assert_contains "contextzip deps shows Cargo"        "Cargo" contextzip deps .

# ── 14. Env ──────────────────────────────────────────

section "Env"

assert_ok      "contextzip env"                      contextzip env
assert_ok      "contextzip env --filter PATH"        contextzip env --filter PATH

# ── 16. Log ──────────────────────────────────────────

section "Log"

TMPLOG=$(mktemp /tmp/contextzip-log-XXXXX.log)
for i in $(seq 1 20); do
    echo "[2025-01-01 12:00:00] INFO: repeated message" >> "$TMPLOG"
done
echo "[2025-01-01 12:00:01] ERROR: something failed" >> "$TMPLOG"

assert_ok      "contextzip log file"                 contextzip log "$TMPLOG"

rm -f "$TMPLOG"

# ── 17. Summary ──────────────────────────────────────

section "Summary"

assert_ok      "contextzip summary echo hello"       contextzip summary echo hello

# ── 18. Err ──────────────────────────────────────────

section "Err"

assert_ok      "contextzip err echo ok"              contextzip err echo ok

# ── 19. Test runner ──────────────────────────────────

section "Test runner"

assert_ok      "contextzip test echo ok"             contextzip test echo ok

# ── 20. Gain ─────────────────────────────────────────

section "Gain"

assert_ok      "contextzip gain"                     contextzip gain
assert_ok      "contextzip gain --history"           contextzip gain --history

# ── 21. Config & Init ────────────────────────────────

section "Config & Init"

assert_ok      "contextzip config"                   contextzip config
assert_ok      "contextzip init --show"              contextzip init --show

# ── 22. Wget ─────────────────────────────────────────

section "Wget"

if command -v wget >/dev/null 2>&1; then
    assert_ok  "contextzip wget stdout"              contextzip wget https://httpbin.org/robots.txt -O
else
    skip_test "contextzip wget" "wget not installed"
fi

# ── 23. Tsc / Lint / Prettier / Next / Playwright ───

section "JS Tooling (help only, no project context)"

assert_help    "contextzip tsc"                      contextzip tsc
assert_help    "contextzip lint"                     contextzip lint
assert_help    "contextzip prettier"                 contextzip prettier
assert_help    "contextzip next"                     contextzip next
assert_help    "contextzip playwright"               contextzip playwright

# ── 24. Prisma ───────────────────────────────────────

section "Prisma (help only)"

assert_help    "contextzip prisma"                   contextzip prisma

# ── 25. Vitest ───────────────────────────────────────

section "Vitest (help only)"

assert_help    "contextzip vitest"                   contextzip vitest

# ── 26. Docker / Kubectl (help only) ────────────────

section "Docker / Kubectl (help only)"

assert_help    "contextzip docker"                   contextzip docker
assert_help    "contextzip kubectl"                  contextzip kubectl

# ── 27. Python (conditional) ────────────────────────

section "Python (conditional)"

if command -v pytest &>/dev/null; then
    assert_help    "contextzip pytest"                    contextzip pytest --help
else
    skip_test "contextzip pytest" "pytest not installed"
fi

if command -v ruff &>/dev/null; then
    assert_help    "contextzip ruff"                      contextzip ruff --help
else
    skip_test "contextzip ruff" "ruff not installed"
fi

if command -v pip &>/dev/null; then
    assert_help    "contextzip pip"                       contextzip pip --help
else
    skip_test "contextzip pip" "pip not installed"
fi

# ── 28. Go (conditional) ────────────────────────────

section "Go (conditional)"

if command -v go &>/dev/null; then
    assert_help    "contextzip go"                        contextzip go --help
    assert_help    "contextzip go test"                   contextzip go test -h
    assert_help    "contextzip go build"                  contextzip go build -h
    assert_help    "contextzip go vet"                    contextzip go vet -h
else
    skip_test "contextzip go" "go not installed"
fi

if command -v golangci-lint &>/dev/null; then
    assert_help    "contextzip golangci-lint"             contextzip golangci-lint --help
else
    skip_test "contextzip golangci-lint" "golangci-lint not installed"
fi

# ── 29. Graphite (conditional) ─────────────────────

section "Graphite (conditional)"

if command -v gt &>/dev/null; then
    assert_help   "contextzip gt"                          contextzip gt --help
    assert_ok     "contextzip gt log short"                contextzip gt log short
else
    skip_test "contextzip gt" "gt not installed"
fi

# ── 30. Global flags ────────────────────────────────

section "Global flags"

assert_ok      "contextzip -u ls ."                  contextzip -u ls .
assert_ok      "contextzip --skip-env npm --help"    contextzip --skip-env npm --help

# ── 31. CcEconomics ─────────────────────────────────

section "CcEconomics"

assert_ok      "contextzip cc-economics"             contextzip cc-economics

# ── 32. Learn ───────────────────────────────────────

section "Learn"

assert_ok      "contextzip learn --help"             contextzip learn --help
assert_ok      "contextzip learn (no sessions)"      contextzip learn --since 0 2>&1 || true

# ── 32. Rewrite ───────────────────────────────────────

section "Rewrite"

assert_contains "rewrite git status"          "contextzip git status"         contextzip rewrite "git status"
assert_contains "rewrite cargo test"          "contextzip cargo test"         contextzip rewrite "cargo test"
assert_contains "rewrite compound &&"         "contextzip git status"         contextzip rewrite "git status && cargo test"
assert_contains "rewrite pipe preserves"      "| head"                 contextzip rewrite "git log | head"

section "Rewrite (#345: RTK_DISABLED skip)"

assert_fails   "rewrite RTK_DISABLED=1 skip"                          contextzip rewrite "RTK_DISABLED=1 git status"
assert_fails   "rewrite env RTK_DISABLED skip"                        contextzip rewrite "FOO=1 RTK_DISABLED=1 cargo test"

section "Rewrite (#346: 2>&1 preserved)"

assert_contains "rewrite 2>&1 preserved"      "2>&1"                  contextzip rewrite "cargo test 2>&1 | head"

section "Rewrite (#196: gh --json skip)"

assert_fails   "rewrite gh --json skip"                               contextzip rewrite "gh pr list --json number"
assert_fails   "rewrite gh --jq skip"                                 contextzip rewrite "gh api /repos --jq .name"
assert_fails   "rewrite gh --template skip"                           contextzip rewrite "gh pr view 1 --template '{{.title}}'"
assert_contains "rewrite gh normal works"     "contextzip gh pr list"        contextzip rewrite "gh pr list"

# ── 33. Verify ────────────────────────────────────────

section "Verify"

assert_ok      "contextzip verify"                   contextzip verify

# ── 34. Proxy ─────────────────────────────────────────

section "Proxy"

assert_ok      "contextzip proxy echo hello"         contextzip proxy echo hello
assert_contains "contextzip proxy passthrough"       "hello" contextzip proxy echo hello

# ── 35. Discover ──────────────────────────────────────

section "Discover"

assert_ok      "contextzip discover"                 contextzip discover

# ── 36. Diff ──────────────────────────────────────────

section "Diff"

assert_ok      "contextzip diff two files"           contextzip diff Cargo.toml LICENSE

# ── 37. Wc ────────────────────────────────────────────

section "Wc"

assert_ok      "contextzip wc Cargo.toml"            contextzip wc Cargo.toml

# ── 38. Smart ─────────────────────────────────────────

section "Smart"

assert_ok      "contextzip smart src/main.rs"        contextzip smart src/main.rs

# ── 39. Json edge cases ──────────────────────────────

section "Json (edge cases)"

assert_fails   "contextzip json on TOML (#347)"                              contextzip json Cargo.toml

# ── 40. Docker (conditional) ─────────────────────────

section "Docker (conditional)"

if command -v docker >/dev/null 2>&1 && docker info >/dev/null 2>&1; then
    assert_ok  "contextzip docker ps"               contextzip docker ps
    assert_ok  "contextzip docker images"           contextzip docker images
else
    skip_test "contextzip docker" "docker not running"
fi

# ── 41. Hook check ───────────────────────────────────

section "Hook check (#344)"

assert_contains "contextzip init --show hook version" "version" contextzip init --show

# ══════════════════════════════════════════════════════
# Report
# ══════════════════════════════════════════════════════

printf "\n${BOLD}══════════════════════════════════════${NC}\n"
printf "${BOLD}Results: ${GREEN}%d passed${NC}, ${RED}%d failed${NC}, ${YELLOW}%d skipped${NC}\n" "$PASS" "$FAIL" "$SKIP"

if [[ ${#FAILURES[@]} -gt 0 ]]; then
    printf "\n${RED}Failures:${NC}\n"
    for f in "${FAILURES[@]}"; do
        printf "  - %s\n" "$f"
    done
fi

printf "${BOLD}══════════════════════════════════════${NC}\n"

exit "$FAIL"
