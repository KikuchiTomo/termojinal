# Quick Start

Welcome to Termojinal, a GPU-accelerated terminal emulator for macOS.

## Install

### Homebrew (recommended)

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

## Command Palette (Cmd+Shift+P)

The command palette opens in **file finder mode** by default.

### File finder mode

Type to explore files and directories in the current working directory.

- **Arrow keys** to navigate, **Tab** to autocomplete
- **Enter** on a directory: `cd` into it. On a file: `cd` to the file's parent directory
- **Shift+Enter**: open in your editor (`$EDITOR`, falls back to `nvim`)
- **`..`** navigates to the parent directory
- **Backspace** on empty input: go to parent directory (at root: dismiss)
- **`/`** in input: navigate into subdirectories (e.g., `src/` enters `src`)

### Command mode

Type **`>`** as the first character to switch to command mode. Fuzzy search across built-in actions and custom commands.

## Key bindings

| Action | Shortcut |
|--------|----------|
| Command Palette | Cmd+Shift+P |
| Quick Launch | Cmd+O |
| Claudes Dashboard | Cmd+Shift+C |
| Quick Terminal | Cmd+\` |
| Split right | Cmd+D |
| Split down | Cmd+Shift+D |
| Next / prev pane | Cmd+] / Cmd+[ |
| Zoom pane | Cmd+Shift+Enter |
| Extract pane to tab | Cmd+Shift+T |
| New tab | Cmd+T |
| Close tab | Cmd+W |
| Next / prev tab | Cmd+Shift+} / { |
| New workspace | Cmd+N |
| Switch workspace | Cmd+1 through Cmd+9 |
| Toggle sidebar | Cmd+B |
| Toggle directory tree | Cmd+Shift+E |
| Search | Cmd+F |
| Font size | Cmd+= / Cmd+- |
| Option+click | Open URL or path via `open` |
| Quit | Cmd+Q |

All keybindings are customizable. See [configuration.md](configuration.md).

## Allow Flow (AI permission management)

When Claude Code needs permission, Termojinal shows a notification and hint bar. Respond from anywhere:

| Key | Action |
|-----|--------|
| y | Allow one request |
| n | Deny one request |
| Y | Allow ALL pending requests |
| N | Deny ALL pending requests |
| a / A | Allow and remember (persistent rule) |
| Esc | Dismiss hint bar |

## Custom commands

Commands are scripts that communicate via JSON over stdio. Place them in `~/.config/termojinal/commands/` and access via the command palette (type `>` to enter command mode).

See [command.md](command.md) for the full protocol reference.

## Configuration

Edit `~/.config/termojinal/config.toml` to customize fonts, colors, sidebar, status bar, and more.

See [configuration.md](configuration.md) for the complete reference.

## Further reading

- [Features](features.md)
- [Configuration reference](configuration.md)
- [Custom commands & JSON API](command.md)
