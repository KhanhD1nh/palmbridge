# Hands

Unofficial **ChatGPT plugin**: local coding tools over MCP. No local LLM. ChatGPT is the brain; this machine is the hands.

Not affiliated with OpenAI or xAI. Tool runtime is [Grok Build](https://github.com/xai-org/grok-build) (Apache-2.0).

```text
ChatGPT Web  →  Secure MCP Tunnel  →  hands  →  your repo
```

## Install

Needs `git`, `python3`, `rustup`. macOS or Linux. First build compiles grok-build (several minutes).

```bash
git clone https://github.com/nghyane/hands.git
cd hands
./install.sh
```

Agents: see `AGENTS.md`. One-shot if keys are already in the environment:

```bash
export CONTROL_PLANE_API_KEY="sk-..."
export CONTROL_PLANE_TUNNEL_ID="tunnel_..."
./install.sh
```

## Zero config

```bash
brew install openai/tools/tunnel-client   # once
cd /path/to/your/repo
hands setup                               # TTY checklist, Keychain, no browser
```

Runtime key goes in the macOS Keychain (file `0600` only for the daemon). Tunnel id is copied to the clipboard. Notification if the tunnel drops.

Config page (optional): `hands config --open` → http://127.0.0.1:8787/  
Scripts: `hands status --json`.

## ChatGPT Web

1. Runtime key (Restricted, Tunnels **Read** + **Use**):  
   https://platform.openai.com/settings/organization/api-keys
2. Tunnel id:  
   https://platform.openai.com/settings/organization/tunnels
3. [chatgpt.com/plugins](https://chatgpt.com/plugins) → Developer mode → Connection **Tunnel** → paste tunnel id → Scan tools.

Plugin name in ChatGPT: **Hands**.

ChatGPT, not Hands, shows Confirm. MCP cannot turn that off.

- Reads auto-run (`readOnlyHint`).
- File edits are routine (`destructiveHint: false`) — auto under **Important actions**.
- Shell / kill still confirm unless you opt in.

**Unattended coding:** first write prompt → **Always allow**, or **Settings → Apps → Hands → Never ask**. New chats keep that app setting. Developer Mode “remember for this conversation” dies on a new chat.

## Tools

| Tool | Role |
|---|---|
| `workspace_info` | current pin + recent |
| `set_workspace` | pin another folder (works from ChatGPT, including away) |
| `read_file` | read |
| `grep` | search contents |
| `list_dir` | tree |
| `glob` | find files by name |
| `lsp` | code intelligence via language servers (definitions, references, symbols, hover) |
| `browser` | inspect localhost Chromium DOM/computed styles, run JS, capture screenshots |
| `search_replace` | edit existing |
| `write` | create / overwrite |
| `apply_patch` | multi-hunk patch |
| `todo_write` | task list |
| `run_terminal_cmd` | tests / git / shell; long FG auto-backgrounds |
| `get_task_output` | poll background job |
| `kill_task` | stop background job |

Debug: `hands list`, `hands call read_file '{"target_file":"README.md"}'`.

`glob` is implemented natively by Hands on Windows, so it does not require `rg` to be on PATH.
LSP auto-detects project/user `.grok/lsp.json`, local `node_modules/.bin`, npm global language servers,
and `rustup which rust-analyzer`. Browser inspection supports Chrome, Brave, Edge, or Chromium and is
intended for local development/debugging; `browser start` creates a persistent Hands browser profile
that can be signed into once and reused for authenticated localhost apps.

On AC the Mac stays awake for the long-poll; on battery, closing the lid may sleep.

## License

Apache-2.0. See `NOTICE`.
