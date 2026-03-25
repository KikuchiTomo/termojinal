//! Directory tree state and navigation.

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use winit::event_loop::EventLoopProxy;

use termojinal_ipc::daemon_connection::daemon_pty_write;
use termojinal_layout::{LayoutTree, PaneId};

use crate::config::{self, format_tab_title};
use crate::{
    active_tab, active_tab_mut, active_ws_mut, get_child_cwd, resize_all_panes, shell_escape,
    spawn_pane, tab_bar_visible, update_window_title, AppState, Tab, UserEvent,
};

pub(crate) struct DirEntry {
    /// Display name (file/directory name only).
    pub(crate) name: String,
    /// Full absolute path.
    pub(crate) path: String,
    /// Whether this entry is a directory.
    pub(crate) is_dir: bool,
    /// Indentation depth (0 = root level).
    pub(crate) depth: usize,
    /// Whether this directory is expanded (only meaningful for dirs).
    pub(crate) expanded: bool,
    /// Whether children have been loaded from the filesystem.
    pub(crate) children_loaded: bool,
}

/// Per-workspace directory tree state.
pub(crate) struct DirectoryTreeState {
    /// Whether the tree is visible for this workspace.
    pub(crate) visible: bool,
    /// Root path of the tree.
    pub(crate) root_path: String,
    /// Flattened list of visible entries.
    pub(crate) entries: Vec<DirEntry>,
    /// Index of the currently selected entry (keyboard navigation).
    pub(crate) selected: usize,
    /// Scroll offset (first visible entry index).
    pub(crate) scroll_offset: usize,
    /// Whether the tree has keyboard focus.
    pub(crate) focused: bool,
    /// Last time a click was registered (for double-click detection).
    pub(crate) last_click_time: Option<Instant>,
    /// Index of the last clicked entry (for double-click detection).
    pub(crate) last_click_index: Option<usize>,
    /// Pending "open in editor" request (set by click handler, consumed by event loop).
    pub(crate) pending_open_in_editor: bool,
    /// Whether find (prefix search) mode is active.
    pub(crate) find_active: bool,
    /// Current find query string.
    pub(crate) find_query: String,
    /// Number of visible lines computed during last render (for scroll/click handling).
    pub(crate) current_visible_lines: usize,
    /// Last resolved CWD used to detect pane CWD changes for tree root updates.
    pub(crate) last_resolved_cwd: String,
}

impl DirectoryTreeState {
    pub(crate) fn new() -> Self {
        Self {
            visible: false,
            root_path: String::new(),
            entries: Vec::new(),
            selected: 0,
            scroll_offset: 0,
            focused: false,
            last_click_time: None,
            last_click_index: None,
            pending_open_in_editor: false,
            find_active: false,
            find_query: String::new(),
            current_visible_lines: 0,
            last_resolved_cwd: String::new(),
        }
    }
}

/// Resolve the tree root directory for a workspace.
/// Uses git root if in a repo (when mode is Auto or GitRoot), otherwise CWD.
pub(crate) fn resolve_tree_root(cwd: &str, mode: &config::TreeRootMode) -> String {
    if cwd.is_empty() {
        return String::new();
    }
    match mode {
        config::TreeRootMode::Cwd => cwd.to_string(),
        config::TreeRootMode::GitRoot | config::TreeRootMode::Auto => {
            if let Ok(output) = std::process::Command::new("git")
                .args(["-C", cwd, "rev-parse", "--show-toplevel"])
                .output()
            {
                if output.status.success() {
                    let root = String::from_utf8_lossy(&output.stdout).trim().to_string();
                    if !root.is_empty() {
                        return root;
                    }
                }
            }
            if matches!(mode, config::TreeRootMode::Auto) {
                cwd.to_string()
            } else {
                cwd.to_string() // fallback for GitRoot when not in a repo
            }
        }
    }
}

/// Read a single directory level and return sorted entries.
/// Directories first (alphabetical), then files (alphabetical).
pub(crate) fn read_directory_entries(dir_path: &str, depth: usize) -> Vec<DirEntry> {
    let Ok(read_dir) = std::fs::read_dir(dir_path) else {
        return Vec::new();
    };
    let mut dirs: Vec<DirEntry> = Vec::new();
    let mut files: Vec<DirEntry> = Vec::new();

    for entry in read_dir.flatten() {
        let Ok(metadata) = entry.metadata() else {
            continue;
        };
        let name = entry.file_name().to_string_lossy().to_string();
        let path = entry.path().to_string_lossy().to_string();
        let is_dir = metadata.is_dir();

        let de = DirEntry {
            name,
            path,
            is_dir,
            depth,
            expanded: false,
            children_loaded: false,
        };

        if is_dir {
            dirs.push(de);
        } else {
            files.push(de);
        }
    }

    dirs.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    files.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    dirs.extend(files);
    dirs
}

/// Load the initial tree (1 level deep) from the root path.
pub(crate) fn load_tree_root(tree: &mut DirectoryTreeState, root: &str) {
    tree.root_path = root.to_string();
    tree.entries = read_directory_entries(root, 0);
    tree.selected = 0;
    tree.scroll_offset = 0;
}

/// Return a color based on file extension for the directory tree.
pub(crate) fn file_extension_color(name: &str) -> [f32; 4] {
    let ext = name.rsplit('.').next().unwrap_or("");
    match ext {
        "rs" => [1.0, 0.6, 0.2, 1.0], // orange — Rust
        "toml" | "json" | "yaml" | "yml" => [0.9, 0.85, 0.4, 1.0], // yellow — config
        "md" => [0.5, 0.7, 1.0, 1.0], // blue — markdown
        "js" | "jsx" => [0.95, 0.85, 0.3, 1.0], // yellow — JavaScript
        "ts" | "tsx" => [0.3, 0.6, 0.95, 1.0], // blue — TypeScript
        "py" => [0.4, 0.75, 0.6, 1.0], // blue-green — Python
        "c" | "h" => [0.6, 0.6, 0.9, 1.0], // light purple — C
        "cpp" | "cc" | "cxx" | "hpp" => [0.7, 0.5, 0.85, 1.0], // purple — C++
        "go" => [0.3, 0.8, 0.85, 1.0], // cyan — Go
        "rb" => [0.9, 0.3, 0.3, 1.0], // red — Ruby
        "java" => [0.9, 0.55, 0.3, 1.0], // orange — Java
        "swift" => [1.0, 0.5, 0.25, 1.0], // orange — Swift
        "sh" | "bash" | "zsh" => [0.5, 0.8, 0.4, 1.0], // green — shell
        "html" | "htm" => [0.9, 0.45, 0.3, 1.0], // red-orange — HTML
        "css" | "scss" | "sass" => [0.35, 0.6, 0.95, 1.0], // blue — CSS
        "lock" => [0.4, 0.4, 0.45, 0.7], // dim grey — lock files
        "png" | "jpg" | "jpeg" | "gif" | "svg" | "ico" | "webp" => [0.7, 0.55, 0.85, 1.0], // purple — image
        _ => [0.6, 0.6, 0.65, 1.0], // default
    }
}

/// Return a Nerd Font devicon for the file based on extension or name.
pub(crate) fn file_icon(name: &str) -> &'static str {
    // Check special filenames first.
    let lower = name.to_lowercase();
    match lower.as_str() {
        "makefile" | "cmakelists.txt" => return "\u{E673} ", //
        "dockerfile" => return "\u{E7B0} ",                  //
        "license" | "licence" => return "\u{F0219} ",        // 󰈙
        ".gitignore" | ".gitmodules" | ".gitattributes" => return "\u{E702} ", //
        ".env" | ".env.local" => return "\u{F0462} ",        // 󰑢
        "cargo.toml" | "cargo.lock" => return "\u{E7A8} ",   //  (Rust)
        "package.json" | "package-lock.json" => return "\u{E74E} ", //
        "tsconfig.json" => return "\u{E628} ",               //  (TS)
        _ => {}
    }
    let ext = name.rsplit('.').next().unwrap_or("");
    match ext {
        "rs" => "\u{E7A8} ",                                                    //  Rust
        "c" | "h" => "\u{E61E} ",                                               //  C
        "cpp" | "cc" | "cxx" | "hpp" | "hh" => "\u{E61D} ",                     //  C++
        "go" => "\u{E626} ",                                                    //  Go
        "py" => "\u{E73C} ",                                                    //  Python
        "rb" => "\u{E739} ",                                                    //  Ruby
        "java" => "\u{E738} ",                                                  //  Java
        "js" | "mjs" | "cjs" => "\u{E781} ",                                    //  JavaScript
        "jsx" => "\u{E7BA} ",                                                   //  React
        "ts" | "mts" | "cts" => "\u{E628} ",                                    //  TypeScript
        "tsx" => "\u{E7BA} ",                                                   //  React (TS)
        "swift" => "\u{E755} ",                                                 //  Swift
        "kt" | "kts" => "\u{E634} ",                                            //  Kotlin
        "html" | "htm" => "\u{E736} ",                                          //  HTML
        "css" => "\u{E749} ",                                                   //  CSS
        "scss" | "sass" => "\u{E74B} ",                                         //  Sass
        "json" => "\u{E60B} ",                                                  //  JSON
        "yaml" | "yml" => "\u{E6A8} ",                                          //  YAML
        "toml" => "\u{E615} ",                                                  //  Config
        "xml" => "\u{F05C0} ",                                                  // 󰗀 XML
        "md" | "mdx" => "\u{E73E} ",                                            //  Markdown
        "txt" => "\u{F0219} ",                                                  // 󰈙 Text
        "sh" | "bash" => "\u{E795} ",                                           //  Shell
        "zsh" => "\u{E795} ",                                                   //  Shell
        "fish" => "\u{E795} ",                                                  //  Shell
        "vim" | "vimrc" => "\u{E62B} ",                                         //  Vim
        "lua" => "\u{E620} ",                                                   //  Lua
        "sql" => "\u{E706} ",                                                   //  SQL
        "graphql" | "gql" => "\u{E662} ",                                       //  GraphQL
        "docker" => "\u{E7B0} ",                                                //  Docker
        "png" | "jpg" | "jpeg" | "gif" | "bmp" | "ico" | "webp" => "\u{F03E} ", //  Image
        "svg" => "\u{F0721} ",                                                  // 󰜡 SVG
        "pdf" => "\u{F0226} ",                                                  // 󰈦 PDF
        "zip" | "tar" | "gz" | "bz2" | "xz" | "7z" | "rar" => "\u{F0187} ",     // 󰆧 Archive
        "wasm" => "\u{E6A1} ",                                                  //  WebAssembly
        "lock" => "\u{F023} ",                                                  //  Lock
        "log" => "\u{F0219} ",                                                  // 󰈙 Log
        "env" => "\u{F0462} ",                                                  // 󰑢 Env
        "gitignore" => "\u{E702} ",                                             //  Git
        _ => "\u{F016} ",                                                       //  Generic file
    }
}

/// Update the directory tree root when the focused pane's CWD changes.
/// Detects CWD changes by comparing against `last_resolved_cwd` and reloads
/// the tree only when necessary.
pub(crate) fn update_tree_root_for_focused_pane(state: &mut AppState) {
    let wi = state.active_workspace;
    if wi >= state.dir_trees.len() || wi >= state.workspaces.len() {
        return;
    }
    if !state.dir_trees[wi].visible {
        return;
    }

    // Get the focused pane's CWD with multiple fallbacks.
    let cwd = {
        let ws = &state.workspaces[wi];
        let tab = &ws.tabs[ws.active_tab];
        let fid = tab.layout.focused();
        let pane = tab.panes.get(&fid);

        // 1. OSC 7 reported CWD (most reliable, set by shell integration).
        let osc_cwd = pane.map(|p| p.terminal.osc.cwd.clone()).unwrap_or_default();
        if !osc_cwd.is_empty() {
            osc_cwd
        } else {
            // 2. lsof process inspection (works even without shell integration).
            let pty_cwd = pane.and_then(|p| {
                let pid = p.shell_pid;
                if pid > 0 {
                    get_child_cwd(pid)
                } else {
                    None
                }
            });
            if let Some(cwd) = pty_cwd {
                cwd
            } else {
                // 3. Cached workspace info CWD.
                state
                    .workspace_infos
                    .get(wi)
                    .map(|inf| inf.cwd.clone())
                    .filter(|s| !s.is_empty())
                    .unwrap_or_else(|| {
                        // 4. Last resort: current process working directory.
                        std::env::current_dir()
                            .map(|p| p.to_string_lossy().to_string())
                            .unwrap_or_else(|_| "/".to_string())
                    })
            }
        }
    };

    // Only reload if CWD has changed.
    if cwd != state.dir_trees[wi].last_resolved_cwd {
        state.dir_trees[wi].last_resolved_cwd = cwd.clone();
        let root = resolve_tree_root(&cwd, &state.config.directory_tree.root_mode);
        if root != state.dir_trees[wi].root_path {
            load_tree_root(&mut state.dir_trees[wi], &root);
        }
    }
}

/// Toggle expand/collapse of a directory entry at the given index.
/// Find the first entry whose name starts with `tree.find_query` (case-insensitive)
/// and move selection to it. Searches from current position forward, wrapping around.
///
/// When the query contains `/`, segments are traversed left-to-right: each intermediate
/// segment finds a matching directory and auto-expands it, and the final segment is
/// prefix-matched among the newly-visible children.
pub(crate) fn dir_tree_find_match(tree: &mut DirectoryTreeState) {
    dir_tree_find_match_from(tree, 0);
}

/// Same as `dir_tree_find_match` but starts searching from `tree.selected + start_offset`.
/// `start_offset = 0` searches from current position (inclusive).
/// `start_offset = 1` skips the current match (used by Tab cycling).
pub(crate) fn dir_tree_find_match_from(tree: &mut DirectoryTreeState, start_offset: usize) {
    if tree.find_query.is_empty() || tree.entries.is_empty() {
        return;
    }
    let query = tree.find_query.to_lowercase();

    if query.contains('/') {
        // Path-segmented search: split on '/' and walk segments.
        dir_tree_find_match_path(tree, &query);
    } else {
        // Simple prefix search.
        dir_tree_find_match_simple(tree, &query, start_offset);
    }
}

/// Simple prefix match — searches visible entries for a name starting with `query`.
pub(crate) fn dir_tree_find_match_simple(tree: &mut DirectoryTreeState, query: &str, start_offset: usize) {
    let n = tree.entries.len();
    for offset in 0..n {
        let idx = (tree.selected + start_offset + offset) % n;
        if tree.entries[idx].name.to_lowercase().starts_with(query) {
            tree.selected = idx;
            dir_tree_ensure_visible(tree);
            return;
        }
    }
}

/// Path-segmented find: e.g. "docs/api" → expand `docs`, then search for `api` inside.
pub(crate) fn dir_tree_find_match_path(tree: &mut DirectoryTreeState, query: &str) {
    let segments: Vec<&str> = query.split('/').collect();

    // Current search scope: start index and the depth we expect entries to be at.
    let mut scope_start: usize = 0;
    let mut scope_end: usize = tree.entries.len();
    let mut scope_depth: Option<usize> = None; // None = any depth in scope

    for (si, seg) in segments.iter().enumerate() {
        let is_last = si == segments.len() - 1;

        if seg.is_empty() && is_last {
            // Trailing '/' — nothing more to search. The previous segment already
            // expanded the directory, so we're done.
            break;
        }

        let seg_lower = seg.to_lowercase();

        // Find first matching entry within scope.
        let mut found_idx: Option<usize> = None;
        for idx in scope_start..scope_end {
            // If we have a scope_depth, only consider entries at that depth.
            if let Some(d) = scope_depth {
                if tree.entries[idx].depth != d {
                    continue;
                }
            }
            if tree.entries[idx]
                .name
                .to_lowercase()
                .starts_with(&seg_lower)
            {
                found_idx = Some(idx);
                break;
            }
        }

        let Some(idx) = found_idx else {
            // No match for this segment — give up.
            return;
        };

        if is_last {
            // Final segment: select the match.
            tree.selected = idx;
            dir_tree_ensure_visible(tree);
            return;
        }

        // Intermediate segment: must be a directory — expand it if needed.
        if !tree.entries[idx].is_dir {
            // Not a directory, can't descend further.
            tree.selected = idx;
            dir_tree_ensure_visible(tree);
            return;
        }

        // Expand the directory if not already expanded.
        if !tree.entries[idx].expanded {
            toggle_tree_entry(tree, idx);
        }

        // Update scope to children of this directory.
        let child_depth = tree.entries[idx].depth + 1;
        scope_start = idx + 1;
        scope_end = scope_start;
        while scope_end < tree.entries.len() && tree.entries[scope_end].depth >= child_depth {
            scope_end += 1;
        }
        scope_depth = Some(child_depth);

        // Select the directory we just expanded.
        tree.selected = idx;
    }

    // If we exit the loop without returning (e.g. trailing slash),
    // ensure the selection is visible.
    dir_tree_ensure_visible(tree);
}

/// Ensure `tree.selected` is within the visible scroll window.
pub(crate) fn dir_tree_ensure_visible(tree: &mut DirectoryTreeState) {
    let max_visible = if tree.current_visible_lines > 0 {
        tree.current_visible_lines
    } else {
        20 // fallback before first render
    };
    if tree.selected < tree.scroll_offset {
        tree.scroll_offset = tree.selected;
    } else if tree.selected >= tree.scroll_offset + max_visible {
        tree.scroll_offset = tree.selected.saturating_sub(max_visible / 2);
    }
}

pub(crate) fn toggle_tree_entry(tree: &mut DirectoryTreeState, index: usize) {
    if index >= tree.entries.len() || !tree.entries[index].is_dir {
        return;
    }

    if tree.entries[index].expanded {
        // Collapse: remove all children (entries with depth > this entry's depth
        // until we hit an entry at the same or lesser depth).
        let parent_depth = tree.entries[index].depth;
        let remove_start = index + 1;
        let mut remove_end = remove_start;
        while remove_end < tree.entries.len() && tree.entries[remove_end].depth > parent_depth {
            remove_end += 1;
        }
        tree.entries.drain(remove_start..remove_end);
        tree.entries[index].expanded = false;
        // Clamp selected index to avoid out-of-bounds after removing children.
        if tree.selected >= tree.entries.len() {
            tree.selected = tree.entries.len().saturating_sub(1);
        }
        // Also adjust scroll_offset if it's now past the end.
        if tree.scroll_offset >= tree.entries.len() {
            tree.scroll_offset = tree.entries.len().saturating_sub(1);
        }
    } else {
        // Expand: load children and insert them after this entry.
        let children =
            read_directory_entries(&tree.entries[index].path, tree.entries[index].depth + 1);
        tree.entries[index].expanded = true;
        tree.entries[index].children_loaded = true;
        let insert_at = index + 1;
        for (ci, child) in children.into_iter().enumerate() {
            tree.entries.insert(insert_at + ci, child);
        }
    }
}

/// Move selection down in the directory tree.
pub(crate) fn dir_tree_move_down(state: &mut AppState) {
    let wi = state.active_workspace;
    if wi >= state.dir_trees.len() {
        return;
    }
    let tree = &mut state.dir_trees[wi];
    if tree.entries.is_empty() {
        return;
    }
    if tree.selected + 1 < tree.entries.len() {
        tree.selected += 1;
        // Scroll if needed — use dynamic visible lines, fall back to config.
        let max_lines = if tree.current_visible_lines > 0 {
            tree.current_visible_lines
        } else {
            state.config.directory_tree.max_visible_lines
        };
        if tree.selected >= tree.scroll_offset + max_lines {
            tree.scroll_offset = tree.selected + 1 - max_lines;
        }
    }
    state.window.request_redraw();
}

/// Move selection up in the directory tree.
pub(crate) fn dir_tree_move_up(state: &mut AppState) {
    let wi = state.active_workspace;
    if wi >= state.dir_trees.len() {
        return;
    }
    let tree = &mut state.dir_trees[wi];
    if tree.entries.is_empty() {
        return;
    }
    if tree.selected > 0 {
        tree.selected -= 1;
        if tree.selected < tree.scroll_offset {
            tree.scroll_offset = tree.selected;
        }
    }
    state.window.request_redraw();
}

/// Expand the selected directory entry (or move into it if already expanded).
pub(crate) fn dir_tree_expand(state: &mut AppState) {
    let wi = state.active_workspace;
    if wi >= state.dir_trees.len() {
        return;
    }
    let idx = state.dir_trees[wi].selected;
    if idx >= state.dir_trees[wi].entries.len() {
        return;
    }
    if state.dir_trees[wi].entries[idx].is_dir {
        if state.dir_trees[wi].entries[idx].expanded {
            // Already expanded: move to first child.
            if idx + 1 < state.dir_trees[wi].entries.len()
                && state.dir_trees[wi].entries[idx + 1].depth
                    > state.dir_trees[wi].entries[idx].depth
            {
                state.dir_trees[wi].selected = idx + 1;
            }
        } else {
            toggle_tree_entry(&mut state.dir_trees[wi], idx);
        }
    }
    state.window.request_redraw();
}

/// Collapse the selected directory entry (or move to parent if already collapsed/is a file).
pub(crate) fn dir_tree_collapse(state: &mut AppState) {
    let wi = state.active_workspace;
    if wi >= state.dir_trees.len() {
        return;
    }
    let idx = state.dir_trees[wi].selected;
    if idx >= state.dir_trees[wi].entries.len() {
        return;
    }

    if state.dir_trees[wi].entries[idx].is_dir && state.dir_trees[wi].entries[idx].expanded {
        // Collapse this directory.
        toggle_tree_entry(&mut state.dir_trees[wi], idx);
    } else {
        // Move to parent directory.
        let current_depth = state.dir_trees[wi].entries[idx].depth;
        if current_depth > 0 {
            // Search backwards for the parent (entry with depth - 1).
            let mut pi = idx;
            while pi > 0 {
                pi -= 1;
                if state.dir_trees[wi].entries[pi].depth < current_depth {
                    state.dir_trees[wi].selected = pi;
                    if pi < state.dir_trees[wi].scroll_offset {
                        state.dir_trees[wi].scroll_offset = pi;
                    }
                    break;
                }
            }
        }
    }
    state.window.request_redraw();
}

/// cd to the selected entry (directory) in the active pane.
/// cd to a directory from the file finder palette.
pub(crate) fn palette_cd_to_dir(state: &mut AppState, path: &str) {
    let focused_id = active_tab(state).layout.focused();
    if let Some(pane) = active_tab_mut(state).panes.get_mut(&focused_id) {
        let cmd = format!("\x15cd {}\n", shell_escape(path));
        let _ = daemon_pty_write(&pane.session_id, cmd.as_bytes());
    }
}

/// Open a file in $EDITOR from the file finder palette (reuses dir_tree_open_in_editor logic).
pub(crate) fn palette_open_in_editor(
    state: &mut AppState,
    path: &str,
    proxy: &winit::event_loop::EventLoopProxy<UserEvent>,
    pty_buffers: &std::sync::Arc<
        std::sync::Mutex<std::collections::HashMap<PaneId, std::collections::VecDeque<Vec<u8>>>>,
    >,
) {
    // Resolve editor.
    let cfg_editor = state.config.directory_tree.editor.clone();
    let editor = if !cfg_editor.is_empty() {
        cfg_editor
    } else {
        std::env::var("EDITOR").unwrap_or_else(|_| "nvim".to_string())
    };

    // Determine CWD for the new tab.
    let file_path = std::path::Path::new(path);
    let tab_cwd = if file_path.is_dir() {
        path.to_string()
    } else {
        file_path
            .parent()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_default()
    };
    if tab_cwd.is_empty() {
        return;
    }

    // Compute pane dimensions.
    let cell_size = state.renderer.cell_size();
    let tab_bar_h = if tab_bar_visible(state) {
        state.config.tab_bar.height
    } else {
        0.0
    };
    let sidebar_w = if state.sidebar_visible {
        state.sidebar_width
    } else {
        0.0
    };
    let status_bar_h = if state.config.status_bar.enabled {
        state.config.status_bar.height
    } else {
        0.0
    };
    let (sw, sh) = state.renderer.surface_size();
    let phys_w = sw as f32;
    let phys_h = sh as f32;
    let avail_w = phys_w - sidebar_w;
    let avail_h = phys_h - tab_bar_h - status_bar_h;
    let cols = (avail_w / cell_size.width).floor() as u16;
    let rows = (avail_h / cell_size.height).floor() as u16;
    let cjk_width = state.renderer.cjk_width;
    let fmt = state.config.tab_bar.format.clone();

    let pane_id = state.next_pane_id;
    state.next_pane_id += 1;
    let time_travel_cfg = Some(&state.config.time_travel);

    match spawn_pane(
        pane_id,
        cols,
        rows,
        proxy,
        pty_buffers,
        Some(tab_cwd),
        time_travel_cfg,
        cjk_width,
    ) {
        Ok(pane) => {
            let cmd = format!("{} {}\n", editor, shell_escape(path));
            let _ = daemon_pty_write(&pane.session_id, cmd.as_bytes());
            let layout = LayoutTree::new(pane_id);
            let mut panes = HashMap::new();
            panes.insert(pane_id, pane);
            let tab_name = std::path::Path::new(path)
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("editor")
                .to_string();
            let ws = &mut state.workspaces[state.active_workspace];
            let tab_num = ws.tabs.len() + 1;
            let tab = Tab {
                layout,
                panes,
                name: tab_name.clone(),
                display_title: format_tab_title(&fmt, &tab_name, "", tab_num),
            };
            ws.tabs.push(tab);
            ws.active_tab = ws.tabs.len() - 1;
            resize_all_panes(state);
        }
        Err(e) => {
            log::error!("palette_open_in_editor: failed to spawn pane: {e}");
        }
    }
}

pub(crate) fn dir_tree_cd(state: &mut AppState) {
    let wi = state.active_workspace;
    if wi >= state.dir_trees.len() {
        return;
    }
    let idx = state.dir_trees[wi].selected;
    if idx >= state.dir_trees[wi].entries.len() {
        return;
    }

    let entry = &state.dir_trees[wi].entries[idx];
    let path = if entry.is_dir {
        entry.path.clone()
    } else {
        // For files, cd to parent directory.
        std::path::Path::new(&entry.path)
            .parent()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_default()
    };

    if !path.is_empty() {
        let focused_id = active_tab(state).layout.focused();
        if let Some(pane) = active_tab_mut(state).panes.get_mut(&focused_id) {
            // Send Ctrl+U first to clear any partial input on the command line,
            // then the cd command. Prevents "fcd" or "cdcd" artifacts.
            let cmd = format!("\x15cd {}\n", shell_escape(&path));
            let _ = daemon_pty_write(&pane.session_id, cmd.as_bytes());
        }
    }
    state.window.request_redraw();
}

/// Open the selected file in $EDITOR (or nvim) in a new tab.
/// The tab is created with the shell, and the editor command is sent immediately
/// so that by the time the tab is displayed, the editor is already running.
pub(crate) fn dir_tree_open_in_editor(
    state: &mut AppState,
    proxy: &EventLoopProxy<UserEvent>,
    buffers: &Arc<Mutex<HashMap<PaneId, VecDeque<Vec<u8>>>>>,
) {
    let wi = state.active_workspace;
    if wi >= state.dir_trees.len() {
        return;
    }
    let idx = state.dir_trees[wi].selected;
    if idx >= state.dir_trees[wi].entries.len() {
        return;
    }

    let entry_path = state.dir_trees[wi].entries[idx].path.clone();
    let entry_is_dir = state.dir_trees[wi].entries[idx].is_dir;

    // Determine editor command (config > $EDITOR > nvim).
    let cfg_editor = &state.config.directory_tree.editor;
    let editor = if !cfg_editor.is_empty() {
        cfg_editor.clone()
    } else {
        std::env::var("EDITOR").unwrap_or_else(|_| "nvim".to_string())
    };

    // Determine CWD for the new tab (parent dir of the file, or the dir itself).
    let cwd = if entry_is_dir {
        Some(entry_path.clone())
    } else {
        std::path::Path::new(&entry_path)
            .parent()
            .map(|p| p.to_string_lossy().to_string())
    };

    // Create a new tab (similar to Action::NewTab).
    let new_id = state.next_pane_id;
    state.next_pane_id += 1;
    let layout = LayoutTree::new(new_id);
    let size = state.window.inner_size();
    let phys_w = size.width as f32;
    let phys_h = size.height as f32;
    let sidebar_w = if state.sidebar_visible {
        state.sidebar_width
    } else {
        0.0
    };
    let cw = (phys_w - sidebar_w).max(1.0);
    let ch = (phys_h - state.config.tab_bar.height).max(1.0);
    let (cols, rows) = state.renderer.grid_size_raw(cw as u32, ch as u32);

    let cjk_width = state.renderer.cjk_width;
    match spawn_pane(
        new_id,
        cols.max(1),
        rows.max(1),
        proxy,
        buffers,
        cwd,
        Some(&state.config.time_travel),
        cjk_width,
    ) {
        Ok(pane) => {
            // Immediately write the editor command to the PTY so it starts
            // before the user sees the tab (prevents flicker).
            let cmd = format!("{} {}\n", editor, shell_escape(&entry_path));
            let _ = daemon_pty_write(&pane.session_id, cmd.as_bytes());

            let mut panes = HashMap::new();
            panes.insert(new_id, pane);
            let fmt = state.config.tab_bar.format.clone();
            let ws = active_ws_mut(state);
            let tab_num = ws.tabs.len() + 1;

            // Use file/dir name as tab title.
            let tab_name = std::path::Path::new(&entry_path)
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("editor")
                .to_string();

            let tab = Tab {
                layout,
                panes,
                name: tab_name.clone(),
                display_title: format_tab_title(&fmt, &tab_name, "", tab_num),
            };
            ws.tabs.push(tab);
            ws.active_tab = ws.tabs.len() - 1;
            resize_all_panes(state);
            update_window_title(state);

            // Unfocus the tree since we're switching to the new tab.
            if wi < state.dir_trees.len() {
                state.dir_trees[wi].focused = false;
            }
            state.window.request_redraw();
        }
        Err(e) => {
            log::error!("failed to spawn pane for editor: {e}");
        }
    }
}
