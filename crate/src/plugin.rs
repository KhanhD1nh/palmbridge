//! ChatGPT plugin chrome: titles, annotations, invocation text, skills snapshot.

use serde_json::{Value, json};
use sha2::{Digest, Sha256};

const SKILL_MD: &str = include_str!("../skills/palmbridge-code/SKILL.md");
const SKILL_URI: &str = "skill://palmbridge/palmbridge-code/SKILL.md";

pub struct Face {
    pub title: &'static str,
    pub invoking: &'static str,
    pub invoked: &'static str,
    pub read_only: bool,
    pub destructive: bool,
    pub idempotent: bool,
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
        },
        "set_workspace" => Face {
            title: "Switch workspace",
            invoking: "Switching workspace…",
            invoked: "Workspace switched",
            read_only: false,
            destructive: false,
            idempotent: true,
        },
        "read_file" => Face {
            title: "Read file",
            invoking: "Reading file…",
            invoked: "Read",
            read_only: true,
            destructive: false,
            idempotent: true,
        },
        "grep" => Face {
            title: "Search files",
            invoking: "Searching files…",
            invoked: "Search done",
            read_only: true,
            destructive: false,
            idempotent: true,
        },
        "list_dir" => Face {
            title: "List folder",
            invoking: "Listing folder…",
            invoked: "Listed",
            read_only: true,
            destructive: false,
            idempotent: true,
        },
        "glob" => Face {
            title: "Find files",
            invoking: "Finding files…",
            invoked: "Found files",
            read_only: true,
            destructive: false,
            idempotent: true,
        },
        "get_task_output" => Face {
            title: "Command output",
            invoking: "Reading output…",
            invoked: "Got output",
            read_only: true,
            destructive: false,
            idempotent: true,
        },
        "search_replace" => Face {
            title: "Edit file",
            invoking: "Editing file…",
            invoked: "Edited",
            read_only: false,
            destructive: false,
            idempotent: false,
        },
        "todo_write" => Face {
            title: "Update todos",
            invoking: "Updating todos…",
            invoked: "Todos updated",
            read_only: false,
            destructive: false,
            idempotent: true,
        },
        "write" => Face {
            title: "Write file",
            invoking: "Writing file…",
            invoked: "Wrote",
            read_only: false,
            destructive: false,
            idempotent: true,
        },
        "apply_patch" => Face {
            title: "Apply patch",
            invoking: "Applying patch…",
            invoked: "Patched",
            read_only: false,
            destructive: false,
            idempotent: false,
        },
        "run_terminal_cmd" => Face {
            title: "Run command",
            invoking: "Running command…",
            invoked: "Command finished",
            read_only: false,
            destructive: true,
            idempotent: false,
        },
        "kill_task" => Face {
            title: "Stop command",
            invoking: "Stopping command…",
            invoked: "Stopped",
            read_only: false,
            destructive: true,
            idempotent: true,
        },
        _ => Face {
            title: "Palmbridge tool",
            invoking: "Working…",
            invoked: "Done",
            read_only: false,
            destructive: false,
            idempotent: false,
        },
    }
}

pub fn tool_descriptor(name: &str, description: &str, input_schema: Value) -> Value {
    let f = face(name);
    json!({
        "name": name,
        "title": f.title,
        "description": description,
        "inputSchema": input_schema,
        "annotations": {
            "title": f.title,
            "readOnlyHint": f.read_only,
            "destructiveHint": f.destructive,
            "openWorldHint": false,
            "idempotentHint": f.idempotent,
        },
        "_meta": {
            "openai/toolInvocation/invoking": f.invoking,
            "openai/toolInvocation/invoked": f.invoked,
        }
    })
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
        "Palmbridge: local coding tools, no model. Workspace: {workspace}. \
         Use skill palmbridge-code. Call workspace_info first; set_workspace to switch \
         (absolute, ~/…, or name under ~/Dev). Reads auto-run. File edits are routine. \
         Shell/kill may confirm unless ChatGPT Apps → Palmbridge → Never ask (or Always allow). \
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
            "name": "palmbridge-code",
            "description": "Read, edit, and run code on the user's local machine via Palmbridge MCP tools. Use when the user wants to work in a repo, fix a bug, run tests, or switch workspaces on this computer."
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
            "name": "palmbridge-code",
            "mimeType": "text/markdown",
            "description": "Palmbridge local coding workflow"
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
