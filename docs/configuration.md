# Termojinal Configuration Reference

This document is the comprehensive reference for configuring termojinal. It covers the main configuration file, theme files, keybindings, and all available options.

---

## Table of Contents

- [Config File Location](#config-file-location)
- [Complete Reference](#complete-reference)
  - [\[font\]](#font)
  - [\[window\]](#window)
  - [\[theme\]](#theme)
  - [\[sidebar\]](#sidebar)
  - [\[tab\_bar\]](#tab_bar)
  - [\[pane\]](#pane)
  - [\[search\]](#search)
  - [\[palette\]](#palette-command-palette)
  - [\[status\_bar\]](#status_bar)
  - [\[notifications\]](#notifications)
  - [\[allow\_flow\]](#allow_flow)
  - [\[quick\_terminal\]](#quick_terminal)
- [Keybindings](#keybindings)
- [Color Format](#color-format)
- [Theme Files](#theme-files)

---

## Config File Location

Termojinal looks for its configuration in the following order:

1. **Primary (XDG-style):** `~/.config/termojinal/config.toml`
2. **Fallback (macOS standard):** `~/Library/Application Support/termojinal/config.toml`

The first path that exists on disk is used. If neither file exists, termojinal starts with built-in defaults.

If a config file is found but contains parse errors, termojinal logs a warning and falls back to the defaults:

```
config parse error: expected `=`, found newline at line 5 column 1
```

Every field in the config file is optional. You only need to specify the values you want to override; unspecified fields use their defaults.

---

## Complete Reference

### [font]

Font rendering settings.

| Field | Type | Default | Description |
|---|---|---|---|
| `family` | String | `"monospace"` | Font family name. Use any font installed on your system (e.g., `"JetBrains Mono"`, `"Fira Code"`). |
| `size` | f32 | `14.0` | Font size in pixels. |
| `line_height` | f32 | `1.2` | Line height as a multiplier of the font size. |
| `max_size` | f32 | `72.0` | Maximum font size reachable via Cmd+= zoom. |
| `size_step` | f32 | `1.0` | Font size increment per zoom step (Cmd+= / Cmd+-). |

**Example:**

```toml
[font]
family = "JetBrains Mono"
size = 13.0
line_height = 1.3
```

---

### [window]

Initial window geometry and appearance.

| Field | Type | Default | Description |
|---|---|---|---|
| `width` | u32 | `960` | Initial window width in pixels. |
| `height` | u32 | `640` | Initial window height in pixels. |
| `opacity` | f32 | `1.0` | Window background opacity. `0.0` = fully transparent, `1.0` = fully opaque. |
| `padding_x` | f32 | `1.0` | Horizontal padding in cell-width units. |
| `padding_y` | f32 | `0.5` | Vertical padding in cell-height units. |

**Example:**

```toml
[window]
width = 1200
height = 800
opacity = 0.92
```

---

### [theme]

Colors for the terminal content area. The built-in defaults follow the Catppuccin Mocha palette.

#### Core Colors

| Field | Type | Default | Description |
|---|---|---|---|
| `background` | String | `"#1E1E2E"` | Terminal background color. |
| `foreground` | String | `"#CDD6F4"` | Default text color. |
| `cursor` | String | `"#F5E0DC"` | Cursor color. |
| `selection_bg` | String | `"#45475A"` | Selection highlight background. |
| `preedit_bg` | String | `"#313244"` | IME pre-edit composition background. |
| `search_highlight_bg` | String | `"#F9E2AF"` | Search match highlight background. |
| `search_highlight_fg` | String | `"#1E1E2E"` | Search match highlight foreground. |
| `bold_brightness` | f32 | `1.2` | Brightness multiplier applied to bold text colors. |
| `dim_opacity` | f32 | `0.6` | Opacity factor applied to dim (faint) text. |

#### ANSI 16-Color Palette

| Field | Default | Description |
|---|---|---|
| `black` | `"#45475A"` | ANSI color 0 (black) |
| `bright_black` | `"#585B70"` | ANSI color 8 (bright black / dark gray) |
| `red` | `"#F38BA8"` | ANSI color 1 (red) |
| `bright_red` | `"#F38BA8"` | ANSI color 9 (bright red) |
| `green` | `"#A6E3A1"` | ANSI color 2 (green) |
| `bright_green` | `"#A6E3A1"` | ANSI color 10 (bright green) |
| `yellow` | `"#F9E2AF"` | ANSI color 3 (yellow) |
| `bright_yellow` | `"#F9E2AF"` | ANSI color 11 (bright yellow) |
| `blue` | `"#89B4FA"` | ANSI color 4 (blue) |
| `bright_blue` | `"#89B4FA"` | ANSI color 12 (bright blue) |
| `magenta` | `"#F5C2E7"` | ANSI color 5 (magenta) |
| `bright_magenta` | `"#F5C2E7"` | ANSI color 13 (bright magenta) |
| `cyan` | `"#94E2D5"` | ANSI color 6 (cyan) |
| `bright_cyan` | `"#94E2D5"` | ANSI color 14 (bright cyan) |
| `white` | `"#BAC2DE"` | ANSI color 7 (white) |
| `bright_white` | `"#A6ADC8"` | ANSI color 15 (bright white) |

#### Auto Dark/Light Switching

| Field | Type | Default | Description |
|---|---|---|---|
| `auto_switch` | bool | `false` | Automatically switch theme when macOS appearance changes. |
| `dark` | String | `""` | Theme file name to load in dark mode (e.g. `"catppuccin-mocha"`). |
| `light` | String | `""` | Theme file name to load in light mode (e.g. `"catppuccin-latte"`). |

**Example:**

```toml
[theme]
background = "#0D1117"
foreground = "#E6EDF3"
cursor = "#58A6FF"

auto_switch = true
dark = "github-dark"
light = "github-light"
```

---

### [sidebar]

The sidebar shows workspaces, tabs, git branch info, and notification indicators.

| Field | Type | Default | Description |
|---|---|---|---|
| `width` | f32 | `240.0` | Default sidebar width in pixels. |
| `min_width` | f32 | `120.0` | Minimum sidebar width when resizing by drag. |
| `max_width` | f32 | `400.0` | Maximum sidebar width when resizing by drag. |
| `bg` | String | `"#0D0D12"` | Sidebar background color. |
| `active_entry_bg` | String | `"#1A1A24"` | Background color for the active workspace entry. |
| `active_fg` | String | `"#F2F2F8"` | Text color for the active workspace name. |
| `inactive_fg` | String | `"#8C8C99"` | Text color for inactive workspace names. |
| `dim_fg` | String | `"#666670"` | Color for secondary info text (path, metadata). |
| `git_branch_fg` | String | `"#5AB3D9"` | Color for the git branch label. |
| `separator_color` | String | `"#333338"` | Color of horizontal separator lines. |
| `notification_dot` | String | `"#FF941A"` | Color of the unread notification indicator dot. |
| `git_dirty_color` | String | `"#CCB34D"` | Color of the git dirty status indicator. |
| `allow_accent_color` | String | `"#4FC1FF"` | Left stripe accent color for workspaces with pending Allow Flow requests. |
| `allow_hint_fg` | String | `"#7DC8FF"` | Foreground color for Allow Flow hint text. |
| `top_padding` | f32 | `6.0` | Top padding in pixels. |
| `side_padding` | f32 | `6.0` | Side (left/right) padding in pixels. |
| `entry_gap` | f32 | `4.0` | Vertical gap between workspace entries in pixels. |
| `info_line_gap` | f32 | `1.0` | Gap between the workspace name and its info lines in pixels. |

**Example:**

```toml
[sidebar]
width = 260.0
bg = "#0A0A10"
active_entry_bg = "#1A1A28"
git_branch_fg = "#7AA2F7"
entry_gap = 16.0
```

---

### [tab_bar]

The tab bar displays tabs within the current workspace.

| Field | Type | Default | Description |
|---|---|---|---|
| `format` | String | `"{title\|cwd_base\|Tab {index}}"` | Tab title format string. Uses fallback syntax: first non-empty value wins. |
| `always_show` | bool | `false` | Show the tab bar even when only one tab is open. |
| `position` | String | `"top"` | Tab bar position: `"top"` or `"bottom"`. |
| `height` | f32 | `36.0` | Tab bar height in pixels. |
| `max_width` | f32 | `200.0` | Maximum width of a single tab in pixels. |
| `min_tab_width` | f32 | `60.0` | Minimum width of a single tab in pixels. |
| `new_tab_button_width` | f32 | `32.0` | Width of the "+" new-tab button in pixels. |
| `bg` | String | `"#1A1A1F"` | Tab bar background color. |
| `active_tab_bg` | String | `"#2E2E38"` | Active tab background color. |
| `active_tab_fg` | String | `"#F2F2F8"` | Active tab text color. |
| `inactive_tab_fg` | String | `"#8C8C99"` | Inactive tab text color. |
| `accent_color` | String | `"#4D8CFF"` | Active tab underline accent color. |
| `separator_color` | String | `"#383840"` | Separator color between tabs. |
| `close_button_fg` | String | `"#808088"` | Tab close button color. |
| `new_button_fg` | String | `"#808088"` | New tab button ("+") color. |
| `padding_x` | f32 | `6.0` | Horizontal padding inside tabs in pixels. |
| `padding_y` | f32 | `6.0` | Vertical padding inside the tab bar in pixels. |
| `accent_height` | u32 | `2` | Active tab underline thickness in pixels. |
| `bottom_border` | bool | `true` | Show a bottom border below the tab bar. |
| `bottom_border_color` | String | `"#2A2A34"` | Bottom border color. |

#### Tab Format String

The `format` field uses a fallback chain syntax separated by `|`:

```
{title|cwd_base|Tab {index}}
```

- `title` -- the tab title set by the shell (via OSC escape sequences)
- `cwd_base` -- the basename of the current working directory
- `Tab {index}` -- literal text with the 1-based tab index

The first non-empty value in the chain is displayed.

**Example:**

```toml
[tab_bar]
format = "{cwd_base|Tab {index}}"
always_show = true
height = 38.0
accent_color = "#7AA2F7"
```

---

### [pane]

Pane splitting, borders, and scrollbar appearance.

| Field | Type | Default | Description |
|---|---|---|---|
| `separator_color` | String | `"#4D4D4D"` | Color of the line between panes. |
| `focus_border_color` | String | `"#3399FFCC"` | Border color of the focused pane (supports alpha). |
| `separator_width` | u32 | `2` | Pane separator line width in pixels. |
| `focus_border_width` | u32 | `2` | Focused pane border width in pixels. |
| `separator_tolerance` | f32 | `4.0` | Mouse hit tolerance for dragging pane separators in pixels. |
| `scrollbar_thumb_opacity` | f32 | `0.5` | Scrollbar thumb opacity (`0.0`--`1.0`). |
| `scrollbar_track_opacity` | f32 | `0.1` | Scrollbar track opacity (`0.0`--`1.0`). |

**Example:**

```toml
[pane]
separator_color = "#2A2A38"
focus_border_color = "#7AA2F7CC"
separator_width = 1
scrollbar_thumb_opacity = 0.4
```

---

### [search]

Search bar overlay appearance.

| Field | Type | Default | Description |
|---|---|---|---|
| `bar_bg` | String | `"#262633F2"` | Search bar background (supports alpha for translucency). |
| `input_fg` | String | `"#F2F2F2"` | Search input text color. |
| `border_color` | String | `"#4D4D66"` | Search bar border color. |

**Example:**

```toml
[search]
bar_bg = "#1A1A28F0"
input_fg = "#E8E8F0"
border_color = "#3A3A50"
```

---

### [palette] (Command Palette)

The command palette is rendered with wgpu using SDF rounded rectangles and a frosted-glass blur effect.

| Field | Type | Default | Description |
|---|---|---|---|
| `bg` | String | `"#1F1F29F2"` | Palette background (supports alpha). |
| `border_color` | String | `"#4D4D66"` | Rounded rectangle border color. |
| `input_fg` | String | `"#F2F2F2"` | Input text color. |
| `separator_color` | String | `"#40404D"` | Separator line between input and results. |
| `command_fg` | String | `"#CCCCD1"` | Command name text color. |
| `selected_bg` | String | `"#383852"` | Background of the selected command. |
| `description_fg` | String | `"#808088"` | Command description text color. |
| `overlay_color` | String | `"#00000080"` | Dark overlay drawn behind the palette. |
| `max_height` | f32 | `400.0` | Maximum palette height in pixels. |
| `max_visible_items` | usize | `10` | Maximum visible commands before scrolling. |
| `width_ratio` | f32 | `0.6` | Palette width as a fraction of the window width (`0.0`--`1.0`). |
| `corner_radius` | f32 | `12.0` | Corner radius in pixels for the SDF rounded rectangle. |
| `blur_radius` | f32 | `20.0` | Gaussian blur radius in pixels for frosted-glass effect. `0` disables blur. |
| `shadow_radius` | f32 | `8.0` | Drop shadow blur radius in pixels. |
| `shadow_opacity` | f32 | `0.3` | Drop shadow opacity (`0.0`--`1.0`). |
| `border_width` | f32 | `1.0` | Border width in pixels for the rounded rectangle outline. |

**Example:**

```toml
[palette]
bg = "#14141EF0"
width_ratio = 0.5
corner_radius = 16.0
blur_radius = 24.0
max_visible_items = 12
```

---

### [status_bar]

A configurable status bar at the bottom of the window with left-aligned and right-aligned segments. Each segment can contain template variables.

#### Top-Level Fields

| Field | Type | Default | Description |
|---|---|---|---|
| `enabled` | bool | `true` | Show or hide the status bar. |
| `height` | f32 | `28.0` | Status bar height in pixels. |
| `background` | String | `"#141420"` | Status bar background color. |
| `padding_x` | f32 | `8.0` | Horizontal padding in pixels. |
| `top_border` | bool | `true` | Show a top border above the status bar. |
| `top_border_color` | String | `"#2A2A34"` | Top border color. |

#### Segment Arrays

Segments are defined as TOML array-of-tables using `[[status_bar.left]]` and `[[status_bar.right]]`:

| Field | Type | Default | Description |
|---|---|---|---|
| `content` | String | *(required)* | Template string with `{variable}` placeholders (see below). |
| `fg` | String | `"#CCCCCC"` | Segment foreground (text) color. |
| `bg` | String | `"#1A1A24"` | Segment background color. |

#### Template Variables

The following variables can be used in segment `content` strings:

| Variable | Description |
|---|---|
| `{user}` | Current username (`$USER`). |
| `{host}` | System hostname. |
| `{cwd}` | Full current working directory path. |
| `{cwd_short}` | Shortened current working directory (home replaced with `~`). |
| `{git_branch}` | Current git branch name. |
| `{git_status}` | Git status summary. |
| `{git_remote}` | Git remote name. |
| `{git_worktree}` | Git worktree path. |
| `{git_stash}` | Git stash count. |
| `{git_ahead}` | Number of commits ahead of remote. |
| `{git_behind}` | Number of commits behind remote. |
| `{git_dirty}` | Number of dirty (modified) files. |
| `{git_untracked}` | Number of untracked files. |
| `{ports}` | Listening ports detected in the session. |
| `{shell}` | Shell name (e.g. `zsh`, `bash`). |
| `{pid}` | Shell process ID. |
| `{pane_size}` | Current pane dimensions (columns x rows). |
| `{font_size}` | Current font size. |
| `{workspace}` | Current workspace name. |
| `{workspace_index}` | Current workspace index (1-based). |
| `{tab}` | Current tab name. |
| `{tab_index}` | Current tab index (1-based). |
| `{time}` | Current local time (HH:MM). |
| `{date}` | Current local date (YYYY-MM-DD). |

**Example:**

```toml
[status_bar]
enabled = true
height = 30.0
background = "#0A0A10"
top_border = true
top_border_color = "#1A1A28"

[[status_bar.left]]
content = "{user}@{host}"
fg = "#1A1A28"
bg = "#7AA2F7"

[[status_bar.left]]
content = "{cwd_short}"
fg = "#C0C0CC"
bg = "#1A1A28"

[[status_bar.left]]
content = "{git_branch} +{git_ahead} -{git_behind} *{git_dirty}"
fg = "#A6E3A1"
bg = "#0F0F18"

[[status_bar.right]]
content = "{ports}"
fg = "#94E2D5"
bg = "#0F0F18"

[[status_bar.right]]
content = "{shell}"
fg = "#606070"
bg = "#1A1A28"

[[status_bar.right]]
content = "{pane_size}"
fg = "#606070"
bg = "#0F0F18"

[[status_bar.right]]
content = "{time}"
fg = "#1A1A28"
bg = "#7AA2F7"
```

---

### [notifications]

Desktop notification behavior (delivered via NSNotificationCenter).

| Field | Type | Default | Description |
|---|---|---|---|
| `enabled` | bool | `true` | Enable desktop notifications. |
| `sound` | bool | `true` | Play a sound with notifications. |

**Example:**

```toml
[notifications]
enabled = true
sound = true
```

---

### [allow_flow]

Allow Flow is the AI agent coordination system that detects when tools like Claude Code request permission and lets you approve or deny from the sidebar or an overlay.

| Field | Type | Default | Description |
|---|---|---|---|
| `overlay_enabled` | bool | `true` | Show an overlay in the terminal pane when a permission request is pending. |
| `side_panel_enabled` | bool | `true` | Show pending requests in the sidebar panel. |
| `auto_focus` | bool | `false` | Automatically focus the pane when a permission request is detected. |
| `sound` | bool | `false` | Play a sound when a permission request is detected. |

#### Custom Detection Patterns

You can add custom regex patterns to detect permission prompts from any tool using `[[allow_flow.patterns]]`:

| Field | Type | Default | Description |
|---|---|---|---|
| `tool` | String | *(required)* | Name of the tool this pattern matches (e.g. `"My CLI Tool"`). |
| `action` | String | *(required)* | Human-readable description of the action (e.g. `"file write"`). |
| `pattern` | String | *(required)* | Regex pattern to match against terminal output. |
| `yes_response` | String | `"y\n"` | String to write to the PTY to approve the request. |
| `no_response` | String | `"n\n"` | String to write to the PTY to deny the request. |

**Example:**

```toml
[allow_flow]
overlay_enabled = true
side_panel_enabled = true
auto_focus = true
sound = true

[[allow_flow.patterns]]
tool = "My Deploy Tool"
action = "production deploy"
pattern = "Deploy to production\\? \\[y/N\\]"
yes_response = "y\n"
no_response = "n\n"

[[allow_flow.patterns]]
tool = "Database CLI"
action = "destructive query"
pattern = "This will delete \\d+ rows\\. Continue\\?"
yes_response = "yes\n"
no_response = "no\n"
```

---

### [quick_terminal]

A drop-down (Quake-style) terminal that slides from the screen edge via a global hotkey.

| Field | Type | Default | Description |
|---|---|---|---|
| `enabled` | bool | `true` | Enable the quick terminal feature. |
| `hotkey` | String | `"ctrl+\`"` | Global hotkey to toggle the quick terminal (active even when termojinal is not focused). |
| `animation` | String | `"slide_down"` | Animation style. Options: `"slide_down"`, `"slide_up"`, `"fade"`, `"none"`. |
| `animation_duration_ms` | u32 | `200` | Animation duration in milliseconds. |
| `height_ratio` | f32 | `0.4` | Quick terminal height as a fraction of the screen height (`0.0`--`1.0`). |
| `width_ratio` | f32 | `1.0` | Quick terminal width as a fraction of the screen width (`0.0`--`1.0`). |
| `position` | String | `"center"` | Horizontal position on screen. Options: `"left"`, `"center"`, `"right"`. |
| `screen_edge` | String | `"top"` | Screen edge the terminal slides from. Options: `"top"`, `"bottom"`. |
| `hide_on_focus_loss` | bool | `false` | Hide the quick terminal when it loses focus. |
| `dismiss_on_esc` | bool | `true` | Hide the quick terminal when Escape is pressed. |
| `show_sidebar` | bool | `false` | Show the sidebar in the quick terminal window. |
| `show_tab_bar` | bool | `false` | Show the tab bar in the quick terminal window. |
| `show_status_bar` | bool | `true` | Show the status bar in the quick terminal window. |
| `window_level` | String | `"floating"` | Window stacking level. Options: `"normal"`, `"floating"`, `"above_all"`. |
| `corner_radius` | f32 | `12.0` | Corner radius for the quick terminal window in pixels. |
| `own_workspace` | bool | `true` | Give the quick terminal its own dedicated workspace. |

**Example:**

```toml
[quick_terminal]
enabled = true
hotkey = "ctrl+`"
animation = "slide_down"
animation_duration_ms = 150
height_ratio = 0.5
width_ratio = 0.8
position = "center"
screen_edge = "top"
hide_on_focus_loss = true
show_status_bar = true
window_level = "floating"
corner_radius = 16.0
```

---

## Keybindings

Keybindings are configured in a separate file:

- **Primary:** `~/.config/termojinal/keybindings.toml`
- **Fallback:** `~/Library/Application Support/termojinal/keybindings.toml`

### Three Layers

Termojinal uses a three-layer keybinding system. Each layer is a TOML table mapping key combinations to action names:

| Layer | Table | When Active |
|---|---|---|
| **normal** | `[normal]` | When termojinal is focused and a regular shell is running. |
| **global** | `[global]` | Even when termojinal is not focused (via macOS CGEventTap). |
| **alternate_screen** | `[alternate_screen]` | When a TUI application (e.g. nvim, htop) is running in alternate screen mode. |

User-specified bindings are merged with defaults: your overrides replace the defaults for those keys, but all other default bindings are preserved.

### Default Keybindings

#### Normal Layer

| Key | Action | Description |
|---|---|---|
| `cmd+d` | `split_right` | Split the current pane to the right. |
| `cmd+shift+d` | `split_down` | Split the current pane downward. |
| `cmd+shift+enter` | `zoom_pane` | Toggle zoom on the current pane. |
| `cmd+]` | `next_pane` | Focus the next pane. |
| `cmd+[` | `prev_pane` | Focus the previous pane. |
| `cmd+t` | `new_tab` | Create a new tab. |
| `cmd+w` | `close_tab` | Close the current pane (cascades to tab/workspace/app). |
| `cmd+n` | `new_workspace` | Create a new workspace. |
| `cmd+shift+}` | `next_tab` | Switch to the next tab. |
| `cmd+shift+{` | `prev_tab` | Switch to the previous tab. |
| `cmd+shift+]` | `next_workspace` | Switch to the next workspace. |
| `cmd+shift+[` | `prev_workspace` | Switch to the previous workspace. |
| `cmd+1` .. `cmd+9` | `workspace(N)` | Switch to workspace N. |
| `cmd+shift+p` | `command_palette` | Open the command palette. |
| `cmd+,` | `open_settings` | Open settings. |
| `cmd+c` | `copy` | Copy selection to clipboard. |
| `cmd+v` | `paste` | Paste from clipboard. |
| `cmd+f` | `search` | Open the search bar. |
| `cmd+k` | `clear_scrollback` | Clear scrollback buffer and screen. |
| `cmd+l` | `clear_screen` | Clear the screen (send ESC[2J ESC[H to PTY). |
| `cmd+=` | `font_increase` | Increase font size. |
| `cmd+-` | `font_decrease` | Decrease font size. |
| `cmd+b` | `toggle_sidebar` | Toggle the sidebar. |
| `cmd+q` | `quit` | Quit the application. |

#### Global Layer

| Key | Action | Description |
|---|---|---|
| `ctrl+\`` | `toggle_quick_terminal` | Toggle the Quick Terminal visor window. |

#### Alternate Screen Layer

No default bindings. Use this layer to override keys when a TUI (nvim, htop, etc.) is running.

### All Available Actions

| Action Name | Description |
|---|---|
| `split_right` | Split the current pane to the right. |
| `split_down` | Split the current pane downward. |
| `zoom_pane` | Toggle zoom on the current pane. |
| `next_pane` | Focus the next pane. |
| `prev_pane` | Focus the previous pane. |
| `new_tab` | Create a new tab in the current workspace. |
| `close_tab` | Close the current pane (cascades to tab/workspace/app). |
| `new_workspace` | Create a new workspace. |
| `next_tab` | Switch to the next tab within the current workspace. |
| `prev_tab` | Switch to the previous tab within the current workspace. |
| `next_workspace` | Switch to the next workspace. |
| `prev_workspace` | Switch to the previous workspace. |
| `workspace(N)` | Switch to workspace N (1--9). In TOML: `{ "workspace" = 3 }`. |
| `command_palette` | Open the command palette. |
| `allow_flow_panel` | Open the Allow Flow AI panel. |
| `unread_jump` | Jump to the next unread notification. |
| `font_increase` | Increase font size. |
| `font_decrease` | Decrease font size. |
| `copy` | Copy selection to clipboard. |
| `paste` | Paste from clipboard. |
| `search` | Open the search bar. |
| `open_settings` | Open settings. |
| `clear_screen` | Clear the screen. |
| `clear_scrollback` | Clear scrollback buffer and screen. |
| `toggle_sidebar` | Toggle the sidebar. |
| `toggle_quick_terminal` | Toggle the Quick Terminal visor window. |
| `passthrough` | Forward the key directly to the PTY (bypass termojinal). |
| `quit` | Quit the application. |
| `about` | Show the About screen (license, credits, version). |
| `none` | Ignore the key entirely (disables a default binding). |
| `{ "command" = "name" }` | Run a named command or plugin. |

### Example Overrides

```toml
# ~/.config/termojinal/keybindings.toml

[normal]
# Remap Cmd+D to open a new tab instead of splitting
"cmd+d" = "new_tab"

# Disable Cmd+Q (prevent accidental quit)
"cmd+q" = "none"

# Add a custom keybinding to open the Allow Flow panel
"cmd+shift+a" = "allow_flow_panel"

# Run a custom command
"cmd+shift+r" = { "command" = "my_plugin" }

[global]
# Use a different hotkey for quick terminal
"cmd+shift+space" = "toggle_quick_terminal"

# Open command palette globally
"ctrl+shift+p" = "command_palette"

[alternate_screen]
# Pass Cmd+C through to TUI apps (e.g. for nvim's copy)
"cmd+c" = "passthrough"
"cmd+v" = "passthrough"
```

---

## Color Format

All color fields accept CSS-style hex color strings in three formats:

| Format | Example | Description |
|---|---|---|
| `#RGB` | `#F00` | 4-bit per channel, expanded to 8-bit (e.g. `#F00` becomes `#FF0000`). |
| `#RRGGBB` | `#FF0000` | Standard 8-bit per channel, fully opaque. |
| `#RRGGBBAA` | `#FF000080` | 8-bit per channel with alpha. `00` = transparent, `FF` = opaque. |

The alpha channel is particularly useful for translucent UI elements like the search bar (`bar_bg`), command palette (`bg`, `overlay_color`), and pane focus borders (`focus_border_color`).

**Examples:**

```toml
# Fully opaque red
cursor = "#FF0000"

# Semi-transparent black overlay
overlay_color = "#00000080"

# Translucent background for the search bar
bar_bg = "#262633F2"

# Shorthand blue
cursor = "#00F"
```

---

## Theme Files

Theme files let you define reusable color schemes that can be loaded by name.

### Location

```
~/.config/termojinal/themes/<name>.toml
```

For example, `~/.config/termojinal/themes/nord.toml`.

### Structure

A theme file has the same structure as the `[theme]` section in `config.toml`, but without the `[theme]` header. All fields are optional; unspecified fields fall back to the built-in defaults.

**Example** (`~/.config/termojinal/themes/nord.toml`):

```toml
background = "#2E3440"
foreground = "#D8DEE9"
cursor = "#D8DEE9"
selection_bg = "#434C5E"

black = "#3B4252"
bright_black = "#4C566A"
red = "#BF616A"
bright_red = "#BF616A"
green = "#A3BE8C"
bright_green = "#A3BE8C"
yellow = "#EBCB8B"
bright_yellow = "#EBCB8B"
blue = "#81A1C1"
bright_blue = "#81A1C1"
magenta = "#B48EAD"
bright_magenta = "#B48EAD"
cyan = "#88C0D0"
bright_cyan = "#8FBCBB"
white = "#E5E9F0"
bright_white = "#ECEFF4"
```

### Auto-Switching with Dark/Light Mode

To automatically switch themes when macOS changes between dark and light appearance, set `auto_switch = true` in the `[theme]` section and provide theme file names:

```toml
[theme]
auto_switch = true
dark = "catppuccin-mocha"    # loads ~/.config/termojinal/themes/catppuccin-mocha.toml
light = "catppuccin-latte"   # loads ~/.config/termojinal/themes/catppuccin-latte.toml
```

When `auto_switch` is enabled, termojinal monitors the system appearance and loads the corresponding theme file automatically. The `dark` and `light` values are theme file names without the `.toml` extension.
