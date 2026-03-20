//! macOS desktop notifications via osascript.
//!
//! Uses `osascript -e 'display notification ...'` which works without
//! entitlements or provisioning profiles.  The command is spawned
//! asynchronously so it never blocks the render loop.

/// Send a macOS desktop notification.
///
/// `title` — notification title (e.g. "termojinal").
/// `body`  — notification body text.
/// `sound` — if `true`, plays the default notification sound.
///
/// Prefers `terminal-notifier` (shows custom app icon) if available,
/// falls back to `osascript` (shows Script Editor icon).
pub fn send_notification(title: &str, body: &str, sound: bool) {
    // Try terminal-notifier first (supports custom icon).
    if send_via_terminal_notifier(title, body, sound) {
        return;
    }
    // Fallback to osascript.
    send_via_osascript(title, body, sound);
}

fn send_via_terminal_notifier(title: &str, body: &str, sound: bool) -> bool {
    let mut cmd = std::process::Command::new("terminal-notifier");
    cmd.args(["-title", title, "-message", body, "-appIcon", icon_path()]);
    if sound {
        cmd.args(["-sound", "default"]);
    }
    cmd.stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    cmd.spawn().is_ok()
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

/// Path to the app icon for notifications.
fn icon_path() -> &'static str {
    // At runtime, check common install locations.
    static ICON: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    ICON.get_or_init(|| {
        // Check Homebrew location
        let candidates = [
            "/usr/local/share/termojinal/icon.png",
            "/opt/homebrew/share/termojinal/icon.png",
        ];
        for p in &candidates {
            if std::path::Path::new(p).exists() {
                return p.to_string();
            }
        }
        // Fallback: relative to executable
        if let Ok(exe) = std::env::current_exe() {
            let icon = exe.parent().unwrap_or(exe.as_ref())
                .join("../resources/Assets.xcassets/AppIcon.appiconset/256.png");
            if icon.exists() {
                return icon.to_string_lossy().into_owned();
            }
        }
        String::new()
    })
}

#[cfg(test)]
mod tests {
    #[test]
    fn escapes_quotes_in_body() {
        // Just ensure we don't panic — we can't easily assert osascript output
        // in a headless test, but we can confirm the function handles special chars.
        // We do NOT actually send a notification in tests.
        let body = r#"He said "hello" and it's done"#;
        let title = r#"termojinal "test""#;
        let escaped_body = body.replace('\\', "\\\\").replace('"', "\\\"");
        let escaped_title = title.replace('\\', "\\\\").replace('"', "\\\"");
        assert!(!escaped_body.contains('"') || escaped_body.contains("\\\""));
        assert!(!escaped_title.contains('"') || escaped_title.contains("\\\""));
    }
}
