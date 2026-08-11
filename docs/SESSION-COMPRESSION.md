# Session Compression — How It Works

A practical guide to ContextZip's session-history compressor: what it does, whether
you need to run anything, and the exact commands.

## The one thing to know first

**Session compression is manual. It does NOT run automatically.**

Installing ContextZip (`cargo install --path .`) puts the `contextzip` binary on your
system. The optional `contextzip init` hook only rewrites shell commands (`git`, `ls`,
`cargo`, ...) to compact their output — it does **not** touch your session files.

Your Claude Code session archive under `~/.claude/projects/*/*.jsonl` is only ever
compressed when **you run a command**. Nothing happens in the background, nothing runs
on a timer, and Claude Code itself is never modified. You stay in control.

So: install once, then run `contextzip compact ...` yourself whenever you want to shrink
a session. There is nothing to "just worry about while using Claude Code."

## What it compresses

Claude Code session `.jsonl` files grow large because they store the full history:
every file read, every command output, screenshots as inlined base64, and duplicated
bookkeeping. ContextZip rewrites the bulky parts into compact, reversible references.

Two tiers:

**Default (safe) axes** — always on with `compact`:
- `ReadDedup` — a file read more than once collapses to a reference to the first read.
- `BashHistoryCompact` — past command output is re-filtered (ANSI stripped, repeated
  lines tallied, capped).
- `GrepGlobDedup`, `BashCmdDedup`, `GenericResultCap` — repeated searches, repeated
  commands, and oversized tool results.

**Aggressive axes** — only with the `--aggressive` flag (off by default):
- `SidecarDedup` — collapses Claude Code's duplicate copy of each tool result.
- `MediaReference` — replaces inlined base64 screenshots with a short `sha256` marker.
- `SignatureDrop` — drops replay-only cryptographic signatures on thinking blocks.
- `McpJsonCompact` — minifies and unwraps double-encoded MCP tool JSON.

Aggressive axes touch Claude-Code-internal fields, so they are opt-in. On a
media-heavy session they take it from ~9 MB to ~1.3 MB (86% smaller); on text-heavy
sessions expect 6-16%.

## The commands

```bash
# See what WOULD be saved, across every session. Writes nothing.
contextzip compact --all-sessions --dry-run --aggressive

# Dry-run a single session (write-free). Accepts a session-id or a full .jsonl path.
contextzip compact <session-id> --dry-run --aggressive

# 1. Compact: writes a reversible <session>.jsonl.compressed sidecar. Original untouched.
contextzip compact <session-id> --aggressive

# 2. Apply: atomic swap — the compacted file becomes live; original saved as .jsonl.bak.
contextzip apply <session-id>

# 3. Expand: roll back — restores the original from .bak.
contextzip expand <session-id>
```

Drop `--aggressive` from any command to use only the safe default axes.

## The safe way to try it

1. **Dry-run first** — `compact --all-sessions --dry-run --aggressive` shows savings with
   zero writes. Pick a large, *closed* session to test on (not one you have open in
   Claude Code right now).
2. **Compact** — writes only the `.compressed` sidecar; your live session is untouched, so
   this step is always safe. Rollback at this stage is just `rm <session>.jsonl.compressed`.
3. **Apply** — this is the only step that rewrites the live `.jsonl` (after backing it up
   to `.bak`). If Claude Code has that session open, reload it afterward.
4. **Expand** — restores the original from `.bak` any time you want it back.

Find your largest sessions:
```bash
ls -1S ~/.claude/projects/*/*.jsonl | head -5
```

## Safety guarantees

- **Reversible** — the original is preserved as `.bak`; `expand` restores it.
- **Never inflates** — if a rewrite would not shrink a record, it is left untouched.
- **Idempotent** — re-running `compact` on an already-compacted session is a no-op.
- **Secret redaction** — Databricks PATs, AWS keys, private keys, JWTs, and OpenAI keys
  are scrubbed before any sidecar or `.bak` is written (on by default).
- **`.bak` retention** — old backups are swept after a configurable window (default 7 days).
- **Record chain intact** — `compact` only rewrites content payloads; it never removes a
  record or alters the `uuid`/`parentUuid` links, so Claude Code can still load the session.

## Configuration (optional)

Config lives at your platform config dir:
- **macOS**: `~/Library/Application Support/contextzip/config.toml`
- **Linux**: `~/.config/contextzip/config.toml`

The `[compact]` section:

```toml
[compact]
redact = true                    # scrub secrets before writing (keep this on)
backup_retention_days = 7        # auto-remove .bak files older than this (0 = never)
aggressive = false               # enable aggressive axes without the CLI flag
generic_cap_chars = 4000         # oversized-result cap thresholds
generic_cap_lines = 200
include_paths_in_markers = true  # show absolute paths in dedup markers
```

Setting `aggressive = true` here is equivalent to always passing `--aggressive`.
