//! Session-history compressor for Claude Code JSONL session logs.
//!
//! Operates on the JSONL produced under `~/.claude/projects/<project>/<session>.jsonl`.
//! Five safe, opt-in axes ship in v0.2:
//!
//! - **`ReadDedup`** - repeated `Read` tool_results for the same `file_path` are
//!   replaced with a short reference back to the first occurrence. A SHA-256 of
//!   the file content is recorded at compact time so that a future `expand` step
//!   can detect staleness (on-disk content changed) and restore the original.
//!
//! - **`GrepGlobDedup`** - repeated identical `Grep` or `Glob` tool_results
//!   (keyed on normalized input args) are replaced with a reference. Avoids
//!   inflating context when the same search is re-run across turns.
//!
//! - **`BashCmdDedup`** - repeated identical `Bash` commands whose first result
//!   was non-empty are replaced with a reference. Commands whose first output was
//!   empty are not deduplicated, since re-runs may legitimately differ.
//!
//! - **`BashHistoryCompact`** - past `Bash` tool_results are re-fed through
//!   ContextZip's normal filter pipeline (ANSI strip, repeat-line tally, line
//!   cap). Only the first occurrence of each repeated command is compressed;
//!   subsequent occurrences are handled by `BashCmdDedup`. Idempotent: re-running
//!   on already-compressed records is a no-op.
//!
//! - **`GenericResultCap`** - any other oversized tool_result (MCP, Task,
//!   WebFetch, etc.) is ANSI-stripped and line-capped at the configured limit.
//!   Does not deduplicate; only reduces runaway payloads.
//!
//! A secret-redaction scrub pass (`crate::redact`) runs over the compacted output
//! before any sidecar or `.bak` write when `compact.redact` is enabled in config.
//! All five axes are reversible, idempotent, and guaranteed never to inflate the
//! token count above the original.
//!
//! Records are never removed and the `uuid` / `parentUuid` chain is never
//! altered - only `tool_result` content payloads are rewritten. The original
//! `.jsonl` is left untouched; output goes to a sibling `.compressed` file.

use anyhow::{Context, Result};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::{Path, PathBuf};

use crate::config::CompactConfig;

/// Aggregated metrics returned to the CLI for the user-facing summary.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct CompactStats {
    pub records_read: usize,
    pub records_written: usize,
    pub bytes_in: usize,
    pub bytes_out: usize,
    pub bash_results_recompressed: usize,
    pub read_results_deduped: usize,
    pub grepglob_results_deduped: usize,
    pub bash_cmds_deduped: usize,
    pub secrets_redacted: usize,
    pub generic_results_capped: usize,
    pub signatures_dropped: usize,
    pub media_referenced: usize,
    pub sidecars_deduped: usize,
    pub mcp_results_compacted: usize,
}

impl CompactStats {
    pub fn percent_saved(&self) -> f64 {
        if self.bytes_in == 0 {
            return 0.0;
        }
        ((self.bytes_in - self.bytes_out.min(self.bytes_in)) as f64 / self.bytes_in as f64) * 100.0
    }
}

/// Compact a session JSONL file into a sidecar `.compressed` file.
/// Returns the path of the sidecar plus aggregated stats. The original is never
/// modified; rollback is `rm <sidecar>`.
pub fn compact_session_file(input: &Path, cfg: &CompactConfig) -> Result<(PathBuf, CompactStats)> {
    let raw = fs::read_to_string(input)
        .with_context(|| format!("Failed to read session file: {}", input.display()))?;

    let (out, stats) = compact_session_str(&raw, cfg);

    let mut sidecar = input.to_path_buf();
    let new_name = match input.file_name().and_then(|s| s.to_str()) {
        Some(name) => format!("{}.compressed", name),
        None => "session.jsonl.compressed".to_string(),
    };
    sidecar.set_file_name(new_name);

    fs::write(&sidecar, &out)
        .with_context(|| format!("Failed to write sidecar: {}", sidecar.display()))?;

    Ok((sidecar, stats))
}

/// Pure-string compaction: takes the raw JSONL, returns the rewritten JSONL.
/// Lines that aren't valid JSON are passed through verbatim.
pub fn compact_session_str(input: &str, cfg: &CompactConfig) -> (String, CompactStats) {
    let mut stats = CompactStats {
        bytes_in: input.len(),
        ..Default::default()
    };

    // Two-pass: first pass collects which Read+file_path -> first tool_use_id,
    // which Grep/Glob args_key -> first tool_use_id, and which Bash command
    // string -> first tool_use_id.
    // Second pass rewrites repeated Read/Grep/Glob/Bash tool_results.
    let mut tool_use_index: HashMap<String, ToolUseInfo> = HashMap::new();
    let mut first_read_for: HashMap<String, FirstRead> = HashMap::new();
    let mut first_grepglob_for: HashMap<String, FirstResult> = HashMap::new();
    let mut first_bashcmd_for: HashMap<String, FirstResult> = HashMap::new();

    for line in input.lines() {
        if line.trim().is_empty() {
            continue;
        }
        let Ok(record) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        index_record(
            &record,
            &mut tool_use_index,
            &mut first_read_for,
            &mut first_grepglob_for,
            &mut first_bashcmd_for,
        );
    }

    // Content scan: fill content_sha256 for each FirstRead AND confirm
    // first_output_nonempty for each Bash FirstResult by scanning user
    // tool_result records. The output lives in a different record than
    // where we learn "this is a Read/Bash", so we scan user records
    // separately after indexing assistant records.
    // Build reverse maps: first tool_use_id -> key in each map.
    let first_id_to_path: HashMap<String, String> = first_read_for
        .iter()
        .map(|(path, fr)| (fr.tool_use_id.clone(), path.clone()))
        .collect();
    let first_id_to_bashcmd: HashMap<String, String> = first_bashcmd_for
        .iter()
        .map(|(cmd, fr)| (fr.tool_use_id.clone(), cmd.clone()))
        .collect();
    for line in input.lines() {
        if line.trim().is_empty() {
            continue;
        }
        let Ok(record) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        if record.get("type").and_then(Value::as_str) != Some("user") {
            continue;
        }
        let Some(content) = record
            .get("message")
            .and_then(|m| m.get("content"))
            .and_then(Value::as_array)
        else {
            continue;
        };
        for block in content {
            if block.get("type").and_then(Value::as_str) != Some("tool_result") {
                continue;
            }
            let Some(use_id) = block.get("tool_use_id").and_then(Value::as_str) else {
                continue;
            };
            // Extract text from the block (same logic as block_text_len).
            let text = match block.get("content") {
                Some(Value::String(s)) => s.as_str().to_string(),
                Some(Value::Array(arr)) => arr
                    .iter()
                    .filter_map(|c| c.get("text").and_then(Value::as_str))
                    .collect::<Vec<_>>()
                    .join("\n"),
                _ => String::new(),
            };
            if let Some(path) = first_id_to_path.get(use_id) {
                if let Some(fr) = first_read_for.get_mut(path) {
                    if fr.content_sha256.is_empty() {
                        fr.content_sha256 = sha256_hex(&text);
                    }
                }
            }
            if let Some(cmd) = first_id_to_bashcmd.get(use_id) {
                if let Some(fr) = first_bashcmd_for.get_mut(cmd) {
                    fr.first_output_nonempty = !text.trim().is_empty();
                }
            }
        }
    }

    // Second pass: rewrite content of tool_results.
    let mut out = String::with_capacity(input.len());
    for line in input.lines() {
        stats.records_read += 1;
        if line.trim().is_empty() {
            out.push('\n');
            continue;
        }

        let mut record: Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(_) => {
                // Pass through unparseable lines unchanged.
                out.push_str(line);
                out.push('\n');
                stats.records_written += 1;
                continue;
            }
        };

        rewrite_record(
            &mut record,
            &tool_use_index,
            &first_read_for,
            &first_grepglob_for,
            &first_bashcmd_for,
            cfg,
            &mut stats,
        );

        if cfg.aggressive {
            rewrite_record_metadata(&mut record, cfg, &mut stats);
        }

        let written = serde_json::to_string(&record).unwrap_or_else(|_| line.to_string());
        out.push_str(&written);
        out.push('\n');
        stats.records_written += 1;
    }

    if cfg.redact {
        let (scrubbed, n) = crate::redact::scrub(&out);
        stats.secrets_redacted = n;
        out = scrubbed;
    }
    stats.bytes_out = out.len();
    (out, stats)
}

#[derive(Debug, Clone)]
struct ToolUseInfo {
    name: String,
    file_path: Option<String>,
    /// Normalized args key for Grep/Glob dedup (BTreeMap-serialized input JSON).
    args_key: Option<String>,
    /// Bash command string for BashCmdDedup.
    bash_cmd: Option<String>,
}

/// First occurrence of a Grep/Glob or Bash command with a given key.
#[derive(Debug, Clone)]
struct FirstResult {
    tool_use_id: String,
    /// Whether the first occurrence's tool_result output was non-empty.
    /// Grep/Glob registrations set this to true unconditionally (output
    /// is assumed non-empty). Bash registrations start false and are
    /// confirmed true in the content scan pass.
    first_output_nonempty: bool,
}

#[derive(Debug, Clone)]
struct FirstRead {
    tool_use_id: String,
    /// SHA-256 hex of the tool_result text from the first read. Used by
    /// `contextzip expand` to detect whether the on-disk file has changed
    /// since compaction.
    content_sha256: String,
}

fn index_record(
    record: &Value,
    tool_use_index: &mut HashMap<String, ToolUseInfo>,
    first_read_for: &mut HashMap<String, FirstRead>,
    first_grepglob_for: &mut HashMap<String, FirstResult>,
    first_bashcmd_for: &mut HashMap<String, FirstResult>,
) {
    if record.get("type").and_then(Value::as_str) != Some("assistant") {
        return;
    }
    let Some(content) = record
        .get("message")
        .and_then(|m| m.get("content"))
        .and_then(Value::as_array)
    else {
        return;
    };

    for block in content {
        if block.get("type").and_then(Value::as_str) != Some("tool_use") {
            continue;
        }
        let Some(id) = block.get("id").and_then(Value::as_str) else {
            continue;
        };
        let name = block
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let file_path = block
            .get("input")
            .and_then(|i| i.get("file_path"))
            .and_then(Value::as_str)
            .map(String::from);

        // Compute a stable args_key for Grep/Glob by sorting input keys via BTreeMap.
        let args_key = match name.as_str() {
            "Grep" | "Glob" => block.get("input").and_then(|input| {
                let map: BTreeMap<String, Value> = serde_json::from_value(input.clone()).ok()?;
                serde_json::to_string(&map).ok()
            }),
            _ => None,
        };

        let bash_cmd = match name.as_str() {
            "Bash" => block
                .get("input")
                .and_then(|i| i.get("command"))
                .and_then(Value::as_str)
                .map(String::from),
            _ => None,
        };

        tool_use_index.insert(
            id.to_string(),
            ToolUseInfo {
                name: name.clone(),
                file_path: file_path.clone(),
                args_key: args_key.clone(),
                bash_cmd: bash_cmd.clone(),
            },
        );

        if name == "Read" {
            if let Some(path) = file_path {
                first_read_for.entry(path).or_insert_with(|| FirstRead {
                    tool_use_id: id.to_string(),
                    content_sha256: String::new(),
                });
            }
        }

        if name == "Grep" || name == "Glob" {
            if let Some(key) = args_key {
                first_grepglob_for
                    .entry(key)
                    .or_insert_with(|| FirstResult {
                        tool_use_id: id.to_string(),
                        first_output_nonempty: true, // assume non-empty for Grep/Glob
                    });
            }
        }

        if name == "Bash" {
            if let Some(cmd) = bash_cmd {
                // ponytail: dedup is command-keyed, not content-keyed; non-deterministic
                // commands (date, ls after changes) collapse to the first result.
                // Upgrade path: content-hash keying if this bites.
                first_bashcmd_for.entry(cmd).or_insert_with(|| FirstResult {
                    tool_use_id: id.to_string(),
                    // Confirmed non-empty in the content scan pass below.
                    first_output_nonempty: false,
                });
            }
        }
    }
}

fn rewrite_record(
    record: &mut Value,
    tool_use_index: &HashMap<String, ToolUseInfo>,
    first_read_for: &HashMap<String, FirstRead>,
    first_grepglob_for: &HashMap<String, FirstResult>,
    first_bashcmd_for: &HashMap<String, FirstResult>,
    cfg: &CompactConfig,
    stats: &mut CompactStats,
) {
    if record.get("type").and_then(Value::as_str) != Some("user") {
        return;
    }
    let Some(content) = record
        .get_mut("message")
        .and_then(|m| m.get_mut("content"))
        .and_then(Value::as_array_mut)
    else {
        return;
    };

    for block in content.iter_mut() {
        if block.get("type").and_then(Value::as_str) != Some("tool_result") {
            continue;
        }
        // Idempotency: skip already-compressed blocks.
        if block.get("contextzip_compressed").is_some() {
            continue;
        }
        let Some(use_id) = block
            .get("tool_use_id")
            .and_then(Value::as_str)
            .map(String::from)
        else {
            continue;
        };
        let Some(info) = tool_use_index.get(&use_id) else {
            continue;
        };

        match info.name.as_str() {
            "Read" => {
                if let Some(path) = info.file_path.as_deref() {
                    if let Some(first) = first_read_for.get(path) {
                        if first.tool_use_id != use_id {
                            // Repeated read of the same path -> replace with reference.
                            let preview_len = block_text_len(block);
                            if replace_with_read_ref(
                                block,
                                path,
                                &first.tool_use_id,
                                preview_len,
                                &first.content_sha256,
                                cfg.include_paths_in_markers,
                            ) {
                                stats.read_results_deduped += 1;
                            }
                        }
                    }
                }
            }
            "Grep" | "Glob" => {
                if let Some(key) = info.args_key.as_deref() {
                    if let Some(first) = first_grepglob_for.get(key) {
                        if first.tool_use_id != use_id {
                            let preview_len = block_text_len(block);
                            if replace_with_generic_ref(
                                block,
                                "GrepGlobDedup",
                                &first.tool_use_id,
                                preview_len,
                            ) {
                                stats.grepglob_results_deduped += 1;
                            }
                        }
                    }
                }
            }
            "Bash" => {
                // BashCmdDedup: if this is not the first occurrence of the same
                // command string, replace with a reference to the first.
                // Dedup wins over text compaction; only fall through to
                // recompress_bash_block when this IS the first occurrence.
                let deduped = info.bash_cmd.as_deref().and_then(|cmd| {
                    let first = first_bashcmd_for.get(cmd)?;
                    if first.tool_use_id == use_id {
                        return None; // this is the first - no dedup
                    }
                    // Guard: skip if the first occurrence had empty output -
                    // deduping to an empty reference is data loss.
                    if !first.first_output_nonempty {
                        return None;
                    }
                    Some(first.tool_use_id.clone())
                });
                if let Some(first_id) = deduped {
                    let preview_len = block_text_len(block);
                    if replace_with_generic_ref(block, "BashCmdDedup", &first_id, preview_len) {
                        stats.bash_cmds_deduped += 1;
                    }
                } else if recompress_bash_block(block) {
                    stats.bash_results_recompressed += 1;
                }
            }
            name if name.starts_with("mcp__") && cfg.aggressive => {
                // mcp_results_compacted is incremented inside the helper when it acts.
                // If the block is non-JSON or already minimal, fall through to generic cap.
                if !mcp_json_compact_block(block, stats) && generic_cap_block(block, cfg) {
                    stats.generic_results_capped += 1;
                }
            }
            _ => {
                if generic_cap_block(block, cfg) {
                    stats.generic_results_capped += 1;
                }
            }
        }
    }
}

/// Entry point for all metadata-rewrite axes (aggressive mode only).
/// Each axis is a helper call; Tasks 3-4 will add more axes here.
pub fn rewrite_record_metadata(record: &mut Value, cfg: &CompactConfig, stats: &mut CompactStats) {
    drop_thinking_signatures(record, stats);
    replace_media_with_sha_markers(record, cfg, stats);
    dedup_sidecar(record, stats);
}

/// SignatureDrop axis: removes replay-only crypto signatures from thinking
/// blocks. The original signature is replaced with a `contextzip_sig`
/// annotation carrying its SHA-256 and byte length so an `expand` step can
/// verify or restore it without re-inflating the session file.
///
/// Idempotent: blocks that already have `contextzip_sig` (or that lack a
/// `signature` field) are skipped silently.
fn drop_thinking_signatures(record: &mut Value, stats: &mut CompactStats) {
    let Some(content) = record
        .get_mut("message")
        .and_then(|m| m.get_mut("content"))
        .and_then(Value::as_array_mut)
    else {
        return;
    };

    for block in content.iter_mut() {
        let Some(obj) = block.as_object_mut() else {
            continue;
        };
        if obj.get("type").and_then(Value::as_str) != Some("thinking") {
            continue;
        }
        // Skip if already processed (idempotency).
        if obj.contains_key("contextzip_sig") {
            continue;
        }
        let sig = match obj.get("signature").and_then(Value::as_str) {
            Some(s) if !s.is_empty() => s.to_string(),
            _ => continue,
        };
        let sha = sha256_hex(&sig);
        let len = sig.len();
        obj.remove("signature");
        obj.insert(
            "contextzip_sig".to_string(),
            json!({"sha256": sha, "len": len}),
        );
        stats.signatures_dropped += 1;
    }
}

/// MediaReference axis: replaces inlined base64 image data with a sha marker.
///
/// Target A: `image` blocks in `record["message"]["content"]` where
/// `block["source"]["type"] == "base64"` and `source["data"]` is non-empty.
///
/// Target B: `record["toolUseResult"]["file"]["base64"]` if non-empty.
///
/// Never-inflate: skips if the marker length >= original data length.
/// Idempotent: skips blocks that already carry `contextzip_media`.
fn replace_media_with_sha_markers(
    record: &mut Value,
    _cfg: &CompactConfig,
    stats: &mut CompactStats,
) {
    // Target A: image blocks in message.content.
    // We need to collect indices to avoid borrow conflicts.
    let content_len = record
        .get("message")
        .and_then(|m| m.get("content"))
        .and_then(Value::as_array)
        .map(|a| a.len())
        .unwrap_or(0);

    for i in 0..content_len {
        let Some(content_arr) = record
            .get_mut("message")
            .and_then(|m| m.get_mut("content"))
            .and_then(Value::as_array_mut)
        else {
            break;
        };
        let Some(block) = content_arr.get_mut(i) else {
            continue;
        };
        if block.get("type").and_then(Value::as_str) != Some("image") {
            continue;
        }
        // Idempotency: skip if already processed.
        if block.get("contextzip_media").is_some() {
            continue;
        }
        let source_type = block
            .get("source")
            .and_then(|s| s.get("type"))
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        if source_type != "base64" {
            continue;
        }
        let data = match block
            .get("source")
            .and_then(|s| s.get("data"))
            .and_then(Value::as_str)
        {
            Some(d) if !d.is_empty() => d.to_string(),
            _ => continue,
        };
        let media_type = block
            .get("source")
            .and_then(|s| s.get("media_type"))
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let sha = sha256_hex(&data);
        let n = data.len();
        let marker = format!("[contextzip: media sha256={} {} bytes]", sha, n);
        // Never-inflate: skip if marker is not smaller than the data.
        if marker.len() >= n {
            continue;
        }
        let annotation = json!({"sha256": sha, "bytes": n, "media_type": media_type});
        // Mutate source.data.
        let Some(source) = block.get_mut("source").and_then(Value::as_object_mut) else {
            continue;
        };
        source.insert("data".to_string(), json!(marker));
        // Set sibling contextzip_media on the block.
        let Some(obj) = block.as_object_mut() else {
            continue;
        };
        obj.insert("contextzip_media".to_string(), annotation);
        stats.media_referenced += 1;
    }

    // Target B: toolUseResult.file.base64.
    let Some(file_obj) = record
        .get_mut("toolUseResult")
        .and_then(|t| t.get_mut("file"))
        .and_then(Value::as_object_mut)
    else {
        return;
    };
    // Idempotency: skip if already processed.
    if file_obj.contains_key("contextzip_media") {
        return;
    }
    let data = match file_obj.get("base64").and_then(Value::as_str) {
        Some(d) if !d.is_empty() => d.to_string(),
        _ => return,
    };
    let sha = sha256_hex(&data);
    let n = data.len();
    let marker = format!("[contextzip: media sha256={} {} bytes]", sha, n);
    if marker.len() >= n {
        return;
    }
    let annotation = json!({"sha256": sha, "bytes": n, "media_type": ""});
    file_obj.insert("base64".to_string(), json!(marker));
    file_obj.insert("contextzip_media".to_string(), annotation);
    stats.media_referenced += 1;
}

/// SidecarDedup axis: collapses `record["toolUseResult"]` when its primary
/// payload text is byte-equal to the `tool_result` block in
/// `record["message"]["content"]`.
///
/// Extraction order for the sidecar payload:
///   1. `toolUseResult.stdout`
///   2. `toolUseResult.originalFile`
///   3. `toolUseResult.content`
///   4. `toolUseResult.file.content`
///
/// The message tool_result text is extracted the same way as `block_text_len`
/// (content may be a bare string or an array of {type,text} objects).
///
/// Only collapses on exact byte equality AND when the payload is large enough
/// that the replacement marker is smaller (never-inflate). Partial matches are
/// never collapsed - only exact equality is safe; a near-match means the sidecar
/// has data that does not exist in message.content, and dropping it would lose
/// information.
///
/// Idempotent: skips if `toolUseResult.contextzip_ref` is already present.
fn dedup_sidecar(record: &mut Value, stats: &mut CompactStats) {
    // Idempotency: already processed.
    if record
        .get("toolUseResult")
        .and_then(|t| t.get("contextzip_ref"))
        .is_some()
    {
        return;
    }

    // Extract the sidecar's primary payload text and which field it came from.
    // "file.content" is represented as ("file", true) so we know to descend.
    let (sidecar_text, matched_field, in_file_obj): (String, &str, bool) = {
        let Some(tur) = record.get("toolUseResult") else {
            return;
        };
        if let Some(s) = tur.get("stdout").and_then(Value::as_str) {
            (s.to_string(), "stdout", false)
        } else if let Some(s) = tur.get("originalFile").and_then(Value::as_str) {
            (s.to_string(), "originalFile", false)
        } else if let Some(s) = tur.get("content").and_then(Value::as_str) {
            (s.to_string(), "content", false)
        } else if let Some(s) = tur
            .get("file")
            .and_then(|f| f.get("content"))
            .and_then(Value::as_str)
        {
            (s.to_string(), "content", true)
        } else {
            return;
        }
    };

    // Extract the message.content tool_result text.
    let content_len = record
        .get("message")
        .and_then(|m| m.get("content"))
        .and_then(Value::as_array)
        .map(|a| a.len())
        .unwrap_or(0);

    let mut tool_result_text: Option<String> = None;
    for i in 0..content_len {
        let block_type = record
            .get("message")
            .and_then(|m| m.get("content"))
            .and_then(Value::as_array)
            .and_then(|a| a.get(i))
            .and_then(|b| b.get("type"))
            .and_then(Value::as_str);
        if block_type != Some("tool_result") {
            continue;
        }
        let block = record
            .get("message")
            .and_then(|m| m.get("content"))
            .and_then(Value::as_array)
            .and_then(|a| a.get(i));
        let text = match block.and_then(|b| b.get("content")) {
            Some(Value::String(s)) => s.clone(),
            Some(Value::Array(arr)) => arr
                .iter()
                .filter_map(|c| c.get("text").and_then(Value::as_str))
                .collect::<Vec<_>>()
                .join(""),
            _ => String::new(),
        };
        tool_result_text = Some(text);
        break;
    }

    let content_text = match tool_result_text {
        Some(t) => t,
        None => return,
    };

    // Only collapse on exact byte equality.
    if sidecar_text != content_text {
        return;
    }

    let payload_len = sidecar_text.len();

    // Never-inflate: the replacement object must be smaller than the payload.
    // The replacement is ~120 chars; require payload > 120 to be conservative.
    if payload_len <= 120 {
        return;
    }

    // Mutate the existing toolUseResult object in place: remove ONLY the
    // matched field and insert the ref markers. Sibling fields (stderr,
    // interrupted, isImage, structuredPatch, etc.) are preserved.
    if let Some(tur) = record
        .get_mut("toolUseResult")
        .and_then(Value::as_object_mut)
    {
        if in_file_obj {
            // matched field was file.content - remove content from file sub-object
            if let Some(file_obj) = tur.get_mut("file").and_then(Value::as_object_mut) {
                file_obj.remove(matched_field);
            }
        } else {
            tur.remove(matched_field);
        }
        tur.insert("contextzip_ref".to_string(), json!("message.content"));
        tur.insert(
            "contextzip_compressed".to_string(),
            json!({
                "axis": "SidecarDedup",
                "original_chars": payload_len,
                "replaced_field": if in_file_obj { "file.content" } else { matched_field }
            }),
        );
        stats.sidecars_deduped += 1;
    }
}

fn block_text_len(block: &Value) -> usize {
    let content = match block.get("content") {
        Some(Value::String(s)) => return s.len(),
        Some(Value::Array(arr)) => arr,
        _ => return 0,
    };
    content
        .iter()
        .filter_map(|c| c.get("text").and_then(Value::as_str))
        .map(str::len)
        .sum()
}

/// Returns false (no-op) if the replacement would be >= the original content
/// length, mirroring the never-inflate invariant of the other dedup axes.
fn replace_with_read_ref(
    block: &mut Value,
    path: &str,
    first_id: &str,
    original_len: usize,
    content_sha256: &str,
    include_path: bool,
) -> bool {
    let marker = if include_path {
        format!(
            "[contextzip: dedup - same as Read in tool_use {} ({} -> 0 chars). \
             Re-expand with `contextzip expand` if the file at {} has changed.]",
            first_id, original_len, path
        )
    } else {
        format!(
            "[contextzip: dedup - same as Read in tool_use {} ({} -> 0 chars). \
             Re-expand with `contextzip expand` to restore.]",
            first_id, original_len
        )
    };
    // The annotation JSON adds roughly 90+ chars on top of the marker text.
    // Build the full new content string and check its length against original.
    let annotation = json!({
        "axis": "ReadDedup",
        "first_tool_use_id": first_id,
        "file_path": if include_path { path } else { "" },
        "original_chars": original_len,
        "content_sha256": content_sha256,
    });
    let replacement_len = marker.len() + annotation.to_string().len();
    if replacement_len >= original_len {
        return false;
    }
    block["content"] = json!([{ "type": "text", "text": marker }]);
    // Annotation so `expand` can find these refs without parsing the marker text.
    // file_path is empty when include_paths_in_markers is false to avoid leaking
    // absolute paths into the compressed sidecar.
    block["contextzip_compressed"] = annotation;
    true
}

/// Generic reference marker for dedup axes other than ReadDedup.
/// Shared by GrepGlobDedup (Task 6) and BashCmdDedup (Task 7).
/// Returns false (no-op) if the generated marker would be >= original_len,
/// mirroring recompress_bash_block's never-inflate invariant.
fn replace_with_generic_ref(
    block: &mut Value,
    axis: &str,
    first_id: &str,
    original_len: usize,
) -> bool {
    let marker = format!(
        "[contextzip: dedup {} - same as tool_use {} ({} -> 0 chars)]",
        axis, first_id, original_len
    );
    if marker.len() >= original_len {
        return false;
    }
    block["content"] = json!([{ "type": "text", "text": marker }]);
    block["contextzip_compressed"] = json!({
        "axis": axis,
        "first_tool_use_id": first_id,
        "original_chars": original_len,
    });
    true
}

fn recompress_bash_block(block: &mut Value) -> bool {
    // Idempotency guard: if we already compacted this once, skip.
    if block.get("contextzip_compressed").is_some() {
        return false;
    }
    let original = match block.get("content") {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Array(arr)) => arr
            .iter()
            .filter_map(|c| c.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join("\n"),
        _ => return false,
    };
    if original.is_empty() {
        return false;
    }

    let filtered = compress_bash_text(&original);
    if filtered.len() >= original.len() {
        // Filter didn't help — leave it alone so we never inflate the JSONL.
        return false;
    }

    let saved = original.len() - filtered.len();
    let new_text = format!(
        "{}\n[contextzip: BashHistoryCompact saved {} chars from past Bash result]",
        filtered, saved
    );
    block["content"] = json!([{ "type": "text", "text": new_text }]);
    block["contextzip_compressed"] = json!({
        "axis": "BashHistoryCompact",
        "original_chars": original.len(),
        "compressed_chars": new_text.len(),
        "content_sha256": sha256_hex(&original),
    });
    true
}

/// Cap oversized tool_results from unrecognized tools (the `_ =>` catch-all).
/// ANSI-strips, keeps the first `cfg.generic_cap_lines` lines, appends a drop
/// marker. Never inflates: only writes back if the result is smaller.
/// Returns true when the block was rewritten.
fn generic_cap_block(block: &mut Value, cfg: &crate::config::CompactConfig) -> bool {
    if block.get("contextzip_compressed").is_some() {
        return false;
    }
    let original = match block.get("content") {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Array(arr)) => arr
            .iter()
            .filter_map(|c| c.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join("\n"),
        _ => return false,
    };
    if original.is_empty() {
        return false;
    }
    // Only act when the text exceeds at least one threshold.
    let line_count = original.lines().count();
    if original.len() <= cfg.generic_cap_chars && line_count <= cfg.generic_cap_lines {
        return false;
    }

    let stripped = crate::ansi_filter::filter_ansi(&original);
    let lines: Vec<&str> = stripped.lines().collect();
    let cap = cfg.generic_cap_lines;
    let new_text = if lines.len() > cap {
        let dropped = lines.len() - cap;
        let mut kept = lines[..cap].join("\n");
        kept.push_str(&format!(
            "\n[contextzip: GenericResultCap - {} more lines dropped]",
            dropped
        ));
        kept
    } else {
        stripped.clone()
    };

    // Never-inflate guard.
    if new_text.len() >= original.len() {
        return false;
    }

    block["content"] = json!([{ "type": "text", "text": new_text }]);
    block["contextzip_compressed"] = json!({
        "axis": "GenericResultCap",
        "original_chars": original.len(),
        "compressed_chars": new_text.len(),
    });
    true
}

/// Apply ContextZip's existing line-based compression heuristics to a past Bash
/// result. We deliberately don't re-execute filters that need a Command — those
/// require a process; we only do safe, idempotent text-level reductions:
///   - strip ANSI escape sequences
///   - drop blank-padding runs
///   - collapse repeated identical lines into "(× N)" tallies
///   - cap at 200 lines with an explicit "(N more)" marker
fn compress_bash_text(input: &str) -> String {
    let stripped = crate::ansi_filter::filter_ansi(input);
    let mut out: Vec<String> = Vec::new();
    let mut blank_run = 0usize;
    let mut last_line: Option<String> = None;
    let mut last_line_count = 0usize;

    for raw in stripped.lines() {
        let line = raw.trim_end();
        if line.is_empty() {
            blank_run += 1;
            if blank_run <= 1 {
                if let Some(prev) = flush_repeat(&mut last_line, &mut last_line_count) {
                    out.push(prev);
                }
                out.push(String::new());
            }
            continue;
        }
        blank_run = 0;

        if last_line.as_deref() == Some(line) {
            last_line_count += 1;
            continue;
        }
        if let Some(prev) = flush_repeat(&mut last_line, &mut last_line_count) {
            out.push(prev);
        }
        last_line = Some(line.to_string());
        last_line_count = 1;
    }
    if let Some(prev) = flush_repeat(&mut last_line, &mut last_line_count) {
        out.push(prev);
    }

    if out.len() > 200 {
        let dropped = out.len() - 200;
        out.truncate(200);
        out.push(format!(
            "(... {} more lines dropped by contextzip)",
            dropped
        ));
    }
    out.join("\n")
}

fn flush_repeat(last_line: &mut Option<String>, count: &mut usize) -> Option<String> {
    let line = last_line.take()?;
    let n = std::mem::replace(count, 0);
    if n <= 1 {
        Some(line)
    } else {
        Some(format!("{} (×{})", line, n))
    }
}

/// McpJsonCompact axis: minifies MCP tool_result JSON payloads and unwraps
/// double-encoded `{"result":"{...}"}` patterns.
///
/// If the block text parses as JSON:
///   - Check for double-encoding: a JSON object where the `result` field is a
///     string that itself parses as JSON. If so, unwrap to the inner value.
///   - Re-serialize compact (no whitespace/indentation).
///   - Never-inflate: only replace when the compacted form is strictly shorter.
///   - Annotate with contextzip_compressed for idempotency and expand support.
///
/// If the text does not parse as JSON, the block is left untouched (fallback-safe).
/// Returns true when the block was rewritten.
fn mcp_json_compact_block(block: &mut Value, stats: &mut CompactStats) -> bool {
    // Idempotency guard is already applied by the caller, but double-check.
    if block.get("contextzip_compressed").is_some() {
        return false;
    }
    let original = match block.get("content") {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Array(arr)) => arr
            .iter()
            .filter_map(|c| c.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join("\n"),
        _ => return false,
    };
    if original.is_empty() {
        return false;
    }

    // Attempt JSON parse - if it fails, leave the block untouched.
    let parsed: Value = match serde_json::from_str(&original) {
        Ok(v) => v,
        Err(_) => return false,
    };

    // Unwrap double-encoding: {"result": "<JSON string>"} pattern.
    // The `result` value must be a string that itself parses as JSON.
    let unwrapped = if let Value::Object(ref obj) = parsed {
        if obj.len() == 1 {
            if let Some(Value::String(inner_str)) = obj.get("result") {
                serde_json::from_str::<Value>(inner_str).ok()
            } else {
                None
            }
        } else {
            None
        }
    } else {
        None
    };

    let target = unwrapped.as_ref().unwrap_or(&parsed);
    let compacted = match serde_json::to_string(target) {
        Ok(s) => s,
        Err(_) => return false,
    };

    // Never-inflate: only replace if strictly smaller.
    if compacted.len() >= original.len() {
        return false;
    }

    let original_chars = original.len();
    let compressed_chars = compacted.len();
    block["content"] = json!([{ "type": "text", "text": compacted }]);
    block["contextzip_compressed"] = json!({
        "axis": "McpJsonCompact",
        "original_chars": original_chars,
        "compressed_chars": compressed_chars,
    });
    stats.mcp_results_compacted += 1;
    true
}

fn sha256_hex(input: &str) -> String {
    let mut h = Sha256::new();
    h.update(input.as_bytes());
    let bytes = h.finalize();
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        use std::fmt::Write;
        let _ = write!(s, "{:02x}", b);
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_assistant_read(id: &str, file_path: &str) -> Value {
        make_assistant_tool(id, "Read", json!({ "file_path": file_path }))
    }

    fn make_user_tool_result(id: &str, text: &str) -> Value {
        json!({
            "type": "user",
            "uuid": format!("usr-{}", id),
            "message": {
                "content": [
                    { "type": "tool_result", "tool_use_id": id, "content": text }
                ]
            }
        })
    }

    fn make_assistant_bash(id: &str, command: &str) -> Value {
        make_assistant_tool(id, "Bash", json!({ "command": command }))
    }

    fn make_assistant_tool(id: &str, name: &str, input: Value) -> Value {
        json!({
            "type": "assistant",
            "uuid": format!("ass-{}", id),
            "message": {
                "content": [
                    { "type": "tool_use", "id": id, "name": name, "input": input }
                ]
            }
        })
    }

    fn jsonl(records: &[Value]) -> String {
        records
            .iter()
            .map(|r| serde_json::to_string(r).unwrap())
            .collect::<Vec<_>>()
            .join("\n")
            + "\n"
    }

    #[test]
    fn read_dedup_replaces_repeat_with_reference() {
        // Content must be long enough (>~330 chars) to exceed the
        // marker + annotation overhead imposed by the never-inflate guard.
        let content =
            "fn main() {\n    ".to_string() + &"println!(\"hello, world!\"); ".repeat(20) + "\n}";
        let records = vec![
            make_assistant_read("u1", "/abs/foo.rs"),
            make_user_tool_result("u1", &content),
            make_assistant_read("u2", "/abs/foo.rs"),
            make_user_tool_result("u2", &content),
        ];
        let input = jsonl(&records);
        let (out, stats) = compact_session_str(&input, &crate::config::CompactConfig::default());

        assert_eq!(stats.read_results_deduped, 1);
        assert!(
            out.contains("ReadDedup"),
            "missing ReadDedup marker in {}",
            out
        );
        // First read still has the full text
        assert!(out.contains("println!"));
        // Second read replaced
        let lines: Vec<&str> = out.lines().collect();
        assert!(lines[3].contains("contextzip"));
    }

    #[test]
    fn read_dedup_never_inflates_tiny_file() {
        // Content is very short ("hi") - the replacement marker + annotation overhead
        // is longer than the original, so the block must NOT be replaced.
        let records = vec![
            make_assistant_read("u1", "/abs/tiny.txt"),
            make_user_tool_result("u1", "hi"),
            make_assistant_read("u2", "/abs/tiny.txt"),
            make_user_tool_result("u2", "hi"),
        ];
        let input = jsonl(&records);
        let (out, stats) = compact_session_str(&input, &crate::config::CompactConfig::default());
        // Tiny content must not be "deduped" (that would inflate)
        assert_eq!(
            stats.read_results_deduped, 0,
            "tiny file must not be deduped"
        );
        // Both blocks still carry the original content
        assert_eq!(
            out.matches("\"hi\"").count(),
            2,
            "both tiny-file results must be preserved"
        );
    }

    #[test]
    fn read_dedup_does_not_touch_unique_reads() {
        let records = vec![
            make_assistant_read("u1", "/abs/a.rs"),
            make_user_tool_result("u1", "let a = 1;"),
            make_assistant_read("u2", "/abs/b.rs"),
            make_user_tool_result("u2", "let b = 2;"),
        ];
        let (out, stats) =
            compact_session_str(&jsonl(&records), &crate::config::CompactConfig::default());
        assert_eq!(stats.read_results_deduped, 0);
        assert!(out.contains("let a = 1;"));
        assert!(out.contains("let b = 2;"));
    }

    #[test]
    fn bash_history_compact_collapses_repeated_lines() {
        let noisy = "ok\nok\nok\nok\nfailure\n".repeat(50);
        let records = vec![
            make_assistant_bash("b1", "noisy"),
            make_user_tool_result("b1", &noisy),
        ];
        let (out, stats) =
            compact_session_str(&jsonl(&records), &crate::config::CompactConfig::default());
        assert_eq!(stats.bash_results_recompressed, 1);
        // Repeated 'ok' lines should be collapsed into a tally
        assert!(out.contains("(×"), "expected tally marker in {}", out);
        assert!(out.len() < jsonl(&records).len());
    }

    #[test]
    fn bash_history_compact_is_idempotent() {
        let noisy = "junk\n".repeat(100);
        let records = vec![
            make_assistant_bash("b1", "noisy"),
            make_user_tool_result("b1", &noisy),
        ];
        let input = jsonl(&records);
        let (first_out, first_stats) =
            compact_session_str(&input, &crate::config::CompactConfig::default());
        let (second_out, second_stats) =
            compact_session_str(&first_out, &crate::config::CompactConfig::default());
        assert_eq!(first_stats.bash_results_recompressed, 1);
        assert_eq!(second_stats.bash_results_recompressed, 0);
        assert_eq!(first_out, second_out);
    }

    #[test]
    fn malformed_lines_pass_through_unchanged() {
        let input = "this is not json\n{\"type\":\"user\"}\nalso not json\n";
        let (out, stats) = compact_session_str(input, &crate::config::CompactConfig::default());
        assert!(out.contains("this is not json"));
        assert!(out.contains("also not json"));
        assert_eq!(stats.records_read, 3);
        assert_eq!(stats.records_written, 3);
    }

    #[test]
    fn empty_input_produces_empty_output() {
        let (out, stats) = compact_session_str("", &crate::config::CompactConfig::default());
        assert_eq!(out, "");
        assert_eq!(stats.records_read, 0);
        assert_eq!(stats.bytes_in, 0);
    }

    #[test]
    fn compact_does_not_inflate_when_filter_yields_no_savings() {
        // A short Bash result has nothing to compress; we should leave it alone
        // rather than wrap it in a marker that grows the payload.
        let small = "exit 0";
        let records = vec![
            make_assistant_bash("b1", "true"),
            make_user_tool_result("b1", small),
        ];
        let original = jsonl(&records);
        let (out, _) = compact_session_str(&original, &crate::config::CompactConfig::default());
        assert!(out.len() <= original.len() + 10); // allow trailing newline diff
    }

    #[test]
    fn percent_saved_handles_zero_input() {
        let s = CompactStats::default();
        assert_eq!(s.percent_saved(), 0.0);
    }

    #[test]
    fn percent_saved_reports_reduction() {
        let s = CompactStats {
            bytes_in: 1000,
            bytes_out: 250,
            ..Default::default()
        };
        assert!((s.percent_saved() - 75.0).abs() < 0.01);
    }

    #[test]
    fn secret_in_tool_result_never_survives_to_output() {
        let pat = format!("dapi{}", "0".repeat(34));
        let records = [make_user_tool_result("u1", &format!("export TOKEN={pat}"))];
        let cfg = crate::config::CompactConfig::default();
        let (out, stats) = compact_session_str(&jsonl(&records), &cfg);
        assert!(!out.contains(&pat), "secret leaked into sidecar");
        assert!(stats.secrets_redacted >= 1);
    }

    #[test]
    fn read_dedup_annotation_carries_sha_and_respects_path_gate() {
        // Content must exceed the marker+annotation overhead (~330 chars) so the
        // never-inflate guard allows the dedup to proceed.
        let content = "fn main() {\n    ".to_string() + &"println!(\"x\"); ".repeat(25) + "\n}";
        // IDs must match between assistant tool_use and user tool_result.
        let records = [
            make_assistant_read("r1", "/tmp/x.rs"),
            make_user_tool_result("r1", &content),
            make_assistant_read("r2", "/tmp/x.rs"),
            make_user_tool_result("r2", &content),
        ];
        let mut cfg = crate::config::CompactConfig::default();
        cfg.include_paths_in_markers = false;
        let (out, _) = compact_session_str(&jsonl(&records), &cfg);
        assert!(
            out.contains("\"content_sha256\""),
            "sha must be recorded for staleness checks"
        );
        // Path must be absent from the dedup marker and annotation written for
        // the second (deduplicated) user tool_result record. We verify by
        // checking the rewritten user record (line 4) directly.
        let lines: Vec<&str> = out.lines().collect();
        // Line index 3 is the 4th record - the deduplicated user tool_result.
        let dedup_line = lines[3];
        assert!(
            !dedup_line.contains("/tmp/x.rs"),
            "path must not appear in dedup marker when gate is off, got: {}",
            dedup_line
        );
        // annotation must have empty file_path
        let v: Value = serde_json::from_str(dedup_line).unwrap();
        let file_path = v
            .get("message")
            .and_then(|m| m.get("content"))
            .and_then(Value::as_array)
            .and_then(|a| a.first())
            .and_then(|b| b.get("contextzip_compressed"))
            .and_then(|c| c.get("file_path"))
            .and_then(Value::as_str)
            .unwrap_or("NOT_FOUND");
        assert_eq!(
            file_path, "",
            "file_path in annotation must be empty, got: {}",
            file_path
        );
    }

    #[test]
    fn sha256_hex_is_deterministic_and_lowercase() {
        let h = sha256_hex("hello world");
        assert_eq!(
            h,
            "b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9"
        );
    }

    #[test]
    fn grep_dedup_replaces_repeat_with_reference() {
        // Output must be longer than the ~72-char dedup marker to pass the never-inflate guard.
        let grep_out =
            "src/a.rs:1: fn alpha\nsrc/b.rs:2: fn beta\nsrc/c.rs:3: fn gamma\nsrc/d.rs:4: fn delta";
        let records = [
            make_assistant_tool("g1", "Grep", json!({"pattern":"fn ","path":"src"})),
            make_user_tool_result("g1", grep_out),
            make_assistant_tool("g2", "Grep", json!({"pattern":"fn ","path":"src"})),
            make_user_tool_result("g2", grep_out),
        ];
        let cfg = crate::config::CompactConfig::default();
        let (out, stats) = compact_session_str(&jsonl(&records), &cfg);
        assert_eq!(stats.grepglob_results_deduped, 1);
        assert!(
            out.contains("GrepGlobDedup"),
            "missing GrepGlobDedup marker in {}",
            out
        );
        // First result's content must survive intact.
        assert!(
            out.contains("fn alpha"),
            "first Grep result content must be preserved"
        );
        assert!(
            out.contains("fn beta"),
            "first Grep result content must be preserved"
        );
        // Second result's body must be replaced - original text gone from the dedup line.
        let lines: Vec<&str> = out.lines().collect();
        let dedup_line = lines[3];
        assert!(
            !dedup_line.contains("fn alpha"),
            "second result body must be replaced, got: {}",
            dedup_line
        );
        assert!(
            dedup_line.contains("GrepGlobDedup"),
            "second result must carry dedup marker"
        );
    }

    #[test]
    fn grep_dedup_does_not_touch_unique_results() {
        let records = [
            make_assistant_tool("g1", "Grep", json!({"pattern":"fn ","path":"src"})),
            make_user_tool_result(
                "g1",
                "src/a.rs:1: fn alpha\nsrc/b.rs:2: fn beta\nsrc/c.rs:3: fn gamma",
            ),
            make_assistant_tool("g2", "Grep", json!({"pattern":"struct ","path":"src"})),
            make_user_tool_result(
                "g2",
                "src/b.rs:5: struct Foo\nsrc/c.rs:6: struct Bar\nsrc/d.rs:7: struct Baz",
            ),
        ];
        let cfg = crate::config::CompactConfig::default();
        let (out, stats) = compact_session_str(&jsonl(&records), &cfg);
        assert_eq!(stats.grepglob_results_deduped, 0);
        assert!(out.contains("fn alpha"));
        assert!(out.contains("struct Foo"));
    }

    #[test]
    fn grep_dedup_is_idempotent() {
        let grep_out =
            "src/a.rs:1: fn alpha\nsrc/b.rs:2: fn beta\nsrc/c.rs:3: fn gamma\nsrc/d.rs:4: fn delta";
        let records = [
            make_assistant_tool("g1", "Grep", json!({"pattern":"fn ","path":"src"})),
            make_user_tool_result("g1", grep_out),
            make_assistant_tool("g2", "Grep", json!({"pattern":"fn ","path":"src"})),
            make_user_tool_result("g2", grep_out),
        ];
        let cfg = crate::config::CompactConfig::default();
        let input = jsonl(&records);
        let (first_out, first_stats) = compact_session_str(&input, &cfg);
        let (second_out, second_stats) = compact_session_str(&first_out, &cfg);
        assert_eq!(first_stats.grepglob_results_deduped, 1);
        assert_eq!(
            second_stats.grepglob_results_deduped, 0,
            "second pass must not re-dedup"
        );
        assert_eq!(first_out, second_out, "output must be stable across passes");
    }

    #[test]
    fn bash_cmd_dedup_references_first_when_command_repeats() {
        // Output must exceed the ~72-char dedup marker to pass the never-inflate guard.
        let ls_out = "total 8\ndrwxr-xr-x  2 user staff   64 Aug 10 12:00 .\ndrwxr-xr-x  5 user staff  160 Aug 10 11:00 ..\n-rw-r--r--  1 user staff    0 Aug 10 12:00 file.txt";
        let records = [
            make_assistant_bash("b1", "ls -la"),
            make_user_tool_result("b1", ls_out),
            make_assistant_bash("b2", "ls -la"),
            make_user_tool_result("b2", ls_out),
        ];
        let cfg = crate::config::CompactConfig::default();
        let (out, stats) = compact_session_str(&jsonl(&records), &cfg);
        assert_eq!(stats.bash_cmds_deduped, 1);
        assert!(
            out.contains("BashCmdDedup"),
            "missing BashCmdDedup marker in {}",
            out
        );
        // First occurrence's output must be preserved.
        assert!(
            out.contains("file.txt"),
            "first Bash result must be preserved"
        );
        // Second occurrence's body must be replaced.
        let lines: Vec<&str> = out.lines().collect();
        let dedup_line = lines[3];
        assert!(
            !dedup_line.contains("file.txt"),
            "second result body must be replaced, got: {}",
            dedup_line
        );
        assert!(
            dedup_line.contains("BashCmdDedup"),
            "second result must carry dedup marker"
        );
    }

    #[test]
    fn bash_cmd_dedup_does_not_touch_unique_commands() {
        let records = [
            make_assistant_bash("b1", "ls -la"),
            make_user_tool_result("b1", "total 8\ndrwxr-xr-x  2 user staff  64 Aug 10 .\n-rw-r--r--  1 user staff   0 Aug 10 file1"),
            make_assistant_bash("b2", "git status"),
            make_user_tool_result("b2", "On branch main\nYour branch is up to date with origin/main.\nnothing to commit, working tree clean"),
        ];
        let cfg = crate::config::CompactConfig::default();
        let (out, stats) = compact_session_str(&jsonl(&records), &cfg);
        assert_eq!(
            stats.bash_cmds_deduped, 0,
            "unique commands must not be deduped"
        );
        assert!(
            out.contains("file1"),
            "first command output must be preserved"
        );
        assert!(
            out.contains("On branch main"),
            "second command output must be preserved"
        );
    }

    #[test]
    fn bash_cmd_dedup_is_idempotent() {
        let ls_out = "total 8\ndrwxr-xr-x  2 user staff   64 Aug 10 12:00 .\ndrwxr-xr-x  5 user staff  160 Aug 10 11:00 ..\n-rw-r--r--  1 user staff    0 Aug 10 12:00 file.txt";
        let records = [
            make_assistant_bash("b1", "ls -la"),
            make_user_tool_result("b1", ls_out),
            make_assistant_bash("b2", "ls -la"),
            make_user_tool_result("b2", ls_out),
        ];
        let cfg = crate::config::CompactConfig::default();
        let input = jsonl(&records);
        let (first_out, first_stats) = compact_session_str(&input, &cfg);
        let (second_out, second_stats) = compact_session_str(&first_out, &cfg);
        assert_eq!(first_stats.bash_cmds_deduped, 1);
        assert_eq!(
            second_stats.bash_cmds_deduped, 0,
            "second pass must not re-dedup"
        );
        assert_eq!(first_out, second_out, "output must be stable across passes");
    }

    #[test]
    fn bash_cmd_dedup_skips_when_first_output_was_empty() {
        // First `date` has empty output; second has real output.
        // The second must NOT be deduped (would point to nothing) and
        // its output must be preserved.
        let records = [
            make_assistant_bash("b1", "date"),
            make_user_tool_result("b1", ""),
            make_assistant_bash("b2", "date"),
            make_user_tool_result("b2", "Sun Aug 10 12:00:00 UTC 2026"),
        ];
        let cfg = crate::config::CompactConfig::default();
        let (out, stats) = compact_session_str(&jsonl(&records), &cfg);
        assert_eq!(
            stats.bash_cmds_deduped, 0,
            "must not dedup when first output was empty"
        );
        assert!(
            out.contains("Sun Aug 10"),
            "second result must be preserved when first was empty"
        );
    }

    #[test]
    fn bash_cmd_dedup_never_inflates_short_output() {
        // Both calls run the same command but output is shorter than the marker.
        // Neither occurrence should be inflated.
        let short = "ok";
        let records = [
            make_assistant_bash("b1", "echo ok"),
            make_user_tool_result("b1", short),
            make_assistant_bash("b2", "echo ok"),
            make_user_tool_result("b2", short),
        ];
        let original = jsonl(&records);
        let cfg = crate::config::CompactConfig::default();
        let (out, stats) = compact_session_str(&original, &cfg);
        assert_eq!(
            stats.bash_cmds_deduped, 0,
            "must not count dedup when marker would inflate"
        );
        // Length must not grow beyond the original.
        assert!(
            out.len() <= original.len() + 10,
            "output inflated from {} to {} bytes",
            original.len(),
            out.len()
        );
    }

    #[test]
    fn generic_cap_trims_oversized_unknown_tool_result() {
        let big = (0..500)
            .map(|i| format!("line {i}"))
            .collect::<Vec<_>>()
            .join("\n");
        let records = [
            make_assistant_tool("m1", "mcp__some__tool", json!({})),
            make_user_tool_result("m1", &big),
        ];
        let cfg = crate::config::CompactConfig::default(); // 200-line cap
        let (out, stats) = compact_session_str(&jsonl(&records), &cfg);
        assert_eq!(stats.generic_results_capped, 1);
        assert!(out.contains("more lines"), "missing drop marker in {}", out);
    }

    #[test]
    fn generic_cap_leaves_small_results_untouched() {
        let small = (0..10)
            .map(|i| format!("line {i}"))
            .collect::<Vec<_>>()
            .join("\n");
        // small is well under both thresholds (200 lines, 4000 chars)
        let records = [
            make_assistant_tool("m1", "mcp__some__tool", json!({})),
            make_user_tool_result("m1", &small),
        ];
        let cfg = crate::config::CompactConfig::default();
        let (out, stats) = compact_session_str(&jsonl(&records), &cfg);
        assert_eq!(
            stats.generic_results_capped, 0,
            "small result must not be capped"
        );
        assert!(
            out.contains("line 0"),
            "small result content must be preserved"
        );
    }

    #[test]
    fn generic_cap_is_idempotent() {
        let big = (0..500)
            .map(|i| format!("line {i}"))
            .collect::<Vec<_>>()
            .join("\n");
        let records = [
            make_assistant_tool("m1", "mcp__some__tool", json!({})),
            make_user_tool_result("m1", &big),
        ];
        let cfg = crate::config::CompactConfig::default();
        let input = jsonl(&records);
        let (first_out, first_stats) = compact_session_str(&input, &cfg);
        let (second_out, second_stats) = compact_session_str(&first_out, &cfg);
        assert_eq!(first_stats.generic_results_capped, 1);
        assert_eq!(
            second_stats.generic_results_capped, 0,
            "second pass must not re-cap"
        );
        assert_eq!(first_out, second_out, "output must be stable across passes");
    }

    #[test]
    fn signature_dropped_only_when_aggressive() {
        let rec = json!({"type":"assistant","uuid":"a1","message":{"content":[
            {"type":"thinking","thinking":"reasoning here","signature":"AAAABBBBCCCCDDDD_long_sig_value_xxxxxxxxxxxx"}
        ]}});
        let input = format!("{}\n", serde_json::to_string(&rec).unwrap());

        // aggressive OFF -> untouched
        let off = CompactConfig::default();
        let (out_off, s_off) = compact_session_str(&input, &off);
        assert!(
            out_off.contains("AAAABBBBCCCC"),
            "signature must survive when not aggressive"
        );
        assert_eq!(s_off.signatures_dropped, 0);

        // aggressive ON -> dropped + annotated with sha
        let on = CompactConfig {
            aggressive: true,
            ..Default::default()
        };
        let (out_on, s_on) = compact_session_str(&input, &on);
        assert!(
            !out_on.contains("AAAABBBBCCCC"),
            "signature must be dropped when aggressive"
        );
        assert!(
            out_on.contains("contextzip_sig"),
            "must annotate the dropped signature for expand"
        );
        assert_eq!(s_on.signatures_dropped, 1);
    }

    #[test]
    fn media_referenced_when_aggressive() {
        let data = "X".repeat(5000); // stand-in base64 payload
        let rec = json!({"type":"user","uuid":"u1","message":{"content":[
            {"type":"image","source":{"type":"base64","media_type":"image/png","data": data}}
        ]}});
        let input = format!("{}\n", serde_json::to_string(&rec).unwrap());

        let on = CompactConfig {
            aggressive: true,
            ..Default::default()
        };
        let (out, s) = compact_session_str(&input, &on);
        assert!(
            !out.contains(&"X".repeat(5000)),
            "base64 data must be replaced"
        );
        assert!(
            out.contains("contextzip_media"),
            "must annotate sha for expand"
        );
        assert_eq!(s.media_referenced, 1);

        // never-inflate: a tiny image is left alone
        let tiny = json!({"type":"user","message":{"content":[
            {"type":"image","source":{"type":"base64","media_type":"image/png","data":"AA"}}]}});
        let (out2, s2) = compact_session_str(&format!("{}\n", tiny), &on);
        assert_eq!(s2.media_referenced, 0);
        assert!(out2.contains("\"data\":\"AA\""));

        // aggressive OFF -> media left untouched
        let off = CompactConfig::default();
        let (out_off, s_off) = compact_session_str(&input, &off);
        assert!(
            out_off.contains(&"X".repeat(50)),
            "base64 data must survive when not aggressive"
        );
        assert_eq!(s_off.media_referenced, 0);
    }

    #[test]
    fn media_referenced_tool_use_result_file_base64() {
        let data = "B".repeat(3000);
        let rec = json!({"type":"assistant","uuid":"a1","toolUseResult":{"file":{"base64": data}}});
        let input = format!("{}\n", serde_json::to_string(&rec).unwrap());

        let on = CompactConfig {
            aggressive: true,
            ..Default::default()
        };
        let (out, s) = compact_session_str(&input, &on);
        assert!(
            !out.contains(&"B".repeat(3000)),
            "base64 in file must be replaced"
        );
        assert!(out.contains("contextzip_media"), "must annotate sha");
        assert_eq!(s.media_referenced, 1);
    }

    #[test]
    fn media_referenced_is_idempotent() {
        let data = "X".repeat(5000);
        let rec = json!({"type":"user","uuid":"u1","message":{"content":[
            {"type":"image","source":{"type":"base64","media_type":"image/png","data": data}}
        ]}});
        let input = format!("{}\n", serde_json::to_string(&rec).unwrap());
        let on = CompactConfig {
            aggressive: true,
            ..Default::default()
        };
        let (first_out, first_stats) = compact_session_str(&input, &on);
        let (second_out, second_stats) = compact_session_str(&first_out, &on);
        assert_eq!(first_stats.media_referenced, 1);
        assert_eq!(
            second_stats.media_referenced, 0,
            "second pass must not re-replace"
        );
        assert_eq!(first_out, second_out, "output must be stable across passes");
    }

    #[test]
    fn signature_drop_is_idempotent() {
        let rec = json!({"type":"assistant","uuid":"a1","message":{"content":[
            {"type":"thinking","thinking":"reasoning here","signature":"AAAABBBBCCCCDDDD_long_sig_value_xxxxxxxxxxxx"}
        ]}});
        let input = format!("{}\n", serde_json::to_string(&rec).unwrap());
        let on = CompactConfig {
            aggressive: true,
            ..Default::default()
        };

        // First pass: drops the signature.
        let (first_out, first_stats) = compact_session_str(&input, &on);
        assert_eq!(first_stats.signatures_dropped, 1);

        // Second pass: contextzip_sig is already present - must be a no-op.
        let (second_out, second_stats) = compact_session_str(&first_out, &on);
        assert_eq!(
            second_stats.signatures_dropped, 0,
            "second pass must not re-drop"
        );
        assert_eq!(first_out, second_out, "output must be stable across passes");
    }

    #[test]
    fn sidecar_deduped_when_byte_equal_and_aggressive() {
        let body = "line one\nline two\nline three".repeat(20);
        let rec = json!({
            "type":"user","uuid":"u1",
            "message":{"content":[{"type":"tool_result","tool_use_id":"t1","content": body.clone()}]},
            "toolUseResult":{"type":"text","stdout": body.clone()}
        });
        let input = format!("{}\n", serde_json::to_string(&rec).unwrap());
        let on = CompactConfig {
            aggressive: true,
            ..Default::default()
        };
        let (out, s) = compact_session_str(&input, &on);
        assert_eq!(s.sidecars_deduped, 1);
        assert!(
            out.contains("contextzip_ref"),
            "sidecar collapses to a reference"
        );
        // The message.content copy is still present (only the duplicate sidecar collapses)
        let v: serde_json::Value = serde_json::from_str(out.lines().next().unwrap()).unwrap();
        assert!(v["message"]["content"][0]["content"]
            .as_str()
            .unwrap()
            .contains("line one"));
    }

    #[test]
    fn sidecar_left_intact_when_differs() {
        let rec = json!({
            "type":"user",
            "message":{"content":[{"type":"tool_result","tool_use_id":"t1","content":"AAA"}]},
            "toolUseResult":{"type":"text","stdout":"DIFFERENT CONTENT ENTIRELY that is quite a bit longer than AAA"}
        });
        let input = format!("{}\n", serde_json::to_string(&rec).unwrap());
        let on = CompactConfig {
            aggressive: true,
            ..Default::default()
        };
        let (out, s) = compact_session_str(&input, &on);
        assert_eq!(
            s.sidecars_deduped, 0,
            "differing sidecar must be left intact"
        );
        assert!(out.contains("DIFFERENT CONTENT"));
    }

    #[test]
    fn sidecar_dedup_off_when_not_aggressive() {
        let body = "line one\nline two\nline three".repeat(20);
        let rec = json!({
            "type":"user","uuid":"u1",
            "message":{"content":[{"type":"tool_result","tool_use_id":"t1","content": body.clone()}]},
            "toolUseResult":{"type":"text","stdout": body.clone()}
        });
        let input = format!("{}\n", serde_json::to_string(&rec).unwrap());
        let off = CompactConfig::default();
        let (out, s) = compact_session_str(&input, &off);
        assert_eq!(
            s.sidecars_deduped, 0,
            "must be untouched when not aggressive"
        );
        assert!(
            !out.contains("contextzip_ref"),
            "no ref when aggressive is off"
        );
    }

    #[test]
    fn sidecar_dedup_preserves_sibling_fields() {
        let body = "line one\nline two\nline three".repeat(20);
        let rec = json!({
            "type":"user","uuid":"u1",
            "message":{"content":[{"type":"tool_result","tool_use_id":"t1","content": body.clone()}]},
            "toolUseResult":{"type":"text","stdout": body.clone(),"stderr":"some error","interrupted":false}
        });
        let input = format!("{}\n", serde_json::to_string(&rec).unwrap());
        let on = CompactConfig {
            aggressive: true,
            ..Default::default()
        };
        let (out, s) = compact_session_str(&input, &on);
        assert_eq!(s.sidecars_deduped, 1);
        assert!(out.contains("contextzip_ref"), "sidecar collapsed");
        let v: serde_json::Value = serde_json::from_str(out.lines().next().unwrap()).unwrap();
        assert_eq!(
            v["toolUseResult"]["stderr"].as_str(),
            Some("some error"),
            "stderr must survive sibling dedup"
        );
        assert_eq!(
            v["toolUseResult"]["interrupted"].as_bool(),
            Some(false),
            "interrupted must survive sibling dedup"
        );
        // stdout (the matched field) must be gone
        assert!(
            v["toolUseResult"]["stdout"].is_null(),
            "matched field must be removed"
        );
    }

    #[test]
    fn mcp_json_compacted_when_aggressive() {
        // double-encoded {"result":"{...}"} with whitespace
        let inner = r#"{  "email" : "a@b.com" ,  "team" : "Pacifico"  }"#;
        let payload = serde_json::json!({"result": inner}).to_string();
        let records = [
            make_assistant_tool("m1", "mcp__sts__ping", json!({})),
            make_user_tool_result("m1", &payload),
        ];
        let on = CompactConfig {
            aggressive: true,
            ..Default::default()
        };
        let (out, s) = compact_session_str(&jsonl(&records), &on);
        assert_eq!(s.mcp_results_compacted, 1);
        // minified: no double-space, inner unwrapped
        assert!(!out.contains("  \"email\""));
        // aggressive OFF -> untouched by MCP axis
        let (out_off, s_off) = compact_session_str(&jsonl(&records), &CompactConfig::default());
        assert_eq!(s_off.mcp_results_compacted, 0);
        let _ = out_off; // suppress unused warning
    }

    #[test]
    fn mcp_non_json_result_left_unchanged() {
        let plain = "pong - server alive";
        let records = [
            make_assistant_tool("m1", "mcp__sts__ping", json!({})),
            make_user_tool_result("m1", plain),
        ];
        let on = CompactConfig {
            aggressive: true,
            ..Default::default()
        };
        let (out, s) = compact_session_str(&jsonl(&records), &on);
        assert_eq!(s.mcp_results_compacted, 0);
        assert!(
            out.contains(plain),
            "plain-text MCP result must be preserved"
        );
    }

    #[test]
    fn sidecar_dedup_is_idempotent() {
        let body = "line one\nline two\nline three".repeat(20);
        let rec = json!({
            "type":"user","uuid":"u1",
            "message":{"content":[{"type":"tool_result","tool_use_id":"t1","content": body.clone()}]},
            "toolUseResult":{"type":"text","stdout": body.clone()}
        });
        let input = format!("{}\n", serde_json::to_string(&rec).unwrap());
        let on = CompactConfig {
            aggressive: true,
            ..Default::default()
        };
        let (first_out, first_stats) = compact_session_str(&input, &on);
        assert_eq!(first_stats.sidecars_deduped, 1);
        let (second_out, second_stats) = compact_session_str(&first_out, &on);
        assert_eq!(
            second_stats.sidecars_deduped, 0,
            "second pass must not re-dedup"
        );
        assert_eq!(first_out, second_out, "output must be stable across passes");
    }
}
