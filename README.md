# Termojinal

[![Homebrew](https://img.shields.io/badge/homebrew-KikuchiTomo%2Ftap-FBB040?logo=homebrew&logoColor=white)](https://github.com/KikuchiTomo/homebrew-termojinal)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)
[![macOS](https://img.shields.io/badge/macOS-13%2B-black?logo=apple&logoColor=white)]()

A GPU-accelerated terminal emulator for macOS, built for developers who work with AI coding agents.

## What is this?

There are already many great terminals out there — cmux, iTerm, and others. Yet none of them felt quite right to me.

As a Japanese speaker, my first requirement is simple: Japanese input that just works, inline, without friction. I could have contributed to one of those existing, excellent projects. But if I was going to fix this anyway, I thought — why not build the terminal I actually want?

I have a deep appreciation for cmux's design aesthetic, and an equal love for iTerm's usability. Termojinal lives somewhere between the two — with the addition of a command palette, so you can reach anything, from anywhere, with joy.

For now, this is a selfish project — of my vibe, by my vibe, for my vibe. Like bonsai, I intend to tend to it slowly, day by day, pruning and shaping it through daily use.

## Features

- GPU-accelerated rendering (wgpu + Metal)
- Workspaces, tabs, and split panes with immutable tree layout
- Vertical sidebar with workspaces, git branches, ports, and AI status
- Command palette with fuzzy search and extensible plugins via JSON stdio
- Allow Flow — approve or deny AI agent permission requests from anywhere with a single key
- Quick Terminal — global hotkey drops a terminal from the top of the screen
- Dark/light theme auto-switching following macOS appearance
- CJK full-width characters and inline Japanese IME
- Color emoji rendering via Core Text
- Inline images (Kitty Graphics, Sixel, iTerm2)
- MCP server for Claude Code workspace control
- Desktop notifications via Notification Center
- Ed25519 command signing for verified plugins
- One-command setup for Claude Code integration (`tm setup`)

## Install

### Homebrew

```bash
brew tap KikuchiTomo/termojinal
brew install termojinal              # CLI tools + daemon
brew install --cask termojinal-app   # GUI app (Termojinal.app → /Applications)
brew services start termojinal       # start daemon (Ctrl+` hotkey)
```

### From source

```bash
git clone https://github.com/KikuchiTomo/termojinal.git
cd termojinal
make install && make app
open target/release/Termojinal.app
```

## Setup

```bash
tm setup
```

Creates config directory, installs Claude Code hooks (Notification + PermissionRequest), and links bundled commands.

### Daemon

The daemon enables global hotkeys (Ctrl+\` Quick Terminal) even when Termojinal is not focused.

```bash
brew services start termojinal     # Homebrew
# or
launchctl load ~/Library/LaunchAgents/com.termojinal.daemon.plist
```

Requires **Accessibility** permission in System Settings.

### MCP server

Gives Claude Code full workspace control — create tabs, read terminal content, approve permissions.

Add to your Claude Code MCP config:

```json
{
  "mcpServers": {
    "termojinal": {
      "command": "termojinal-mcp"
    }
  }
}
```

## Key bindings

| Action | Default |
|--------|---------|
| Command Palette | Cmd+Shift+P |
| Quick Terminal | Ctrl+\` |
| Split right | Cmd+D |
| Split down | Cmd+Shift+D |
| Next / prev pane | Cmd+] / Cmd+[ |
| Zoom pane | Cmd+Shift+Enter |
| New tab | Cmd+T |
| Close tab | Cmd+W |
| Next / prev tab | Cmd+Shift+} / { |
| New workspace | Cmd+N |
| Switch workspace | Cmd+1 through Cmd+9 |
| Toggle sidebar | Cmd+B |
| Search | Cmd+F |
| Font size | Cmd+= / Cmd+- |

All keybindings are customizable. See [configuration docs](docs/configuration.md).

## Allow Flow

When Claude Code needs permission, Termojinal intercepts the request via a `PermissionRequest` hook and shows it in the sidebar. You can approve or deny from anywhere — no need to switch focus.

| Key | Action |
|-----|--------|
| y | Allow one |
| n | Deny one |
| Y | Allow all |
| N | Deny all |
| a | Allow and remember |
| Esc | Dismiss |

Decisions are sent back to Claude Code via structured IPC — no terminal output parsing.

## Custom commands

Scripts that communicate with Termojinal via JSON over stdio. They appear in the command palette.

```
~/.config/termojinal/commands/my-command/
├── command.toml
└── run.sh
```

Bundled: `start-review`, `switch-worktree`, `kill-merged`, `clone-and-open`, `run-agent`, `hello-world`

See [command docs](docs/command.md) for the full protocol reference.

## Configuration

`~/.config/termojinal/config.toml`

Customize fonts, colors, sidebar, tab bar, status bar, pane separators, notifications, quick terminal, and Allow Flow.

See [configuration docs](docs/configuration.md) for the complete reference.

## Development

```bash
make run-dev        # build + start daemon + open .app (debug)
make run-dev-debug  # same with RUST_LOG=debug
make test           # run all tests
make lint           # clippy
make fmt            # format
```

`make run-dev` mirrors the release setup: builds all binaries, creates the `.app` bundle, starts `termojinald` in the background, and opens the app. When the app closes, the daemon is stopped automatically.

## Architecture

| Binary | Purpose |
|--------|---------|
| Termojinal.app | GUI application (wgpu + Metal + winit) |
| termojinald | Session daemon (PTY management, global hotkeys) |
| tm | CLI tool (setup, IPC client, Allow Flow) |
| termojinal-mcp | MCP server for Claude Code |
| termojinal-sign | Ed25519 command signer |

### Crates

| Crate | Purpose |
|-------|---------|
| termojinal-pty | PTY fork/exec |
| termojinal-vt | VT parser, cell grid, scrollback, images |
| termojinal-render | wgpu renderer, font/emoji atlas, SDF shaders |
| termojinal-layout | Immutable split pane tree |
| termojinal-session | Daemon, hotkeys, persistence |
| termojinal-ipc | IPC protocol, keybindings, CLI, command system |
| termojinal-claude | Allow Flow engine |
| termojinal-mcp | MCP server |

## Documentation

- [Quick Start](docs/quick_start.md)
- [Configuration Reference](docs/configuration.md)
- [Custom Commands & JSON API](docs/command.md)

## License

[MIT](LICENSE)

Copyright (c) 2026 Tomoo Kikuchi
