//! ChatGPT plugin chrome: titles, annotations, invocation text, skills snapshot.

use serde_json::{Value, json};
use sha2::{Digest, Sha256};

const SKILL_MD: &str = include_str!("../skills/graft-code/SKILL.md");
const SKILL_URI: &str = "skill://graft/graft-code/SKILL.md";

pub struct Face {
    pub title: &'static str,
    pub invoking: &'static str,
    pub invoked: &'static str,
    pub read_only: bool,
    pub destructive: bool,
    pub idempotent: bool,
    pub open_world: bool,
}

/// Host confirmation (ChatGPT):
/// - `read_only` → auto-run
/// - write + not `destructive` → auto under **Important actions**
/// - `destructive` → confirm unless the app is **Never ask**
pub fn face(name: &str) -> Face {
    match name {
        "workspace_info" => Face {
            title: "Current workspace",
            invoking: "Checking workspace…",
            invoked: "Workspace ready",
            read_only: true,
            destructive: false,
            idempotent: true,
            open_world: false,
        },
        "set_workspace" => Face {
            title: "Switch workspace",
            invoking: "Switching workspace…",
            invoked: "Workspace switched",
            read_only: false,
            destructive: false,
            idempotent: true,
            open_world: false,
        },
        "read_file" => Face {
            title: "Read file",
            invoking: "Reading file…",
            invoked: "Read",
            read_only: true,
            destructive: false,
            idempotent: true,
            open_world: false,
        },
        "batch_read" => Face {
            title: "Read files",
            invoking: "Reading files…",
            invoked: "Files read",
            read_only: true,
            destructive: false,
            idempotent: true,
            open_world: false,
        },
        "grep" => Face {
            title: "Search files",
            invoking: "Searching files…",
            invoked: "Search done",
            read_only: true,
            destructive: false,
            idempotent: true,
            open_world: false,
        },
        "list_dir" => Face {
            title: "List folder",
            invoking: "Listing folder…",
            invoked: "Listed",
            read_only: true,
            destructive: false,
            idempotent: true,
            open_world: false,
        },
        "glob" => Face {
            title: "Find files",
            invoking: "Finding files…",
            invoked: "Found files",
            read_only: true,
            destructive: false,
            idempotent: true,
            open_world: false,
        },
        "get_task_output" => Face {
            title: "Command output",
            invoking: "Reading output…",
            invoked: "Got output",
            read_only: true,
            destructive: false,
            idempotent: true,
            open_world: false,
        },
        "lsp" => Face {
            title: "Code intelligence",
            invoking: "Querying code intelligence…",
            invoked: "Code intelligence ready",
            read_only: true,
            destructive: false,
            idempotent: true,
            open_world: false,
        },
        "git_status" => Face {
            title: "Git status",
            invoking: "Checking Git status…",
            invoked: "Git status ready",
            read_only: true,
            destructive: false,
            idempotent: true,
            open_world: false,
        },
        "git_diff" => Face {
            title: "Git diff",
            invoking: "Reading Git diff…",
            invoked: "Git diff ready",
            read_only: true,
            destructive: false,
            idempotent: true,
            open_world: false,
        },
        "search_replace" => Face {
            title: "Edit file",
            invoking: "Editing file…",
            invoked: "Edited",
            read_only: false,
            destructive: false,
            idempotent: false,
            open_world: false,
        },
        "todo_write" => Face {
            title: "Update todos",
            invoking: "Updating todos…",
            invoked: "Todos updated",
            read_only: false,
            destructive: false,
            idempotent: true,
            open_world: false,
        },
        "write" => Face {
            title: "Write file",
            invoking: "Writing file…",
            invoked: "Wrote",
            read_only: false,
            destructive: false,
            idempotent: true,
            open_world: false,
        },
        "apply_patch" => Face {
            title: "Apply patch",
            invoking: "Applying patch…",
            invoked: "Patched",
            read_only: false,
            destructive: false,
            idempotent: false,
            open_world: false,
        },
        "run_terminal_cmd" => Face {
            title: "Run command",
            invoking: "Running command…",
            invoked: "Command finished",
            read_only: false,
            destructive: true,
            idempotent: false,
            open_world: false,
        },
        "kill_task" => Face {
            title: "Stop command",
            invoking: "Stopping command…",
            invoked: "Stopped",
            read_only: false,
            destructive: true,
            idempotent: true,
            open_world: false,
        },
        "browser" => Face {
            title: "Browser debug",
            invoking: "Inspecting browser…",
            invoked: "Browser result ready",
            read_only: false,
            destructive: false,
            idempotent: false,
            open_world: true,
        },
        _ => Face {
            title: "Graft tool",
            invoking: "Working…",
            invoked: "Done",
            read_only: false,
            destructive: false,
            idempotent: false,
            open_world: false,
        },
    }
}

pub fn tool_descriptor(name: &str, description: &str, input_schema: Value) -> Value {
    let f = face(name);
    let input_schema = compact_schema(name, input_schema);
    json!({
        "name": name,
        "title": f.title,
        "description": description,
        "inputSchema": input_schema,
        "annotations": {
            "title": f.title,
            "readOnlyHint": f.read_only,
            "destructiveHint": f.destructive,
            "openWorldHint": f.open_world,
            "idempotentHint": f.idempotent,
        },
        "_meta": {
            "openai/toolInvocation/invoking": f.invoking,
            "openai/toolInvocation/invoked": f.invoked,
        }
    })
}

/// Keep tool selection cheap. Workflow detail belongs in the Graft skill; the
/// descriptor only needs enough information for the model to choose the tool.
pub fn compact_description(name: &str, fallback: &str) -> String {
    let description = match name {
        "read_file" => "Read one file. Supports line windows plus image/PDF rendering.",
        "grep" => "Regex-search file contents with optional path, type, glob, and context filters.",
        "list_dir" => "List a directory, respecting gitignore and summarizing very large folders.",
        "glob" => {
            "Find files by glob. Fast mode stops after enough matches; recent mode scans for newest files."
        }
        "search_replace" => {
            "Replace one exact string (or all matches) in a file. Read the file first."
        }
        "write" => "Create or overwrite a file. Read existing files before overwriting them.",
        "apply_patch" => "Apply a multi-hunk/multi-file patch in the Graft patch format.",
        "todo_write" => "Create or update the visible task list for multi-step work.",
        "lsp" => {
            "Query language-server definitions, references, implementations, symbols, or hover info."
        }
        "run_terminal_cmd" => {
            "Run a shell command. Long commands can run in the background and return a task id."
        }
        "get_task_output" => "Read status/output for one or more background command task ids.",
        "kill_task" => "Terminate a background command by task id.",
        _ => return truncate_text(fallback, 360),
    };
    description.into()
}

fn compact_schema(name: &str, mut value: Value) -> Value {
    if name == "glob"
        && let Some(properties) = value.get_mut("properties").and_then(Value::as_object_mut)
    {
        properties.insert(
            "mode".into(),
            json!({
                "type": "string",
                "enum": ["fast", "recent"],
                "default": "fast",
                "description": "fast stops after enough matches; recent scans all matches and sorts newest-first."
            }),
        );
    }
    compact_schema_value(&mut value);
    value
}

fn compact_schema_value(value: &mut Value) {
    match value {
        Value::Object(map) => {
            map.remove("$schema");
            if let Some(Value::String(description)) = map.get_mut("description")
                && description.chars().count() > 240
            {
                *description = truncate_text(description, 240);
            }
            for child in map.values_mut() {
                compact_schema_value(child);
            }
        }
        Value::Array(items) => {
            for child in items {
                compact_schema_value(child);
            }
        }
        _ => {}
    }
}

fn truncate_text(text: &str, max_chars: usize) -> String {
    let mut chars = text.chars();
    let prefix: String = chars.by_ref().take(max_chars).collect();
    if chars.next().is_some() {
        format!("{}…", prefix.trim_end())
    } else {
        prefix
    }
}

pub fn initialize_capabilities() -> Value {
    json!({
        "tools": { "listChanged": false },
        "resources": { "listChanged": false },
        "extensions": {
            "io.modelcontextprotocol/skills": {}
        }
    })
}

pub fn initialize_instructions(workspace: &str) -> String {
    format!(
        "Graft: local coding tools, no model. Workspace: {workspace}. \
         Use skill graft-code. Call workspace_info when the workspace is unknown or may have changed; set_workspace switches this \
         stateful session (absolute, ~/…, or name under ~/Dev); use persist=true for stateless/reconnecting clients. `graft use` changes \
         the persisted default. Prefer batch_read for several known files and git_status/git_diff for read-only Git inspection. Reads auto-run. File edits are routine. \
         Shell/kill may confirm unless ChatGPT Apps → Graft → Never ask (or Always allow). \
         After edits, rerun the failing check. Long commands: background + get_task_output."
    )
}

fn skill_digest() -> String {
    format!("sha256:{:x}", Sha256::digest(SKILL_MD.as_bytes()))
}

fn skill_entry() -> Value {
    json!({
        "uri": SKILL_URI,
        "frontmatter": {
            "name": "graft-code",
            "description": "Read, edit, and run code on the user's local machine via Graft MCP tools. Use when the user wants to work in a repo, fix a bug, run tests, or switch workspaces on this computer."
        },
        "resources": [{
            "uri": SKILL_URI,
            "digest": skill_digest()
        }]
    })
}

pub fn skills_list() -> Value {
    json!({ "skills": [skill_entry()] })
}

pub fn skills_get(params: &Value) -> Result<Value, (i64, String, Value)> {
    let uri = params.get("uri").and_then(Value::as_str).unwrap_or("");
    if uri != SKILL_URI {
        return Err((-32602, format!("unknown skill uri: {uri}"), Value::Null));
    }
    Ok(json!({ "skill": skill_entry() }))
}

pub fn resources_list() -> Value {
    json!({
        "resources": [{
            "uri": SKILL_URI,
            "name": "graft-code",
            "mimeType": "text/markdown",
            "description": "Graft local coding workflow"
        }]
    })
}

pub fn resources_read(params: &Value) -> Result<Value, (i64, String, Value)> {
    let uri = params.get("uri").and_then(Value::as_str).unwrap_or("");
    if uri != SKILL_URI {
        return Err((-32602, format!("unknown resource uri: {uri}"), Value::Null));
    }
    Ok(json!({
        "contents": [{
            "uri": SKILL_URI,
            "mimeType": "text/markdown",
            "text": SKILL_MD
        }]
    }))
}
