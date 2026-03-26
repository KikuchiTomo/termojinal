# Features

Termojinal is a GPU-accelerated terminal emulator for macOS, built for developers who work with AI coding agents.

---

## GPU-Accelerated Rendering

Everything is rendered on the GPU via wgpu + Metal. Scrolling through large outputs is smooth, window resizing is instantaneous, and semi-transparent windows with blur effects run at full frame rate.

---

## Daemon-Owned PTY

The GUI is a thin client. The daemon (`termojinald`) owns all PTY sessions, so your shells survive GUI restarts.

- `tm exit` -- disconnect the GUI. All shells keep running.
- `tm kill` -- terminate a shell session.

---

## Workspaces

Isolated environments within a single window. Each workspace has its own tabs, panes, and working directory.

| Action | Shortcut |
|--------|----------|
| New workspace | `Cmd+N` |
| Switch to workspace N | `Cmd+1` through `Cmd+9` |
| Next / previous workspace | `Cmd+Shift+]` / `[` |

---

## Tabs

Each workspace can have multiple tabs with independent terminal sessions.

| Action | Shortcut |
|--------|----------|
| New tab | `Cmd+T` |
| Close tab | `Cmd+W` |
| Next / previous tab | `Cmd+Shift+}` / `{` |

The tab title uses a fallback chain: shell-set title, then CWD basename, then "Tab N". Customize the format in `config.toml` under `[tab_bar]`.

---

## Split Panes

Split the current pane to view multiple sessions side by side.

| Action | Shortcut |
|--------|----------|
| Split right | `Cmd+D` |
| Split down | `Cmd+Shift+D` |
| Next / previous pane | `Cmd+]` / `[` |
| Zoom pane (fullscreen toggle) | `Cmd+Shift+Enter` |
| Extract pane to tab | `Cmd+Shift+T` |

- Drag the separator to resize panes.
- Drag a tab into the pane area to create a new split (tab drag pane split).
- `Cmd+Shift+T` extracts the focused pane into its own tab (only when the tab has multiple panes).

---

## Claudes Dashboard

`Cmd+Shift+C` -- A lazygit-style 2-pane overview of all Claude Code sessions across workspaces. See status, switch between sessions, and manage permission requests at a glance.

---

## Quick Launch

`Cmd+O` -- Fuzzy search overlay for quickly switching between workspaces, tabs, and panes. Type to filter, Enter to jump.

---

## Sidebar

Toggle with `Cmd+B`. Shows each workspace with:

- Workspace name and color dot
- Git branch and dirty status
- Active ports
- Notification dot for unread alerts
- Allow Flow accent stripe for pending AI permission requests
- AI agent status indicator (hooks-based, no polling)

Agent status indicator styles (configurable):

| Style | Behavior |
|-------|----------|
| `"pulse"` | Animated glow on the workspace dot (default) |
| `"color"` | Static color (purple = active, yellow = permission wait) |
| `"none"` | Hidden |

Sidebar width is draggable (default 240px, range 120-400px).

---

## Command Palette & File Finder

`Cmd+Shift+P` -- Opens the palette with two modes.

### File finder mode (default)

Shows the contents of the focused pane's working directory.

- **Type** to filter entries by prefix
- **Arrow keys** to navigate, **Tab** to autocomplete
- **Enter** on a directory: `cd` into it. On a file: `cd` to the file's parent directory
- **Shift+Enter**: open in your editor (`$EDITOR`, falls back to `nvim`)
- **`..`** navigates to the parent directory
- **Backspace** on empty input: go to parent directory (at root: dismiss)
- **`/`** in input: navigate into subdirectories (e.g., `src/` enters `src`)

### Command mode

Type **`>`** as the first character to switch to command mode. Fuzzy search across built-in actions and custom commands. Backspace on empty input returns to file finder mode.

---

## Allow Flow

When Claude Code needs permission, Termojinal intercepts the request via a `PermissionRequest` hook and shows it in the sidebar. Respond from anywhere:

| Key | Action |
|-----|--------|
| `y` | Allow one |
| `n` | Deny one |
| `Y` | Allow ALL pending |
| `N` | Deny ALL pending |
| `a` / `A` | Allow and remember (persistent rule) |
| `Esc` | Dismiss hint bar |

You can also add custom detection patterns for other tools:

```toml
[[allow_flow.patterns]]
tool = "My Deploy Tool"
action = "production deploy"
pattern = "Deploy to production\\? \\[y/N\\]"
yes_response = "y\n"
no_response = "n\n"
```

---

## Quick Terminal

`Cmd+`` ` -- A Quake-style drop-down terminal from the top of the screen. Works even when Termojinal is not focused (requires daemon).

Press the hotkey again or Escape to dismiss. Sessions persist between toggles in a dedicated workspace.

### Configuration

| Option | Default | Description |
|--------|---------|-------------|
| `enabled` | `true` | Enable the feature |
| `hotkey` | `"ctrl+\`"` | Global hotkey |
| `animation` | `"slide_down"` | `slide_down`, `slide_up`, `fade`, `none` |
| `animation_duration_ms` | `200` | Animation speed |
| `height_ratio` | `0.4` | Height as fraction of screen |
| `width_ratio` | `1.0` | Width as fraction of screen |
| `position` | `"center"` | `left`, `center`, `right` |
| `screen_edge` | `"top"` | `top`, `bottom` |
| `hide_on_focus_loss` | `false` | Auto-hide when clicking away |
| `dismiss_on_esc` | `true` | Hide on Escape |
| `window_level` | `"floating"` | `normal`, `floating`, `above_all` |
| `corner_radius` | `12.0` | Corner radius in pixels |

---

## Directory Tree

Toggle with `Cmd+Shift+E`. A file browser panel in the sidebar.

Enable in `config.toml`:

```toml
[directory_tree]
enabled = true
```

- Git-aware root detection (`auto`, `cwd`, `git_root`)
- Arrow keys to navigate, prefix search to jump
- Double-click a directory to `cd`
- Press `v` on a file to open in your editor (`$EDITOR` or nvim)

---

## Time Travel (Command History)

Navigate between commands using OSC 133 shell integration markers.

| Action | Shortcut |
|--------|----------|
| Previous command | `Cmd+Up` |
| Next command | `Cmd+Down` |
| First command | `Cmd+Shift+Up` |
| Last command | `Cmd+Shift+Down` |
| Command timeline | `Cmd+Shift+H` |

### Configuration

```toml
[time_travel]
command_history = true
max_command_history = 10000
command_navigation = true
show_command_marker = true
show_command_position = true
timeline_ui = true
session_persistence = true
restore_on_startup = true
snapshots = true
max_snapshots_per_session = 50
```

---

## Search

`Cmd+F` -- Search through the terminal scrollback buffer. Case-insensitive substring matching with customizable highlight colors.

---

## Status Bar

A configurable bar at the bottom showing real-time session info. Composed of left and right segments using template variables.

### Available variables

`{user}`, `{host}`, `{cwd}`, `{cwd_short}`, `{git_branch}`, `{git_status}`, `{git_remote}`, `{git_worktree}`, `{git_stash}`, `{git_ahead}`, `{git_behind}`, `{git_dirty}`, `{git_untracked}`, `{ports}`, `{shell}`, `{pid}`, `{pane_size}`, `{font_size}`, `{workspace}`, `{workspace_index}`, `{tab}`, `{tab_index}`, `{time}`, `{date}`

### Example

```toml
[status_bar]
enabled = true

[[status_bar.left]]
content = "{user}@{host}"
fg = "#1A1A28"
bg = "#7AA2F7"

[[status_bar.right]]
content = "{time}"
fg = "#1A1A28"
bg = "#7AA2F7"
```

---

## Inline Images

Three protocols supported: **Kitty Graphics Protocol**, **iTerm2 Inline Images** (OSC 1337), and **Sixel Graphics**. Images are GPU-composited alongside text.

---

## CJK & Japanese Input

- CJK full-width characters rendered at correct double width
- Inline IME: Japanese input composing state rendered at cursor position with distinct background color (`preedit_bg`)
- No separate input window -- type Japanese directly in the terminal

---

## Color Emoji

Rendered via macOS Core Text. Same emoji rendering as native macOS apps, displayed inline at the correct width.

---

## Theme & Appearance

- Default theme: **Catppuccin Mocha**
- Custom themes in `~/.config/termojinal/themes/<name>.toml`
- Auto dark/light switching following macOS appearance
- Configurable window opacity, bold brightness, dim opacity
- Font size zoom: `Cmd+=` / `Cmd+-`

---

## Copy with Colors

`Cmd+C` with selected text copies to clipboard in RTF format, preserving terminal colors. Without a selection, `Cmd+C` sends Ctrl+C to the terminal.

---

## Option+Click

Hold Option and click on text: URLs open in the default browser, file paths open via macOS `open`.

---

## Brew Update Checker

On launch, checks for updates via Homebrew (formula and cask). Shows a notification if a newer version is available.

---

## Custom Commands

Scripts that extend Termojinal via the command palette (enter command mode with `>`). They communicate via line-delimited JSON over stdin/stdout.

### Directory structure

```
~/.config/termojinal/commands/my-command/
├── command.toml    # Manifest
└── run.sh          # Entry point
```

### Interactive UI types

| Type | Description |
|------|-------------|
| `fuzzy` | Filterable single-select list |
| `multi` | Multi-select with checkboxes |
| `confirm` | Yes/No dialog |
| `text` | Text input with optional completions |
| `info` | Progress message |
| `done` | Completion signal with optional notification |
| `error` | Error message |

### Bundled commands

`hello-world`, `start-review`, `switch-worktree`, `kill-merged`, `clone-and-open`, `run-agent`

### SDK

A typed Deno SDK (`@termojinal/sdk`) with helpers: `fuzzy()`, `multi()`, `confirm()`, `text()`, `info()`, `done()`, `error()`.

See [command.md](command.md) for the full protocol reference.

---

## MCP Server

Gives Claude Code workspace control -- create tabs, read terminal content, approve permissions.

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

---

## Notifications

Desktop notifications via macOS Notification Center. Triggered by command completion, Allow Flow requests, and custom commands.

```toml
[notifications]
enabled = true
sound = false
```

---

## Command Signing

Commands can be signed with Ed25519 for trust verification.

1. Generate keypair: `termojinal-sign --generate-key`
2. Sign: `termojinal-sign path/to/command.toml <secret-key-hex>`
3. Verified commands show a checkmark in the palette.

---

## All Keybindings

| Key | Action |
|-----|--------|
| `Cmd+Shift+P` | Command palette |
| `Cmd+O` | Quick Launch |
| `Cmd+Shift+C` | Claudes Dashboard |
| `Cmd+`` ` | Quick Terminal (global) |
| `Cmd+D` | Split right |
| `Cmd+Shift+D` | Split down |
| `Cmd+]` / `[` | Next / previous pane |
| `Cmd+Shift+Enter` | Zoom pane |
| `Cmd+Shift+T` | Extract pane to tab |
| `Cmd+T` | New tab |
| `Cmd+W` | Close tab |
| `Cmd+Shift+}` / `{` | Next / previous tab |
| `Cmd+N` | New workspace |
| `Cmd+1`-`9` | Switch workspace |
| `Cmd+Shift+]` / `[` | Next / previous workspace |
| `Cmd+B` | Toggle sidebar |
| `Cmd+Shift+E` | Toggle directory tree |
| `Cmd+F` | Search |
| `Cmd+=` / `-` | Font size up / down |
| `Cmd+A` | Select all |
| `Cmd+C` / `V` | Copy / paste |
| `Cmd+K` | Clear scrollback |
| `Cmd+L` | Clear screen |
| `Cmd+Up` / `Down` | Previous / next command |
| `Cmd+Shift+Up` / `Down` | First / last command |
| `Cmd+Shift+H` | Command timeline |
| `Cmd+,` | Open settings |
| `Cmd+Q` | Quit |
| Option+click | Open URL / path |

All keybindings are customizable in `~/.config/termojinal/keybindings.toml` with three layers: **normal**, **global**, and **alternate_screen**.

See [configuration.md](configuration.md) for the complete reference.

---

## Further reading

- [Quick Start](quick_start.md) -- Installation and first steps
- [Configuration Reference](configuration.md) -- Complete config.toml reference
- [Custom Commands & JSON API](command.md) -- Command protocol specification
