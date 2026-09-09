use std::cmp::Reverse;
use std::collections::BinaryHeap;
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

    let workspace = dunce::canonicalize(cwd)
        .map_err(|e| format!("canonicalize workspace {}: {e}", cwd.display()))?;
    let requested = arguments
        .get("path")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty());
    let search_dir = match requested {
        Some(path) => {
            let path = PathBuf::from(path);
            let path = if path.is_absolute() { path } else { workspace.join(path) };
            if !path.is_dir() {
                return Err(format!("glob path is not a directory: {}", path.display()));
            }
            let path = dunce::canonicalize(&path)
                .map_err(|e| format!("canonicalize glob path {}: {e}", path.display()))?;
            if !path.starts_with(&workspace) {
                return Err(format!("glob path escapes workspace: {}", path.display()));
            }
            path
        }
        None => workspace,
    };

    let matcher = compile_matcher(pattern)?;
    let mut newest: BinaryHeap<Reverse<(SystemTime, PathBuf)>> = BinaryHeap::new();
    let mut total = 0usize;

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
        total += 1;
        newest.push(Reverse((modified, path)));
        if newest.len() > RESULT_LIMIT {
            newest.pop();
        }
    }

    let mut matches = newest
        .into_iter()
        .map(|Reverse(item)| item)
        .collect::<Vec<_>>();
    matches.sort_by(|a, b| b.0.cmp(&a.0));
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

#[cfg(test)]
mod tests {
    use super::run;
    use serde_json::json;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_dir(name: &str) -> std::path::PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "palmbridge-{name}-{}-{nonce}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn caps_large_results_without_storing_all_matches() {
        let dir = temp_dir("glob-limit");
        for i in 0..105 {
            std::fs::write(dir.join(format!("file-{i:03}.txt")), b"x").unwrap();
        }
        let output = run(&json!({"pattern":"*.txt"}), &dir).unwrap();
        assert!(output.contains("105 files matched"));
        assert!(output.lines().count() <= 101);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn rejects_search_directories_outside_workspace() {
        let workspace = temp_dir("glob-workspace");
        let outside = temp_dir("glob-outside");
        let error = run(
            &json!({"pattern":"*", "path": outside.display().to_string()}),
            &workspace,
        )
        .unwrap_err();
        assert!(error.contains("escapes workspace"));
        let _ = std::fs::remove_dir_all(workspace);
        let _ = std::fs::remove_dir_all(outside);
    }
}
