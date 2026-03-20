# termojinal

Terminal of the vibe coding, by the vibe coding, for the vibe coding.

## What is this?

There are already many great terminals out there — cmux, iTerm, and others. Yet none of them felt quite right to me.
As a Japanese speaker, my first requirement is simple: Japanese input that just works, inline, without friction.
I could have contributed to one of those existing, excellent projects. But if I was going to fix this anyway, I thought — why not build the terminal I actually want?
I have a deep appreciation for cmux's design aesthetic, and an equal love for iTerm's usability. Termojinal lives somewhere between the two — with the addition of a command palette, so you can reach anything, from anywhere, with joy.
For now, this is a selfish project — of my vibe, by my vibe, for my vibe. Like bonsai, I intend to tend to it slowly, day by day, pruning and shaping it through daily use.

## Features

- GPU-accelerated rendering with wgpu + Metal
- Workspace, tab, and split pane management with an immutable tree layout
- Vertical sidebar showing workspaces, git branches, listening ports, and AI status
- Command palette with fuzzy search and external plugin support via stdio JSON
- Allow Flow — approve or deny AI agent permission requests inline from the sidebar
- Batch approve/deny across multiple pending requests with a single keystroke
- Quick Terminal — global hotkey summons the terminal instantly from anywhere
- Dark/light theme auto-switching following macOS appearance
- Full ANSI 16-color palette configurable per theme
- SDF rounded rectangle and Gaussian blur shaders for overlay UI
- CJK full-width character support and inline Japanese IME
- Emoji rendering via Core Text with proper Unicode Emoji_Presentation handling
- Scrollback with hot (memory) + warm (mmap) hybrid storage
- Image protocols — Kitty Graphics, Sixel, iTerm2 inline images
- Ed25519 command signing for verified plugins
- MCP server so Claude Code can create workspaces, read terminal content, and manage Allow Flow
- Desktop notifications via Notification Center with app icon
- One-command setup (`tm setup`) configures Claude Code hooks and notification channel
- Homebrew installable with launchd daemon auto-start

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

```bash
tm setup
```

This one command does everything:
- Creates `~/.config/termojinal/` with default config
- Links bundled commands (start-review, run-agent, etc.)
- Configures Claude Code notification channel (OSC 9 via `preferredNotifChannel = iterm2`)
- Installs Claude Code notification hook (`~/.claude/hooks/termojinal-notify.sh`)
- Registers the hook in `~/.claude/settings.json`

### Daemon (optional, for global hotkeys)

Enables `` Ctrl+` `` Quick Terminal even when termojinal is not focused.

```bash
# Manual
make run-daemon

# Auto-start on login
cp dist/launchd/com.termojinal.daemon.plist ~/Library/LaunchAgents/
launchctl load ~/Library/LaunchAgents/com.termojinal.daemon.plist
```

Requires **Accessibility permission**: System Settings > Privacy & Security > Accessibility > add `termojinald`.

### MCP server (optional, for Claude Code workspace control)

Lets Claude Code create workspaces, tabs, read terminal content, and manage Allow Flow.

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
termojinal          # release
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

Install bundled commands: `tm setup` (or manually: `cp -r commands/* ~/.config/termojinal/commands/`)

## Architecture

```
termojinal        GUI application (wgpu + Metal + winit)
termojinald       Session daemon (PTY management + global hotkeys)
termojinal-mcp    MCP server (Claude Code integration)
tm                CLI tool (IPC client + setup)
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
