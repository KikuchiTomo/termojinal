//! Quick Terminal state and animations.

use crate::AppState;

pub(crate) struct QuickTerminalState {
    /// Quick Terminal mode is active (has been initialized).
    pub(crate) active: bool,
    /// Window is currently visible/shown.
    pub(crate) visible: bool,
    /// In-progress animation (None when idle).
    pub(crate) animation: Option<QuickTerminalAnimation>,
    /// Dedicated workspace index (if own_workspace = true).
    pub(crate) workspace_idx: Option<usize>,
}

#[allow(dead_code)]
pub(crate) struct QuickTerminalAnimation {
    pub(crate) start_time: std::time::Instant,
    pub(crate) duration: std::time::Duration,
    pub(crate) kind: AnimationKind,
}

#[allow(dead_code)]
pub(crate) enum AnimationKind {
    SlideDown { from_y: f64, to_y: f64 },
    SlideUp { from_y: f64, to_y: f64 },
    FadeIn,
    FadeOut,
}

impl QuickTerminalState {
    pub(crate) fn new() -> Self {
        Self {
            active: false,
            visible: false,
            animation: None,
            workspace_idx: None,
        }
    }
}

// ---------------------------------------------------------------------------
// Quick Terminal show/hide/toggle logic
// ---------------------------------------------------------------------------

/// Toggle the Quick Terminal: show if hidden, hide if visible.
pub(crate) fn toggle_quick_terminal(state: &mut AppState) {
    let qtc = &state.config.quick_terminal;
    if !qtc.enabled {
        return;
    }

    if !state.quick_terminal.active {
        // First activation: mark as active.
        state.quick_terminal.active = true;
        show_quick_terminal(state);
    } else if state.quick_terminal.visible {
        hide_quick_terminal(state);
    } else {
        show_quick_terminal(state);
    }
}

/// Show the Quick Terminal window with optional slide-down animation.
pub(crate) fn show_quick_terminal(state: &mut AppState) {
    let qtc = &state.config.quick_terminal;

    // Get screen dimensions.
    let window_size = state.window.inner_size();
    let screen_h = window_size.height as f64 / state.scale_factor;

    // Target height based on config.
    let target_h = (screen_h * qtc.height_ratio as f64).round();

    // Make window visible and bring to front.
    state.window.set_visible(true);
    state.window.focus_window();

    // Start slide-down animation if configured.
    match qtc.animation.as_str() {
        "slide_down" => {
            state.quick_terminal.animation = Some(QuickTerminalAnimation {
                start_time: std::time::Instant::now(),
                duration: std::time::Duration::from_millis(qtc.animation_duration_ms as u64),
                kind: AnimationKind::SlideDown {
                    from_y: -target_h,
                    to_y: 0.0,
                },
            });
        }
        _ => {
            // No animation, just show immediately.
        }
    }

    state.quick_terminal.visible = true;

    // Switch to Quick Terminal workspace if it has a dedicated one.
    if let Some(ws_idx) = state.quick_terminal.workspace_idx {
        if ws_idx < state.workspaces.len() {
            state.active_workspace = ws_idx;
        }
    }

    // Bring app to front (macOS specific).
    #[cfg(target_os = "macos")]
    {
        activate_app();
    }

    state.window.request_redraw();
}

/// Hide the Quick Terminal window with optional slide-up animation.
pub(crate) fn hide_quick_terminal(state: &mut AppState) {
    let qtc = &state.config.quick_terminal;

    match qtc.animation.as_str() {
        "slide_down" => {
            let window_size = state.window.inner_size();
            let screen_h = window_size.height as f64 / state.scale_factor;
            let target_h = (screen_h * qtc.height_ratio as f64).round();
            state.quick_terminal.animation = Some(QuickTerminalAnimation {
                start_time: std::time::Instant::now(),
                duration: std::time::Duration::from_millis(qtc.animation_duration_ms as u64),
                kind: AnimationKind::SlideUp {
                    from_y: 0.0,
                    to_y: -target_h,
                },
            });
            // Don't hide the window yet — animation completion will hide it.
        }
        _ => {
            // No animation, hide immediately.
            state.window.set_visible(false);
            state.quick_terminal.visible = false;
        }
    }

    state.window.request_redraw();
}

/// Tick the Quick Terminal animation. Returns `true` if an animation is active
/// and we should keep requesting redraws.
pub(crate) fn tick_quick_terminal_animation(state: &mut AppState) -> bool {
    let anim = match state.quick_terminal.animation.as_ref() {
        Some(a) => a,
        None => return false,
    };

    let elapsed = anim.start_time.elapsed();
    let t = (elapsed.as_secs_f64() / anim.duration.as_secs_f64()).min(1.0);
    // Ease-out cubic: 1 - (1 - t)^3
    let eased = 1.0 - (1.0 - t).powi(3);

    let current_x = state
        .window
        .outer_position()
        .map(|p| p.x as f64)
        .unwrap_or(0.0);

    match &anim.kind {
        AnimationKind::SlideDown { from_y, to_y } | AnimationKind::SlideUp { from_y, to_y } => {
            let current_y = from_y + (to_y - from_y) * eased;
            let pos = winit::dpi::LogicalPosition::new(current_x, current_y);
            state.window.set_outer_position(pos);
        }
        AnimationKind::FadeIn | AnimationKind::FadeOut => {
            // Fade not yet implemented — would need NSWindow alpha.
        }
    }

    if t >= 1.0 {
        // Animation complete.
        let was_hiding = matches!(
            anim.kind,
            AnimationKind::SlideUp { .. } | AnimationKind::FadeOut
        );
        state.quick_terminal.animation = None;

        if was_hiding {
            state.window.set_visible(false);
            state.quick_terminal.visible = false;
        }
        false
    } else {
        // Keep animating.
        true
    }
}

/// Bring the application to the front on macOS.
#[cfg(target_os = "macos")]
pub(crate) fn activate_app() {
    use objc2::rc::Id;
    use objc2::runtime::NSObject;
    use objc2::{class, msg_send, msg_send_id};

    unsafe {
        let cls = class!(NSApplication);
        let app: Id<NSObject> = msg_send_id![cls, sharedApplication];
        let _: () = msg_send![&*app, activateIgnoringOtherApps: true];
    }
}
