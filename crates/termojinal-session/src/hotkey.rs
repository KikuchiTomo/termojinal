//! Global hotkey monitoring via macOS CGEventTap.
//!
//! Requires the Accessibility permission to be granted to the process.
//! If permission is denied, CGEventTapCreate returns NULL, and we gracefully
//! degrade by logging a warning and continuing without hotkey support.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;

/// Events that can be triggered by global hotkeys.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HotkeyEvent {
    /// Cmd+Shift+P -- open the Command Palette.
    CommandPalette,
    /// Cmd+Shift+A -- open the Allow Flow panel.
    AllowFlowPanel,
    /// Ctrl+` -- toggle the Quick Terminal visor window.
    QuickTerminal,
}

/// Error type for hotkey operations.
#[derive(Debug, thiserror::Error)]
pub enum HotkeyError {
    #[error("accessibility permission denied -- CGEventTap cannot be created")]
    AccessibilityDenied,
    #[error("failed to create run loop source from event tap")]
    RunLoopSourceFailed,
    #[error("failed to start hotkey monitor: {0}")]
    StartFailed(String),
}

/// Handle to the global hotkey monitor. Dropping this stops the event tap.
pub struct GlobalHotkey {
    running: Arc<AtomicBool>,
    _thread: thread::JoinHandle<()>,
}

impl GlobalHotkey {
    /// Start monitoring for global hotkeys.
    ///
    /// The `callback` is invoked on a dedicated background thread whenever a
    /// registered hotkey combination is pressed.
    ///
    /// Returns `Ok(GlobalHotkey)` on success, or `Err` if the event tap could
    /// not be created (typically due to missing Accessibility permission).
    pub fn start(
        callback: impl Fn(HotkeyEvent) + Send + 'static,
    ) -> Result<Self, HotkeyError> {
        let running = Arc::new(AtomicBool::new(true));
        let running_clone = running.clone();

        // We try creating the event tap on the dedicated thread and report
        // success/failure back via a oneshot channel.
        let (tx, rx) = std::sync::mpsc::sync_channel::<Result<(), HotkeyError>>(1);

        let thread = thread::Builder::new()
            .name("termojinal-hotkey".into())
            .spawn(move || {
                platform::run_event_tap(callback, running_clone, tx);
            })
            .map_err(|e| HotkeyError::StartFailed(e.to_string()))?;

        // Wait for the background thread to report whether the tap was created.
        match rx.recv() {
            Ok(Ok(())) => Ok(GlobalHotkey {
                running,
                _thread: thread,
            }),
            Ok(Err(e)) => Err(e),
            Err(_) => Err(HotkeyError::StartFailed(
                "hotkey thread exited prematurely".into(),
            )),
        }
    }

    /// Stop monitoring for global hotkeys.
    pub fn stop(&self) {
        self.running.store(false, Ordering::SeqCst);
        // The CFRunLoop will exit on the next iteration of the polling loop.
    }
}

impl Drop for GlobalHotkey {
    fn drop(&mut self) {
        self.stop();
    }
}

/// macOS virtual keycode for 'P'.
const KEYCODE_P: i64 = 35;
/// macOS virtual keycode for 'A'.
const KEYCODE_A: i64 = 0;
/// macOS virtual keycode for '`' (backtick / grave accent, US layout).
const KEYCODE_BACKTICK: i64 = 50;

#[cfg(target_os = "macos")]
mod platform {
    use super::*;
    use core_foundation::runloop::{kCFRunLoopCommonModes, CFRunLoop};
    use core_graphics::event::{
        CGEventFlags, CGEventTap, CGEventTapLocation, CGEventTapOptions, CGEventTapPlacement,
        CGEventType, EventField,
    };

    /// Check whether Cmd and Shift are both held (ignoring other modifiers).
    fn is_cmd_shift(flags: CGEventFlags) -> bool {
        flags.contains(CGEventFlags::CGEventFlagCommand)
            && flags.contains(CGEventFlags::CGEventFlagShift)
    }

    /// Check whether Control is held (ignoring other modifiers).
    fn is_ctrl(flags: CGEventFlags) -> bool {
        flags.contains(CGEventFlags::CGEventFlagControl)
    }

    /// Check Accessibility permission, prompting the user if not yet granted.
    /// Returns `true` if the process is trusted.
    fn check_accessibility_with_prompt() -> bool {
        use core_foundation::base::TCFType;
        use core_foundation::boolean::CFBoolean;
        use core_foundation::dictionary::CFDictionary;
        use core_foundation::string::CFString;

        #[link(name = "ApplicationServices", kind = "framework")]
        extern "C" {
            fn AXIsProcessTrustedWithOptions(
                options: core_foundation::base::CFTypeRef,
            ) -> bool;
        }

        let key = CFString::new("AXTrustedCheckOptionPrompt");
        let value = CFBoolean::true_value();
        let options = CFDictionary::from_CFType_pairs(&[(key, value)]);

        unsafe { AXIsProcessTrustedWithOptions(options.as_CFTypeRef()) }
    }

    pub(super) fn run_event_tap(
        callback: impl Fn(HotkeyEvent) + Send + 'static,
        running: Arc<AtomicBool>,
        tx: std::sync::mpsc::SyncSender<Result<(), HotkeyError>>,
    ) {
        // Prompt for Accessibility permission if not yet granted.
        if !check_accessibility_with_prompt() {
            log::warn!("Accessibility permission not yet granted — waiting for user approval");
            // Wait up to 30 seconds for the user to grant permission.
            for _ in 0..60 {
                std::thread::sleep(std::time::Duration::from_millis(500));
                if check_accessibility_with_prompt() {
                    log::info!("Accessibility permission granted!");
                    break;
                }
                if !running.load(Ordering::SeqCst) {
                    let _ = tx.send(Err(HotkeyError::AccessibilityDenied));
                    return;
                }
            }
        }

        // Create the CGEventTap using the crate's high-level API.
        let tap = CGEventTap::new(
            CGEventTapLocation::HID,
            CGEventTapPlacement::HeadInsertEventTap,
            CGEventTapOptions::ListenOnly,
            vec![CGEventType::KeyDown],
            move |_proxy, _event_type, event| {
                let flags = event.get_flags();
                let keycode =
                    event.get_integer_value_field(EventField::KEYBOARD_EVENT_KEYCODE);

                if is_cmd_shift(flags) {
                    match keycode {
                        KEYCODE_P => {
                            callback(HotkeyEvent::CommandPalette);
                        }
                        KEYCODE_A => {
                            callback(HotkeyEvent::AllowFlowPanel);
                        }
                        _ => {}
                    }
                }

                // Ctrl+` — toggle Quick Terminal.
                if is_ctrl(flags) && keycode == KEYCODE_BACKTICK {
                    callback(HotkeyEvent::QuickTerminal);
                }

                // ListenOnly tap -- always return None (we don't modify events).
                None
            },
        );

        let tap = match tap {
            Ok(tap) => tap,
            Err(()) => {
                // CGEventTapCreate returned NULL -- Accessibility permission denied.
                let _ = tx.send(Err(HotkeyError::AccessibilityDenied));
                return;
            }
        };

        // Create a CFRunLoopSource from the CGEventTap's mach port.
        let loop_source = match tap.mach_port.create_runloop_source(0) {
            Ok(source) => source,
            Err(()) => {
                let _ = tx.send(Err(HotkeyError::RunLoopSourceFailed));
                return;
            }
        };

        let current_loop = CFRunLoop::get_current();
        unsafe {
            current_loop.add_source(&loop_source, kCFRunLoopCommonModes);
        }

        // Enable the event tap.
        tap.enable();

        // Report success to the caller.
        let _ = tx.send(Ok(()));

        // Run the loop, checking periodically if we should stop.
        while running.load(Ordering::SeqCst) {
            unsafe {
                CFRunLoop::run_in_mode(
                    kCFRunLoopCommonModes,
                    std::time::Duration::from_millis(500),
                    false,
                );
            }
        }

        current_loop.stop();
        log::info!("global hotkey monitor stopped");
    }
}

#[cfg(not(target_os = "macos"))]
mod platform {
    use super::*;

    pub(super) fn run_event_tap(
        _callback: impl Fn(HotkeyEvent) + Send + 'static,
        _running: Arc<AtomicBool>,
        tx: std::sync::mpsc::SyncSender<Result<(), HotkeyError>>,
    ) {
        let _ = tx.send(Err(HotkeyError::StartFailed(
            "global hotkeys are only supported on macOS".into(),
        )));
    }
}
