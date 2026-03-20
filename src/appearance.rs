//! macOS system appearance detection (Dark/Light mode).
//!
//! Uses objc2 to query `NSApplication.effectiveAppearance` for the current
//! system appearance. Falls back to `Dark` on non-macOS platforms.

use crate::config::Appearance;

/// Detect the current macOS system appearance (Dark or Light).
///
/// Queries `NSApp.effectiveAppearance.name` and checks whether it contains
/// "Dark". Falls back to `Appearance::Dark` if detection fails.
#[cfg(target_os = "macos")]
pub fn detect_macos_appearance() -> Appearance {
    use objc2::{class, msg_send, msg_send_id};
    use objc2::rc::Id;
    use objc2::runtime::NSObject;

    unsafe {
        // Get NSApplication.sharedApplication
        let cls = class!(NSApplication);
        let app: Id<NSObject> = msg_send_id![cls, sharedApplication];

        // Get effectiveAppearance
        let appearance: Option<Id<NSObject>> = msg_send_id![&*app, effectiveAppearance];
        let appearance = match appearance {
            Some(a) => a,
            None => {
                log::warn!("failed to get effectiveAppearance, defaulting to Dark");
                return Appearance::Dark;
            }
        };

        // Get appearance name (NSAppearanceName, which is an NSString)
        let name: Option<Id<NSObject>> = msg_send_id![&*appearance, name];
        let name = match name {
            Some(n) => n,
            None => {
                log::warn!("failed to get appearance name, defaulting to Dark");
                return Appearance::Dark;
            }
        };

        // Convert NSString to &str via UTF8String
        let utf8: *const std::ffi::c_char = msg_send![&*name, UTF8String];
        if utf8.is_null() {
            log::warn!("appearance name UTF8String is null, defaulting to Dark");
            return Appearance::Dark;
        }
        let name_str = std::ffi::CStr::from_ptr(utf8).to_string_lossy();
        log::info!("macOS appearance: {}", name_str);

        if name_str.contains("Dark") {
            Appearance::Dark
        } else {
            Appearance::Light
        }
    }
}

/// On non-macOS platforms, always returns `Dark`.
#[cfg(not(target_os = "macos"))]
pub fn detect_macos_appearance() -> Appearance {
    Appearance::Dark
}
