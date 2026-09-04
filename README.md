# Palmbridge

Use ChatGPT Web as the model and a Mac as the local coding-tool runtime. Palmbridge exposes your selected workspace through MCP; it does not run a local model.

```text
ChatGPT Web → OpenAI tunnel → Palmbridge on your Mac → selected repository
```

Fork of [nghyane/hands](https://github.com/nghyane/hands). Not affiliated with OpenAI or xAI. The tool runtime is [Grok Build](https://github.com/xai-org/grok-build) (Apache-2.0).

## macOS quick start

### 1. Install prerequisites

```bash
xcode-select --install
brew install git python rustup openai/tools/tunnel-client
rustup default stable
```

`install.sh` builds Palmbridge from source. First build downloads and compiles Grok Build; expect several minutes.

### 2. Install Palmbridge

```bash
git clone https://github.com/KhanhD1nh/palmbridge.git
cd palmbridge
./install.sh
```

The executable is installed at `~/.local/bin/hands`. Add it to your shell path if needed:

```bash
echo 'export PATH="$HOME/.local/bin:$PATH"' >> ~/.zshrc
source ~/.zshrc
hands --version
```

### 3. Create tunnel credentials

Create a **restricted** OpenAI API key with only **Tunnels: Read** and **Tunnels: Use**:

- https://platform.openai.com/settings/organization/api-keys

Create or copy a tunnel ID:

- https://platform.openai.com/settings/organization/tunnels

### 4. Select a repository and start the tunnel

```bash
cd /path/to/your/repository
hands setup
```

The interactive checklist asks for the runtime key and `tunnel_...` ID. The key is stored in macOS Keychain; Palmbridge starts the tunnel and copies its ID to the clipboard.

Non-interactive setup:

```bash
export CONTROL_PLANE_API_KEY="sk-..."
export CONTROL_PLANE_TUNNEL_ID="tunnel_..."
cd /path/to/your/repository
hands setup
```

On macOS, Palmbridge installs a LaunchAgent so the tunnel starts automatically. Check it with:

```bash
hands status --json
```

### 5. Connect ChatGPT Web

1. Open https://chatgpt.com/plugins.
2. Enable Developer mode.
3. Create a **Tunnel** connection.
4. Paste the copied `tunnel_...` ID.
5. Scan tools.

The connection appears as **Hands** because the executable and MCP server retain the upstream-compatible name.

## Daily use

Pin a workspace before asking ChatGPT to work in it:

```bash
cd /path/to/repository
hands use
```

Then use the ChatGPT connection normally. `workspace_info` shows the pinned directory; `set_workspace` can switch it from ChatGPT.

```bash
hands start            # start tunnel
hands stop             # stop tunnel
hands enable           # install/start automatic service
hands disable          # stop/remove automatic service
hands config --open    # local configuration UI at http://127.0.0.1:8787/
hands status --json    # machine-readable status
```

Mac stays awake while connected to AC. Closing the lid on battery can suspend the tunnel.

## ChatGPT approvals

ChatGPT controls approvals, not Palmbridge:

- Read-only tools can auto-run.
- Edits are normal actions.
- Shell commands and task termination require approval unless allowed in ChatGPT settings.

For unattended edits, approve the first write with **Always allow**, or use **Settings → Apps → Hands → Never ask**. That setting persists across chats.

## Tools

| Tool | Purpose |
|---|---|
| `workspace_info` | Active workspace and recent folders |
| `set_workspace` | Pin another workspace |
| `read_file`, `grep`, `list_dir`, `glob` | Read and search files |
| `lsp` | Definitions, references, symbols, hover |
| `browser` | Inspect local Chromium pages, run JS, screenshots |
| `search_replace`, `write`, `apply_patch` | Edit files |
| `todo_write` | Track work |
| `run_terminal_cmd` | Run commands and tests |
| `get_task_output`, `kill_task` | Manage background commands |

LSP detects `.grok/lsp.json`, project `node_modules/.bin`, npm-global servers, and `rust-analyzer`. `browser` supports Chrome, Brave, Edge, and Chromium; `browser start` creates a persistent local profile for authenticated localhost development.

## Debugging

```bash
hands list
hands call read_file '{"target_file":"README.md"}'
hands status --json
```

If `hands` is not found after installation, run `export PATH="$HOME/.local/bin:$PATH"` or add it to `~/.zshrc`.

## License

Apache-2.0. See `NOTICE`.
