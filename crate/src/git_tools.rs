use std::path::Path;
use std::process::Command;

use serde_json::{Value, json};

const DIFF_CHAR_LIMIT: usize = 40_000;
const MAX_PATHS: usize = 32;

pub fn status_schema() -> Value {
    json!({
        "type": "object",
        "properties": {},
        "additionalProperties": false
    })
}

pub fn diff_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "staged": {
                "type": "boolean",
                "default": false,
                "description": "Compare staged changes instead of the working tree."
            },
            "base": {
                "type": ["string", "null"],
                "description": "Optional Git revision to diff against, e.g. HEAD or origin/main."
            },
            "paths": {
                "type": "array",
                "items": { "type": "string" },
                "maxItems": MAX_PATHS,
                "description": "Optional workspace-relative paths to restrict the diff."
            },
            "context": {
                "type": "integer",
                "minimum": 0,
                "maximum": 20,
                "default": 3,
                "description": "Unified diff context lines."
            },
            "stat": {
                "type": "boolean",
                "default": false,
                "description": "Return diff statistics instead of patch text."
            }
        },
        "additionalProperties": false
    })
}

pub fn status(cwd: &Path) -> Result<(String, Value), String> {
    let output = Command::new("git")
        .args(["status", "--porcelain=v2", "--branch"])
        .current_dir(cwd)
        .output()
        .map_err(|e| format!("run git status: {e}"))?;
    if !output.status.success() {
        return Err(command_error("git status", &output.stderr));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut oid = None;
    let mut branch = None;
    let mut upstream = None;
    let mut ahead = 0i64;
    let mut behind = 0i64;
    let mut entries = Vec::new();

    for line in stdout.lines() {
        if let Some(value) = line.strip_prefix("# branch.oid ") {
            oid = Some(value.to_string());
        } else if let Some(value) = line.strip_prefix("# branch.head ") {
            branch = Some(value.to_string());
        } else if let Some(value) = line.strip_prefix("# branch.upstream ") {
            upstream = Some(value.to_string());
        } else if let Some(value) = line.strip_prefix("# branch.ab ") {
            let mut parts = value.split_whitespace();
            ahead = parts
                .next()
                .and_then(|v| v.strip_prefix('+'))
                .and_then(|v| v.parse().ok())
                .unwrap_or(0);
            behind = parts
                .next()
                .and_then(|v| v.strip_prefix('-'))
                .and_then(|v| v.parse().ok())
                .unwrap_or(0);
        } else if !line.starts_with('#') && !line.is_empty() {
            entries.push(parse_status_entry(line));
        }
    }

    let dirty = !entries.is_empty();
    let mut summary = String::new();
    summary.push_str("branch: ");
    summary.push_str(branch.as_deref().unwrap_or("(unknown)"));
    if let Some(upstream) = upstream.as_deref() {
        summary.push_str(" -> ");
        summary.push_str(upstream);
    }
    if ahead != 0 || behind != 0 {
        summary.push_str(&format!(" (ahead {ahead}, behind {behind})"));
    }
    summary.push('\n');
    if dirty {
        for entry in &entries {
            let xy = entry.get("xy").and_then(Value::as_str).unwrap_or("??");
            let path = entry.get("path").and_then(Value::as_str).unwrap_or("");
            summary.push_str(&format!("{xy} {path}\n"));
        }
        if summary.ends_with('\n') {
            summary.pop();
        }
    } else {
        summary.push_str("working tree clean");
    }

    Ok((
        summary,
        json!({
            "branch": branch,
            "oid": oid,
            "upstream": upstream,
            "ahead": ahead,
            "behind": behind,
            "dirty": dirty,
            "entries": entries
        }),
    ))
}

pub fn diff(arguments: &Value, cwd: &Path) -> Result<(String, Value), String> {
    let staged = arguments
        .get("staged")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let stat = arguments
        .get("stat")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let context = arguments
        .get("context")
        .and_then(Value::as_u64)
        .unwrap_or(3)
        .min(20);
    let base = arguments
        .get("base")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty());
    if let Some(base) = base
        && (base.starts_with('-') || base.contains('\0'))
    {
        return Err("git_diff base must be a revision name, not a command option".into());
    }

    let paths = arguments
        .get("paths")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .filter(|path| !path.is_empty() && !path.contains('\0'))
                .take(MAX_PATHS)
                .map(str::to_string)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    let mut command = Command::new("git");
    command
        .current_dir(cwd)
        .arg("diff")
        .arg("--no-ext-diff")
        .arg("--no-color")
        .arg(format!("--unified={context}"));
    if staged {
        command.arg("--cached");
    }
    if stat {
        command.arg("--stat");
    }
    if let Some(base) = base {
        command.arg(base);
    }
    if !paths.is_empty() {
        command.arg("--");
        command.args(&paths);
    }

    let output = command.output().map_err(|e| format!("run git diff: {e}"))?;
    if !output.status.success() {
        return Err(command_error("git diff", &output.stderr));
    }

    let raw = String::from_utf8_lossy(&output.stdout).into_owned();
    let raw_chars = raw.chars().count();
    let (text, truncated) = truncate_middle(&raw, DIFF_CHAR_LIMIT);
    let text = if text.is_empty() {
        "(no diff)".into()
    } else {
        text
    };

    Ok((
        text,
        json!({
            "staged": staged,
            "base": base,
            "paths": paths,
            "context": context,
            "stat": stat,
            "rawChars": raw_chars,
            "truncated": truncated
        }),
    ))
}

fn parse_status_entry(line: &str) -> Value {
    if let Some(path) = line.strip_prefix("? ") {
        return json!({ "kind": "untracked", "xy": "??", "path": path });
    }
    if let Some(path) = line.strip_prefix("! ") {
        return json!({ "kind": "ignored", "xy": "!!", "path": path });
    }
    if line.starts_with("1 ") {
        let parts = line.splitn(9, ' ').collect::<Vec<_>>();
        return json!({
            "kind": "ordinary",
            "xy": parts.get(1).copied().unwrap_or(""),
            "path": parts.get(8).copied().unwrap_or("")
        });
    }
    if line.starts_with("2 ") {
        let parts = line.splitn(10, ' ').collect::<Vec<_>>();
        let path_field = parts.get(9).copied().unwrap_or("");
        let (path, original_path) = path_field.split_once('\t').unwrap_or((path_field, ""));
        return json!({
            "kind": "rename_or_copy",
            "xy": parts.get(1).copied().unwrap_or(""),
            "path": path,
            "originalPath": original_path
        });
    }
    if line.starts_with("u ") {
        let parts = line.splitn(11, ' ').collect::<Vec<_>>();
        return json!({
            "kind": "unmerged",
            "xy": parts.get(1).copied().unwrap_or("UU"),
            "path": parts.get(10).copied().unwrap_or("")
        });
    }
    json!({ "kind": "unknown", "xy": "??", "path": line })
}

fn truncate_middle(text: &str, limit: usize) -> (String, bool) {
    let count = text.chars().count();
    if count <= limit {
        return (text.to_string(), false);
    }
    let marker = format!("\n... diff truncated; {count} chars total ...\n");
    let keep = limit.saturating_sub(marker.chars().count());
    let head = keep * 2 / 3;
    let tail = keep - head;
    let start: String = text.chars().take(head).collect();
    let end: String = text
        .chars()
        .rev()
        .take(tail)
        .collect::<String>()
        .chars()
        .rev()
        .collect();
    (format!("{start}{marker}{end}"), true)
}

fn command_error(name: &str, stderr: &[u8]) -> String {
    let detail = String::from_utf8_lossy(stderr).trim().to_string();
    if detail.is_empty() {
        format!("{name} failed")
    } else {
        format!("{name} failed: {detail}")
    }
}

#[cfg(test)]
mod tests {
    use super::{parse_status_entry, truncate_middle};

    #[test]
    fn parses_untracked_status() {
        let value = parse_status_entry("? src/new file.rs");
        assert_eq!(value["kind"], "untracked");
        assert_eq!(value["path"], "src/new file.rs");
    }

    #[test]
    fn truncates_large_diffs() {
        let input = "x".repeat(1000);
        let (output, truncated) = truncate_middle(&input, 100);
        assert!(truncated);
        assert!(output.chars().count() <= 140);
        assert!(output.contains("truncated"));
    }
}
