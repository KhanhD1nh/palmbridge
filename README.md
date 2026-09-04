# Palmbridge

Local coding tools for ChatGPT Web over MCP. ChatGPT provides the model; Palmbridge runs on your machine and works only inside the workspace you select.

```text
ChatGPT Web → OpenAI tunnel → Palmbridge → selected repository
```

Fork of [nghyane/hands](https://github.com/nghyane/hands). Not affiliated with OpenAI or xAI. Tool runtime: [Grok Build](https://github.com/xai-org/grok-build), Apache-2.0.

## Requirements

| Platform | Required |
|---|---|
| macOS | Xcode Command Line Tools, Homebrew, `git`, `python3`, Rust, `tunnel-client` |
| Windows 10/11 | Git, Python 3 launcher (`py`), Rust MSVC toolchain, Visual Studio Build Tools C++, `protoc.exe` on `PATH` |
| Linux | `git`, `python3`, Rust, `tunnel-client`, systemd user services |

The first install clones and compiles Grok Build. It can take several minutes.

## Install

### macOS

```bash
xcode-select --install
brew install git python rustup openai/tools/tunnel-client
rustup default stable

git clone https://github.com/KhanhD1nh/palmbridge.git
cd palmbridge
./install.sh
```

Palmbridge installs `hands` to `~/.local/bin`. For zsh:

```bash
echo 'export PATH="$HOME/.local/bin:$PATH"' >> ~/.zshrc
source ~/.zshrc
hands --version
```

### Windows

Install prerequisites first:

```powershell
winget install Git.Git Python.Python.3.11 Rustlang.Rustup Microsoft.VisualStudio.2022.BuildTools Google.Protobuf
rustup default stable-msvc
```

Visual Studio Build Tools must include the **Desktop development with C++** workload. Restart PowerShell after installation so `git`, `py`, `cargo`, and `protoc` are on `PATH`.

```powershell
git clone https://github.com/KhanhD1nh/palmbridge.git
cd palmbridge
.\install.ps1
```

Palmbridge installs `hands.exe` and `tunnel-client.exe` to `%USERPROFILE%\.local\bin`. Add it to the current PowerShell session, then verify:

```powershell
$env:Path = "$env:USERPROFILE\.local\bin;$env:Path"
hands --version
```

Persist it through Windows Environment Variables if required for new terminals.

### Linux

```bash
git clone https://github.com/KhanhD1nh/palmbridge.git
cd palmbridge
./install.sh
export PATH="$HOME/.local/bin:$PATH"
```

Install `tunnel-client` separately for your distribution before running setup.

## Connect ChatGPT

### 1. Create credentials

Create a restricted OpenAI API key with only **Tunnels: Read** and **Tunnels: Use**:

- https://platform.openai.com/settings/organization/api-keys

Create or copy a `tunnel_...` ID:

- https://platform.openai.com/settings/organization/tunnels

### 2. Select workspace and start

Run from the repository ChatGPT may access:

```bash
cd /path/to/repository
hands setup
```

```powershell
cd C:\path\to\repository
hands setup
```

The interactive setup requests the runtime key and tunnel ID, saves them locally, starts the tunnel, and copies the tunnel ID to the clipboard.

Non-interactive setup:

```bash
export CONTROL_PLANE_API_KEY="sk-..."
export CONTROL_PLANE_TUNNEL_ID="tunnel_..."
hands setup
```

```powershell
$env:CONTROL_PLANE_API_KEY = "sk-..."
$env:CONTROL_PLANE_TUNNEL_ID = "tunnel_..."
hands setup
```

Credential storage: macOS uses Keychain. The tunnel-client profile references a local key file. Never commit either value.

### 3. Add the ChatGPT connection

1. Open https://chatgpt.com/plugins.
2. Enable Developer mode.
3. Create a **Tunnel** connection.
4. Paste the `tunnel_...` ID.
5. Scan tools.

The connection is currently named **Hands** for upstream MCP compatibility.

## Use

Pin a workspace before asking ChatGPT to edit it:

```bash
cd /path/to/repository
hands use
```

```powershell
cd C:\path\to\repository
hands use
```

`workspace_info` reports the active workspace. `set_workspace` can switch it from ChatGPT.

| Command | Purpose |
|---|---|
| `hands setup` | Configure credentials, pin workspace, enable tunnel |
| `hands use` | Pin current directory |
| `hands start` / `hands stop` | Start or stop tunnel |
| `hands enable` / `hands disable` | Enable or disable service |
| `hands status --json` | Check machine-readable health |
| `hands config --open` | Open local UI at `http://127.0.0.1:8787/` |
| `hands list` | List MCP tools |
| `hands call <tool> <json>` | Call a tool locally for debugging |

macOS uses a LaunchAgent. Linux uses systemd user services. Windows starts a detached background process; after reboot or crash, run `hands start` again.

## ChatGPT approvals

ChatGPT, not Palmbridge, decides approvals:

- Read-only tools can run automatically.
- File edits are routine actions.
- Shell commands and task termination may require approval.

For unattended edits, select **Always allow** on the first write, or set **Settings → Apps → Hands → Never ask**. This setting persists across chats.

## Tools

| Tool | Purpose |
|---|---|
| `workspace_info`, `set_workspace` | Inspect or switch workspace |
| `read_file`, `grep`, `list_dir`, `glob` | Read and search files |
| `lsp` | Definitions, references, symbols, hover |
| `browser` | Inspect local Chromium pages, run JavaScript, screenshots |
| `search_replace`, `write`, `apply_patch` | Edit files |
| `todo_write` | Track work |
| `run_terminal_cmd` | Run commands and tests |
| `get_task_output`, `kill_task` | Manage background commands |

LSP detects `.grok/lsp.json`, project `node_modules/.bin`, npm-global language servers, and `rust-analyzer`. Browser inspection supports Chrome, Brave, Edge, and Chromium. `browser start` creates a persistent local profile for authenticated localhost development.

## Troubleshooting

```bash
hands status --json
hands list
hands call read_file '{"target_file":"README.md"}'
```

| Symptom | Fix |
|---|---|
| `hands: command not found` | Add `~/.local/bin` to `PATH`. On Windows add `%USERPROFILE%\.local\bin`. |
| `tunnel-client not found` | macOS: `brew install openai/tools/tunnel-client`; Windows: rerun `install.ps1`. |
| Windows build fails before compiling Hands | Confirm VS C++ Build Tools and `protoc.exe` are installed and on `PATH`. |
| Tunnel not running after Windows reboot | Run `hands start`. |
| Tunnel appears offline | Run `hands status --json`; then `hands stop` and `hands start`. |

## License

Apache-2.0. See `NOTICE`.
