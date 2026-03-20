# termojinal

Terminal of the vibe coding, by the vibe coding, for the vibe coding.

## What is this?

There are already many great terminals out there — cmux, iTerm, and others. Yet none of them felt quite right to me.
As a Japanese speaker, my first requirement is simple: Japanese input that just works, inline, without friction.
I could have contributed to one of those existing, excellent projects. But if I was going to fix this anyway, I thought — why not build the terminal I actually want?
I have a deep appreciation for cmux's design aesthetic, and an equal love for iTerm's usability. Termojinal lives somewhere between the two — with the addition of a command palette, so you can reach anything, from anywhere, with joy.
For now, this is a selfish project — of my vibe, by my vibe, for my vibe. Like bonsai, I intend to tend to it slowly, day by day, pruning and shaping it through daily use.

## Installation

### Prerequisites

- macOS 13 Ventura or later (Apple Silicon + Intel)
- Rust 1.78+

### Build from source

```bash
git clone https://github.com/KikuchiTomo/termojinal.git
cd termojinal
make install   # install Rust toolchain + fetch dependencies
make           # release build
```

### Homebrew (coming soon)

```bash
brew tap KikuchiTomo/termojinal
brew install termojinal
brew services start termojinal  # start daemon for global hotkeys
```

### One-liner install

```bash
curl -fsSL https://raw.githubusercontent.com/KikuchiTomo/termojinal/main/dist/install.sh | bash
```

## Setup

### 1. Configuration

```bash
mkdir -p ~/.config/termojinal
cp resources/config.example.toml ~/.config/termojinal/config.toml
```

### 2. Claude Code integration (required for AI features)

termojinal supports Claude Code's permission notifications via OSC 9 (iTerm2 protocol). Run:

```bash
claude config set --global preferredNotifChannel iterm2
```

This tells Claude Code to send terminal notifications using the OSC 9 protocol, which termojinal natively supports. Without this setting, Claude Code's Allow Flow notifications will not appear.

### 3. Claude Code Hooks (recommended)

For reliable desktop notifications from Claude Code (task complete, permission needed, etc.), install the notification hook:

```bash
mkdir -p ~/.claude/hooks
cp hooks/claude-code-notify.sh ~/.claude/hooks/
chmod +x ~/.claude/hooks/claude-code-notify.sh
```

Add the following to your `~/.claude/settings.json`:

```json
{
  "hooks": {
    "Notification": [
      {
        "type": "command",
        "command": "~/.claude/hooks/claude-code-notify.sh"
      }
    ]
  }
}
```

This uses the `tm notify` CLI to forward Claude Code events as macOS desktop notifications and sidebar unread indicators.

### 4. Start the daemon (optional, for global hotkeys)

The daemon enables global hotkeys like `Ctrl+`` for Quick Terminal, even when termojinal is not focused.

```bash
# Manual start
make run-daemon

# Auto-start on login (launchd)
cp dist/launchd/com.termojinal.daemon.plist ~/Library/LaunchAgents/
launchctl load ~/Library/LaunchAgents/com.termojinal.daemon.plist
```

Note: The daemon requires **Accessibility permission** for global hotkeys.
Go to System Settings > Privacy & Security > Accessibility and add `termojinald`.

### 5. MCP server (optional, for Claude Code workspace control)

termojinal includes an MCP server that lets Claude Code create workspaces, tabs, read terminal content, and manage Allow Flow requests.

Add to your Claude Code MCP settings:

```json
{
  "mcpServers": {
    "termojinal": {
      "command": "termojinal-mcp"
    }
  }
}
```

## Usage

```bash
make run-dev        # development mode
make run-dev-debug  # with debug logging
```

### Key bindings

| Action | Default |
|---|---|
| Command Palette | `Cmd+Shift+P` |
| Quick Terminal | `Ctrl+`` |
| Split Right | `Cmd+D` |
| Split Down | `Cmd+Shift+D` |
| New Tab | `Cmd+T` |
| New Workspace | `Cmd+N` |
| Close | `Cmd+W` |
| Search | `Cmd+F` |
| Allow Flow Panel | `Cmd+Shift+A` |

All keybindings are configurable in `~/.config/termojinal/keybindings.toml`.

### Allow Flow (AI permission management)

When Claude Code requests permission (e.g., to run a bash command), termojinal shows the request inline in the sidebar:

| Key | Action |
|---|---|
| `y` | Allow one request |
| `n` | Deny one request |
| `Y` (Shift) | Allow ALL requests |
| `N` (Shift) | Deny ALL requests |
| `a` | Allow + remember rule |

### Custom commands

Place command scripts in `~/.config/termojinal/commands/`:

```
~/.config/termojinal/commands/my-command/
├── command.toml
└── run.sh
```

Bundled commands: `start-review`, `switch-worktree`, `kill-merged`, `clone-and-open`, `run-agent`, `hello-world`.

Install bundled commands:
```bash
cp -r commands/* ~/.config/termojinal/commands/
```

## Architecture

```
termojinal-dev    GUI application (wgpu + Metal + winit)
termojinald       Session daemon (PTY management + global hotkeys)
termojinal-mcp    MCP server (Claude Code integration)
tm                CLI tool (IPC client)
```

### Crate structure

| Crate | Purpose |
|---|---|
| `termojinal-pty` | PTY fork/exec |
| `termojinal-vt` | VT parser + cell grid + scrollback |
| `termojinal-render` | wgpu GPU renderer + font atlas + SDF shaders |
| `termojinal-layout` | Immutable split pane tree |
| `termojinal-session` | Session daemon + global hotkeys |
| `termojinal-ipc` | IPC protocol + keybindings + CLI |
| `termojinal-claude` | Allow Flow engine |
| `termojinal-mcp` | MCP server |

## License

MIT
