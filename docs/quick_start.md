# Quick Start

Welcome to Termojinal, a GPU-accelerated terminal emulator for macOS.

## Install

### Homebrew (recommended)

```bash
brew install KikuchiTomo/tap/termojinal
brew services start termojinal   # start the daemon
```

### From source

```bash
git clone https://github.com/KikuchiTomo/termojinal.git
cd termojinal
make install   # install Rust toolchain + fetch deps
make build     # release build
make app       # create Termojinal.app
```

## First launch

Open `Termojinal.app` from `/Applications` (Homebrew) or `target/release/Termojinal.app` (source).

On first launch macOS will ask for notification permission. Allow it to receive Claude Code permission prompts and command completion alerts.

## Setup for Claude Code

```bash
tm setup
```

This single command:
- Creates `~/.config/termojinal/`
- Installs Claude Code notification and permission hooks
- Links bundled commands

## Key bindings

| Action | Shortcut |
|--------|----------|
| Split right | Cmd+D |
| Split down | Cmd+Shift+D |
| Next pane | Cmd+] |
| Previous pane | Cmd+[ |
| Zoom pane | Cmd+Shift+Enter |
| New tab | Cmd+T |
| Close tab | Cmd+W |
| Next/prev tab | Cmd+Shift+} / { |
| New workspace | Cmd+N |
| Switch workspace | Cmd+1 through Cmd+9 |
| Command palette | Cmd+Shift+P |
| Toggle sidebar | Cmd+B |
| Search | Cmd+F |
| Quick Terminal | Ctrl+` |
| Quit | Cmd+Q |

All keybindings are customizable in `~/.config/termojinal/keybindings.toml`.

## Allow Flow (AI permission management)

When Claude Code needs permission (file edits, shell commands, etc.), Termojinal shows a notification and hint bar. Respond from anywhere:

| Key | Action |
|-----|--------|
| y | Allow one request |
| n | Deny one request |
| Y | Allow ALL pending requests |
| N | Deny ALL pending requests |
| a / A | Allow and remember (persistent rule) |
| Esc | Dismiss hint bar |

## Custom commands

Commands are scripts that communicate via JSON over stdio. Place them in `~/.config/termojinal/commands/` and access via the command palette.

See [command.md](command.md) for the full protocol reference.

## Configuration

Edit `~/.config/termojinal/config.toml` to customize fonts, colors, sidebar, status bar, and more.

See [configuration.md](configuration.md) for the complete reference.

## Architecture

| Binary | Purpose |
|--------|---------|
| `Termojinal.app` | GUI application |
| `termojinald` | Session daemon (global hotkeys, persistence) |
| `tm` | CLI tool (setup, notifications, allow flow) |
| `termojinal-mcp` | MCP server for Claude Code integration |
| `termojinal-sign` | Ed25519 command signer |

## Further reading

- [Configuration reference](configuration.md)
- [Custom commands & JSON API](command.md)
