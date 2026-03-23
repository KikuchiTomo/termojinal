# Features

Termojinal is a GPU-accelerated terminal emulator for macOS, built for developers who work with AI coding agents. This document provides a detailed overview of every feature.

---

## Table of Contents

- [GPU-Accelerated Rendering](#gpu-accelerated-rendering)
- [Workspaces](#workspaces)
- [Tabs](#tabs)
- [Split Panes](#split-panes)
- [Sidebar](#sidebar)
- [Command Palette](#command-palette)
- [Allow Flow (AI Permission Management)](#allow-flow)
- [Quick Terminal](#quick-terminal)
- [Directory Tree](#directory-tree)
- [Time Travel (Command History)](#time-travel)
- [Search](#search)
- [Status Bar](#status-bar)
- [Inline Images](#inline-images)
- [CJK & Japanese Input](#cjk--japanese-input)
- [Color Emoji](#color-emoji)
- [Theme & Appearance](#theme--appearance)
- [Keybindings](#keybindings)
- [Notifications](#notifications)
- [Custom Commands (Plugins)](#custom-commands)
- [MCP Server](#mcp-server)
- [Session Daemon](#session-daemon)
- [CLI Tool (tm)](#cli-tool)
- [Command Signing](#command-signing)
- [Git Integration](#git-integration)
- [Port Detection](#port-detection)

---

## GPU-Accelerated Rendering

Termojinal renders everything on the GPU using **wgpu** with a Metal backend on macOS. This provides smooth, low-latency drawing even at high refresh rates and large window sizes.

### How it works

1. **Font atlas** — Glyphs are rasterized on the CPU using fontdue and packed into a single texture atlas that is uploaded to the GPU once.
2. **Instanced rendering** — Each visible cell is drawn as an instanced quad in a single draw call, referencing the correct glyph in the atlas.
3. **WGSL shaders** — Custom vertex and fragment shaders handle color conversion for the full xterm 256-color palette plus true color (24-bit RGB).
4. **SDF rendering** — UI elements like the command palette use Signed Distance Field (SDF) rounded rectangles with Gaussian blur for a frosted-glass effect.
5. **Image pipeline** — Inline images (Kitty, Sixel, iTerm2) are uploaded as separate GPU textures and composited alongside text.

### What this means in practice

- Scrolling through large outputs is buttery smooth.
- Window resizing is instantaneous — no flash of blank space.
- Semi-transparent windows and blur effects run at full frame rate.

---

## Workspaces

Workspaces are isolated environments within a single Termojinal window. Each workspace has its own set of tabs and panes, with independent working directories.

| Action | Shortcut |
|--------|----------|
| New workspace | `Cmd+N` |
| Switch to workspace N | `Cmd+1` through `Cmd+9` |
| Next workspace | `Cmd+Shift+]` |
| Previous workspace | `Cmd+Shift+[` |

Workspaces appear in the sidebar with their name, git branch, active ports, and AI agent status. The active workspace is highlighted.

**Use case:** Run your dev server in workspace 1, Claude Code in workspace 2, and database tools in workspace 3 — switch between them instantly with `Cmd+1`/`2`/`3`.

---

## Tabs

Each workspace can contain multiple tabs. Tabs share the workspace's sidebar entry but have independent terminal sessions.

| Action | Shortcut |
|--------|----------|
| New tab | `Cmd+T` |
| Close tab | `Cmd+W` |
| Next tab | `Cmd+Shift+}` |
| Previous tab | `Cmd+Shift+{` |

### Tab title format

The tab bar supports a fallback-chain format string:

```
{title|cwd_base|Tab {index}}
```

- `title` — the tab title set by the shell (via OSC escape sequences)
- `cwd_base` — the basename of the current working directory
- `Tab {index}` — literal text with the 1-based tab index

The first non-empty value wins. Customize this in `config.toml` under `[tab_bar]`.

### Tab bar options

- Show/hide when only one tab exists (`always_show`)
- Position: top or bottom
- Customizable colors, height, min/max tab width
- Active tab underline accent

---

## Split Panes

Split the current pane horizontally or vertically to view multiple terminal sessions side by side within a single tab.

| Action | Shortcut |
|--------|----------|
| Split right (horizontal) | `Cmd+D` |
| Split down (vertical) | `Cmd+Shift+D` |
| Next pane | `Cmd+]` |
| Previous pane | `Cmd+[` |
| Zoom pane (toggle fullscreen) | `Cmd+Shift+Enter` |

### Immutable tree layout

Pane splits are managed by an **immutable split pane tree** — a functional, persistent data structure. This means:

- Pane configurations are robust and predictable.
- Splits can be nested arbitrarily deep.
- Each split stores a ratio that can be adjusted by dragging the separator.

### Pane separator

- Configurable separator color and width.
- Draggable — grab and drag to resize panes.
- Hit tolerance is configurable (`separator_tolerance`) so it's easy to grab even at 1–2px width.

### Focus border

The focused pane shows a colored border (supports alpha transparency). Both the color and width are configurable.

### Scrollbar

Each pane has its own scrollbar with configurable thumb and track opacity.

---

## Sidebar

The vertical sidebar on the left side of the window is the central navigation hub.

Toggle: `Cmd+B`

### What it shows

For each workspace:

- **Workspace name** with a color dot
- **Git branch** name (e.g. `main`, `feature/login`)
- **Git dirty status** indicator (yellow dot when there are uncommitted changes)
- **Active ports** detected in the session
- **Notification dot** (orange) for unread alerts
- **Allow Flow indicator** — a left accent stripe when a workspace has pending AI permission requests
- **AI agent status** — shows whether a Claude Code (or other AI agent) session is active, with configurable indicator styles

### Agent status indicators

When a Claude Code session is detected in a workspace, the sidebar shows its status:

| Style | Behavior |
|-------|----------|
| `"pulse"` | The workspace dot pulses with an animated glow (default) |
| `"color"` | The dot changes to a static color (purple = active, yellow = waiting for permission) |
| `"none"` | No agent indicator |

### Sidebar resizing

The sidebar width is draggable. Hold and drag the right edge to resize:

- Default width: 240px
- Min width: 120px
- Max width: 400px

---

## Command Palette

Open with `Cmd+Shift+P`. A fuzzy search overlay for accessing built-in actions and custom commands.

### Visual design

The palette is rendered entirely on the GPU with:

- **SDF rounded rectangles** for crisp corners at any resolution
- **Frosted-glass blur** behind the palette (configurable blur radius, default 20px)
- **Drop shadow** with configurable radius and opacity
- **Dark overlay** behind the palette to dim the terminal content

### Features

- Fuzzy text matching — type any part of a command name to filter
- Keyboard navigation — arrow keys to select, Enter to execute, Escape to dismiss
- Scrolling — when more commands than `max_visible_items` (default 10), the list scrolls
- Width configurable as a fraction of the window (`width_ratio`, default 60%)

### Command kinds

| Kind | Display | Description |
|------|---------|-------------|
| Builtin | "Builtin" | Actions shipped with Termojinal |
| Plugin | "Plugin" | Unsigned user-created commands |
| PluginVerified | "Verified" with checkmark | Commands signed with a valid Ed25519 signature |

---

## Allow Flow

Allow Flow is Termojinal's AI agent coordination system. When tools like Claude Code need permission — to edit a file, run a shell command, or access a resource — Termojinal intercepts the request and lets you approve or deny it without switching focus.

### How it works

1. Claude Code sends a `PermissionRequest` hook via structured IPC (not terminal output parsing).
2. Termojinal shows a **notification** and a **hint bar** at the bottom of the relevant pane.
3. The **sidebar** shows a blue accent stripe on the workspace with pending requests.
4. You respond with a single key from any workspace:

| Key | Action |
|-----|--------|
| `y` | Allow this one request |
| `n` | Deny this one request |
| `Y` | Allow ALL pending requests in this workspace |
| `N` | Deny ALL pending requests in this workspace |
| `a` / `A` | Allow and **remember** (creates a persistent rule) |
| `Esc` | Dismiss the hint bar |

### Custom detection patterns

Beyond the built-in Claude Code integration, you can add regex patterns to detect permission prompts from any tool:

```toml
[[allow_flow.patterns]]
tool = "My Deploy Tool"
action = "production deploy"
pattern = "Deploy to production\\? \\[y/N\\]"
yes_response = "y\n"
no_response = "n\n"
```

### Configuration options

| Option | Default | Description |
|--------|---------|-------------|
| `overlay_enabled` | `true` | Show a hint bar in the pane |
| `side_panel_enabled` | `true` | Show pending requests in sidebar |
| `auto_focus` | `false` | Auto-focus the pane when a request arrives |
| `sound` | `false` | Play a sound on permission request |

---

## Quick Terminal

A Quake-style drop-down terminal that slides from the edge of the screen with a global hotkey — works even when Termojinal is not focused.

**Default hotkey:** `Ctrl+`` ` (requires the daemon to be running)

### How it works

Press the hotkey anywhere on your Mac. A terminal window slides down from the top of the screen. Press the hotkey again (or Escape) to dismiss it. The Quick Terminal has its own dedicated workspace, so your sessions persist between toggles.

### Configuration

| Option | Default | Description |
|--------|---------|-------------|
| `enabled` | `true` | Enable the feature |
| `hotkey` | `"ctrl+\`"` | Global hotkey (customizable) |
| `animation` | `"slide_down"` | Animation: `slide_down`, `slide_up`, `fade`, `none` |
| `animation_duration_ms` | `200` | Animation speed |
| `height_ratio` | `0.4` | Height as fraction of screen (0.0–1.0) |
| `width_ratio` | `1.0` | Width as fraction of screen (0.0–1.0) |
| `position` | `"center"` | Horizontal position: `left`, `center`, `right` |
| `screen_edge` | `"top"` | Edge to slide from: `top`, `bottom` |
| `hide_on_focus_loss` | `false` | Auto-hide when clicking away |
| `dismiss_on_esc` | `true` | Hide on Escape |
| `show_sidebar` | `false` | Show sidebar in quick terminal |
| `show_tab_bar` | `false` | Show tab bar in quick terminal |
| `show_status_bar` | `true` | Show status bar in quick terminal |
| `window_level` | `"floating"` | Stacking: `normal`, `floating`, `above_all` |
| `corner_radius` | `12.0` | Corner radius in pixels |
| `own_workspace` | `true` | Dedicated workspace for quick terminal |

---

## Directory Tree

A file browser panel in the sidebar that shows the directory structure of the current working directory.

### Activation

Enable in `config.toml`:

```toml
[directory_tree]
enabled = true
```

### Features

- **Git-aware root detection** — The tree root can be set to:
  - `auto` — uses the git repository root if inside a repo, otherwise the CWD
  - `cwd` — always uses the current working directory
  - `git_root` — always uses the git repository root
- **Expandable/collapsible directories** — click or use keyboard to expand
- **Color-coded entries** — directories and files have distinct colors (customizable)
- **Keyboard navigation** — arrow keys to move, prefix search to jump to entries
- **File actions:**
  - Double-click a directory to `cd` into it
  - Press `v` on a file to open it in your editor (`$EDITOR` or nvim by default)
- **Scrolling** — configurable `max_visible_lines` (default 20) before the tree scrolls

---

## Time Travel

Time Travel tracks your command history using OSC 133 shell integration markers, letting you navigate between commands and restore previous sessions.

### Command history

- **Recording** — Every command boundary is tracked automatically via OSC 133 escape sequences (supported by modern shells like zsh, bash, fish).
- **Navigation** — Jump between commands with `Cmd+Up` / `Cmd+Down`.
- **Command marker** — A visual marker in the left gutter shows where each command begins.
- **Command position** — The status bar shows your current position (e.g. "Command 15/42").
- **Capacity** — Stores up to 10,000 command records per session (configurable).

### Timeline UI

Open with `Cmd+Shift+T`. A visual timeline of your command history for quick navigation.

### Session persistence

- **Save on exit** — Full session state is saved when Termojinal closes.
- **Restore on startup** — Previous sessions are automatically restored when Termojinal launches.
- **Named snapshots** — Save and name specific points in your session. Up to 50 snapshots per session (configurable).

### Configuration

```toml
[time_travel]
command_history = true           # Enable command recording
max_command_history = 10000      # Max records per session
command_navigation = true        # Cmd+Up/Down navigation
show_command_marker = true       # Gutter markers
show_command_position = true     # Status bar position
timeline_ui = true               # Cmd+Shift+T timeline
session_persistence = true       # Save on exit
restore_on_startup = true        # Restore on launch
snapshots = true                 # Named snapshots
max_snapshots_per_session = 50   # Max snapshots
```

---

## Search

Search through the terminal scrollback buffer to find text.

**Open:** `Cmd+F`

### Features

- Case-insensitive substring search
- Matching text is highlighted with customizable colors (`search_highlight_bg`, `search_highlight_fg`)
- Translucent search bar overlay with configurable background, text color, and border

---

## Status Bar

A configurable bar at the bottom of the window showing real-time information about your session.

### Segments

The status bar is composed of **left-aligned** and **right-aligned** segments. Each segment has:

- `content` — a template string with `{variable}` placeholders
- `fg` — foreground (text) color
- `bg` — background color

### Available template variables

| Variable | Description |
|----------|-------------|
| `{user}` | Current username |
| `{host}` | System hostname |
| `{cwd}` | Full working directory path |
| `{cwd_short}` | Working directory with `~` for home |
| `{git_branch}` | Current git branch |
| `{git_status}` | Git status summary |
| `{git_remote}` | Git remote name |
| `{git_worktree}` | Git worktree path |
| `{git_stash}` | Stash count |
| `{git_ahead}` | Commits ahead of remote |
| `{git_behind}` | Commits behind remote |
| `{git_dirty}` | Modified file count |
| `{git_untracked}` | Untracked file count |
| `{ports}` | Listening ports in session |
| `{shell}` | Shell name (zsh, bash, etc.) |
| `{pid}` | Shell process ID |
| `{pane_size}` | Pane dimensions (cols x rows) |
| `{font_size}` | Current font size |
| `{workspace}` | Workspace name |
| `{workspace_index}` | Workspace index (1-based) |
| `{tab}` | Tab name |
| `{tab_index}` | Tab index (1-based) |
| `{time}` | Current time (HH:MM) |
| `{date}` | Current date (YYYY-MM-DD) |

### Example

```toml
[status_bar]
enabled = true

[[status_bar.left]]
content = "{user}@{host}"
fg = "#1A1A28"
bg = "#7AA2F7"

[[status_bar.left]]
content = "{cwd_short}"
fg = "#C0C0CC"
bg = "#1A1A28"

[[status_bar.left]]
content = "{git_branch} +{git_ahead} -{git_behind}"
fg = "#A6E3A1"
bg = "#0F0F18"

[[status_bar.right]]
content = "{time}"
fg = "#1A1A28"
bg = "#7AA2F7"
```

---

## Inline Images

Termojinal supports displaying images directly in the terminal using three protocols.

### Supported protocols

| Protocol | Description |
|----------|-------------|
| **Kitty Graphics Protocol** | APC-based protocol supporting RGB, RGBA, and PNG. Supports direct transmission, chunked transfer, placement, and deletion. |
| **iTerm2 Inline Images** | OSC 1337 protocol used by iTerm2 and other terminals. |
| **Sixel Graphics** | DCS-based legacy format for terminal graphics. |

### How it works

Image data is received via escape sequences, decoded, and uploaded to the GPU as separate textures. They are composited alongside text content during rendering, giving smooth scrolling and resizing of image content.

---

## CJK & Japanese Input

Termojinal was built with Japanese input as a first-class requirement.

### CJK full-width characters

- Characters that occupy two columns (CJK ideographs, full-width katakana, etc.) are rendered at double width.
- Ambiguous-width characters are handled correctly.

### Inline IME (Input Method Editor)

- The macOS IME composing state is rendered inline at the cursor position.
- Pre-edit text has a distinct background color (`preedit_bg`) so you can see what's being composed.
- No separate input window, no context switching — type Japanese directly into the terminal.

---

## Color Emoji

Color emoji are rendered using macOS Core Text, giving you the same emoji rendering as other native macOS applications. Emoji display inline with text at the correct width.

---

## Theme & Appearance

### Built-in theme

The default theme follows the **Catppuccin Mocha** palette — a warm, eye-friendly dark theme.

### Custom themes

Create theme files in `~/.config/termojinal/themes/<name>.toml`. A theme file has the same structure as the `[theme]` section in `config.toml`:

```toml
# ~/.config/termojinal/themes/nord.toml
background = "#2E3440"
foreground = "#D8DEE9"
cursor = "#D8DEE9"
selection_bg = "#434C5E"
black = "#3B4252"
red = "#BF616A"
green = "#A3BE8C"
# ... (all 16 ANSI colors)
```

### Auto dark/light switching

Termojinal can automatically switch themes when macOS toggles between dark and light appearance:

```toml
[theme]
auto_switch = true
dark = "catppuccin-mocha"
light = "catppuccin-latte"
```

### Appearance options

| Option | Default | Description |
|--------|---------|-------------|
| Window opacity | `1.0` | Semi-transparent windows (0.0–1.0) |
| Bold brightness | `1.2` | Brightness multiplier for bold text |
| Dim opacity | `0.6` | Opacity for dim (faint) text |
| Font size zoom | `Cmd+=` / `Cmd+-` | Adjustable font size (up to `max_size`) |

---

## Keybindings

All keybindings are fully customizable in `~/.config/termojinal/keybindings.toml`.

### Three-layer system

| Layer | When active |
|-------|-------------|
| **normal** | Termojinal is focused, regular shell running |
| **global** | Always active, even when Termojinal is not focused (via macOS CGEventTap) |
| **alternate_screen** | A TUI app (nvim, htop, etc.) is running in alternate screen mode |

User overrides are **merged** with defaults — you only need to specify the bindings you want to change.

### Default keybindings

| Key | Action |
|-----|--------|
| `Cmd+D` | Split right |
| `Cmd+Shift+D` | Split down |
| `Cmd+Shift+Enter` | Zoom pane |
| `Cmd+]` / `Cmd+[` | Next / previous pane |
| `Cmd+T` | New tab |
| `Cmd+W` | Close tab |
| `Cmd+Shift+}` / `{` | Next / previous tab |
| `Cmd+N` | New workspace |
| `Cmd+1`–`Cmd+9` | Switch to workspace N |
| `Cmd+Shift+]` / `[` | Next / previous workspace |
| `Cmd+Shift+P` | Command palette |
| `Cmd+B` | Toggle sidebar |
| `Cmd+F` | Search |
| `Cmd+=` / `Cmd+-` | Font size up / down |
| `Cmd+K` | Clear scrollback |
| `Cmd+L` | Clear screen |
| `Cmd+C` / `Cmd+V` | Copy / paste |
| `Cmd+,` | Open settings |
| `Cmd+Q` | Quit |
| `Ctrl+`` ` | Toggle Quick Terminal (global) |

### Special actions

| Action | Description |
|--------|-------------|
| `passthrough` | Forward the key directly to the PTY (bypass Termojinal) |
| `none` | Disable a default binding |
| `{ "command" = "name" }` | Run a custom command or plugin |

### Example overrides

```toml
[normal]
"cmd+d" = "new_tab"                          # Remap Cmd+D
"cmd+q" = "none"                             # Disable accidental quit
"cmd+shift+r" = { "command" = "start-review" }  # Custom command

[alternate_screen]
"cmd+c" = "passthrough"                      # Let nvim handle Cmd+C
```

---

## Notifications

Desktop notifications are delivered via macOS **NSNotificationCenter** (the modern `UNUserNotificationCenter` API).

### When notifications are sent

- **Command completion** — When a long-running command finishes
- **Allow Flow requests** — When an AI agent needs permission
- **Custom commands** — When a command sends a `done` message with a `notify` field

### Configuration

```toml
[notifications]
enabled = true    # Enable/disable notifications
sound = true      # Play a sound with notifications (default)
```

---

## Custom Commands

Custom commands are scripts that extend Termojinal through the command palette. They communicate via **line-delimited JSON over stdin/stdout** and can be written in any language (Bash, Python, Deno, compiled binaries).

### Directory structure

```
~/.config/termojinal/commands/
├── my-command/
│   ├── command.toml    # Manifest
│   └── run.sh          # Entry point
```

### Interactive UI types

Commands can present the following UI elements:

| Type | Description |
|------|-------------|
| `fuzzy` | Filterable single-select list |
| `multi` | Filterable multi-select list with checkboxes |
| `confirm` | Yes/No dialog |
| `text` | Text input field with optional completions |
| `info` | Progress message (fire-and-forget) |
| `done` | Completion signal with optional notification |
| `error` | Error message |

### Bundled commands

| Command | Description |
|---------|-------------|
| `hello-world` | Protocol demonstration |
| `start-review` | GitHub PR review workflow (requires `gh` CLI) |
| `switch-worktree` | Git worktree switching |
| `kill-merged` | Merged branch cleanup |
| `clone-and-open` | Clone and open a repository |
| `run-agent` | Launch an AI agent (Claude Code, Codex, Aider) |

### SDK

A typed **Deno SDK** (`@termojinal/sdk`) is provided with high-level helpers: `fuzzy()`, `multi()`, `confirm()`, `text()`, `info()`, `done()`, `error()`.

See [command.md](command.md) for the full protocol reference.

---

## MCP Server

The MCP (Model Context Protocol) server gives Claude Code direct control over Termojinal workspaces.

### Binary

`termojinal-mcp` — communicates via JSON-RPC 2.0 over stdio, connecting to Termojinal through a Unix socket.

### Capabilities

- Create and manage tabs
- Read terminal content
- Approve or deny Allow Flow permission requests
- Update AI agent status in the sidebar

### Setup

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

## Session Daemon

The session daemon (`termojinald`) runs in the background and provides system-level features that require persistent processes.

### What it does

- **Global hotkeys** — Intercepts keyboard events system-wide via macOS CGEventTap, enabling the Quick Terminal hotkey even when Termojinal is not focused.
- **PTY management** — Manages pseudo-terminal sessions.
- **Session persistence** — Saves and restores session state across app restarts.

### Requirements

- **Accessibility permission** — Required in System Settings for CGEventTap to work.

### Starting the daemon

```bash
# Via Homebrew
brew services start termojinal

# Via launchctl
launchctl load ~/Library/LaunchAgents/com.termojinal.daemon.plist
```

---

## CLI Tool

`tm` is the CLI companion tool for interacting with Termojinal from the command line.

### Commands

| Command | Description |
|---------|-------------|
| `tm setup` | One-command setup: creates config directory, installs Claude Code hooks, links bundled commands |
| `tm list` | List active sessions (`--json` for machine-readable output) |
| `tm new` | Create a new session |
| `tm kill <id>` | Kill a session |
| `tm resize <id> <cols> <rows>` | Resize a PTY |
| `tm ping` | Check daemon status |
| `tm notify` | Send a desktop notification |
| `tm allow-request` | Handle Claude Code permission hooks |

---

## Command Signing

Commands can be cryptographically signed using **Ed25519** to establish trust.

### How it works

1. Generate a keypair: `termojinal-sign --generate-key`
2. Sign a command: `termojinal-sign path/to/command.toml <secret-key-hex>`
3. Verification happens automatically at load time.

### Trust levels

| Status | Palette display | Description |
|--------|----------------|-------------|
| Unsigned | "Plugin" | No signature (default for user-created commands) |
| Verified | "Verified" with checkmark | Signature matches the official public key |
| Invalid | Warning indicator | Signature present but verification failed |

---

## Git Integration

Termojinal automatically detects git repository information and displays it throughout the UI.

### Information displayed

| Location | Information |
|----------|-------------|
| Sidebar | Branch name, dirty status indicator |
| Status bar | Branch, remote, stash count, ahead/behind, dirty count, untracked count, worktree |
| Directory tree | Git-aware root detection |

Git information is updated automatically as you work.

---

## Port Detection

Termojinal automatically detects listening ports in your terminal sessions and displays them in the sidebar and status bar via the `{ports}` template variable. This is useful for seeing which dev servers are running without having to run `lsof` or `netstat`.

---

## Further reading

- [Quick Start](quick_start.md) — Installation and first steps
- [Configuration Reference](configuration.md) — Complete config.toml reference
- [Custom Commands & JSON API](command.md) — Command protocol specification
