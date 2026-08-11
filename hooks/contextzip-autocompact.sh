#!/usr/bin/env bash
# contextzip-hook-version: 1
# ContextZip SessionEnd hook - automatically compacts a finished session so the
# next resume rebuilds a smaller context (fewer input tokens).
#
# Safety model (SessionEnd is officially untested for file rewrites, so this is
# built defensively):
#   - Fires only on SessionEnd (session already terminated: the safe window).
#   - Does the real work in a DETACHED background process and exits immediately,
#     so it never blocks session exit or hits the ~1.5s SessionEnd budget.
#   - Sleeps briefly first: the transcript is written asynchronously and may lag
#     the in-memory conversation at SessionEnd.
#   - compact is fallback-safe on partial JSON; apply is an atomic swap that
#     backs the original up to .bak. An interrupted run leaves either the intact
#     original or a valid compacted file - never a corrupt one. `expand` restores.
#
# Requires: jq, contextzip on PATH.

command -v jq >/dev/null 2>&1 || exit 0
command -v contextzip >/dev/null 2>&1 || exit 0

INPUT=$(cat)
TP=$(printf '%s' "$INPUT" | jq -r '.transcript_path // empty' 2>/dev/null)

# Only proceed with a real transcript file.
[ -n "$TP" ] && [ -f "$TP" ] || exit 0

# Detach the work so it outlives this hook and never blocks session exit.
# nohup + & is portable across macOS and Linux (setsid is Linux-only).
# `apply` refuses when a .bak already exists (session was applied before), so a
# resumed-then-re-ended session is a safe no-op rather than a double-apply.
#
# Compaction defaults to the SAFE axes only. To enable the aggressive metadata
# axes here, set `aggressive = true` under [compact] in
# ~/.config/contextzip/config.toml - `compact` reads it, no flag needed.
# $TP is passed as a positional arg ($1), never interpolated into the script
# string, so a path containing shell metacharacters cannot be parsed as code.
nohup bash -c '
  sleep 3
  contextzip compact "$1" >/dev/null 2>&1 && \
  contextzip apply "$1" >/dev/null 2>&1
' _ "$TP" >/dev/null 2>&1 < /dev/null &

exit 0
