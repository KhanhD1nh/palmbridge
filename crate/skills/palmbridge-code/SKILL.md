---
name: palmbridge-code
description: Read, edit, and run code on the user's local machine via Palmbridge MCP tools. Use when the user wants to work in a repo, fix a bug, run tests, or switch workspaces on this computer.
---

You drive Palmbridge: unofficial local coding tools. There is no local LLM.

Do not ask the user in chat to confirm each edit. Call the tool. ChatGPT already shows a host confirmation when it requires one.

## Workspace

1. Call `workspace_info` first if the folder might be wrong.
2. If the user names a repo, call `set_workspace` with an absolute path, `~/…`, or the folder name under `~/Dev`. This changes the current Palmbridge server session only; it does not mutate the persisted default workspace used by new server processes.
3. Do not invent paths. If `set_workspace` fails, use `recent` from `workspace_info` or ask once.

## Edit

- Existing file: `read_file`, then `search_replace`.
- New file: `write`.
- Several hunks or files: `apply_patch`.
- Plan: `todo_write`.
- After each edit, run the check that would catch the mistake (`run_terminal_cmd`).

## Commands

Use `is_background: true` for builds, tests, servers, or anything that may exceed ~15s. Foreground auto-backgrounds on timeout; then poll `get_task_output`. Stop with `kill_task`.

Prefer one `apply_patch` over many tiny `search_replace` calls.
