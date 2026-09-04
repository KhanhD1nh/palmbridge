use std::path::{Path, PathBuf};
use std::time::SystemTime;

use globset::{Glob, GlobMatcher};
use ignore::WalkBuilder;
use serde_json::Value;

const RESULT_LIMIT: usize = 100;

pub fn run(arguments: &Value, cwd: &Path) -> Result<String, String> {
    let pattern = arguments
        .get("pattern")
        .and_then(Value::as_str)
        .ok_or("glob requires a non-empty pattern")?;
    if pattern.trim().is_empty() {
        return Err("glob requires a non-empty pattern".into());
    }

    let search_dir = arguments
        .get("path")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
        .map(|p| if p.is_absolute() { p } else { cwd.join(p) })
        .unwrap_or_else(|| cwd.to_path_buf());

    if !search_dir.is_dir() {
        return Err(format!("glob path is not a directory: {}", search_dir.display()));
    }

    let matcher = compile_matcher(pattern)?;
    let mut matches = Vec::new();

    let mut builder = WalkBuilder::new(&search_dir);
    builder
        .hidden(false)
        .git_ignore(true)
        .git_global(true)
        .git_exclude(true)
        .parents(true)
        .follow_links(false)
        .filter_entry(|entry| entry.file_name() != ".git");

    for entry in builder.build().flatten() {
        if !entry.file_type().is_some_and(|t| t.is_file()) {
            continue;
        }
        let path = entry.into_path();
        let rel = path.strip_prefix(&search_dir).unwrap_or(&path);
        let rel_normalized = rel.to_string_lossy().replace('\\', "/");
        if !matcher.is_match(&rel_normalized) && !matcher.is_match(&path) {
            continue;
        }
        let modified = path
            .metadata()
            .and_then(|m| m.modified())
            .unwrap_or(SystemTime::UNIX_EPOCH);
        matches.push((modified, path));
    }

    matches.sort_by(|a, b| b.0.cmp(&a.0));
    let total = matches.len();
    let shown = total.min(RESULT_LIMIT);
    let mut lines = matches
        .into_iter()
        .take(RESULT_LIMIT)
        .map(|(_, p)| p.display().to_string())
        .collect::<Vec<_>>();

    if total == 0 {
        return Ok(format!(
            "No files matched pattern `{pattern}` under {}.",
            search_dir.display()
        ));
    }
    if total > RESULT_LIMIT {
        lines.push(format!(
            "... {total} files matched; showing the {shown} most recently modified."
        ));
    }
    Ok(lines.join("\n"))
}

fn compile_matcher(pattern: &str) -> Result<GlobMatcher, String> {
    Glob::new(pattern)
        .map(|g| g.compile_matcher())
        .map_err(|e| format!("invalid glob pattern `{pattern}`: {e}"))
}
