//! macOS desktop notifications via Notification Center API.
//!
//! Uses `mac-notification-sys` which wraps `NSUserNotificationCenter` /
//! `UNUserNotificationCenter` natively. Falls back to `osascript` if
//! the native API fails.

/// Initialize the notification system. Call once at startup.
pub fn init() {
    // Set the application bundle so notifications show termojinal's icon.
    // "com.termojinal.app" is our bundle identifier.
    if let Err(e) = mac_notification_sys::set_application("com.termojinal.app") {
        log::debug!("notification init with bundle failed: {e}, trying default");
        // Fallback: use Terminal.app's bundle so at least a terminal icon shows.
        let _ = mac_notification_sys::set_application("com.apple.Terminal");
    }
}

/// Send a macOS desktop notification.
///
/// `title` — notification title (e.g. "termojinal").
/// `body`  — notification body text.
/// `sound` — if `true`, plays the default notification sound.
pub fn send_notification(title: &str, body: &str, sound: bool) {
    let mut notif = mac_notification_sys::Notification::new();
    if sound {
        notif.sound("default");
    }

    match mac_notification_sys::send_notification(title, None, body, Some(&notif)) {
        Ok(_) => {}
        Err(e) => {
            log::debug!("native notification failed ({e}), falling back to osascript");
            send_via_osascript(title, body, sound);
        }
    }
}

fn send_via_osascript(title: &str, body: &str, sound: bool) {
    let escaped_body = body.replace('\\', "\\\\").replace('"', "\\\"");
    let escaped_title = title.replace('\\', "\\\\").replace('"', "\\\"");

    let script = if sound {
        format!(
            "display notification \"{}\" with title \"{}\" sound name \"default\"",
            escaped_body, escaped_title
        )
    } else {
        format!(
            "display notification \"{}\" with title \"{}\"",
            escaped_body, escaped_title
        )
    };

    std::process::Command::new("osascript")
        .args(["-e", &script])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .ok();
}
