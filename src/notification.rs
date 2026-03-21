//! macOS desktop notifications via Notification Center API.
//!
//! Uses `mac-notification-sys` which wraps `NSUserNotificationCenter` natively.

/// Bundle identifier — matches Info.plist in the .app bundle.
fn bundle_id() -> &'static str {
    if cfg!(debug_assertions) {
        "com.termojinal.app.dev"
    } else {
        "com.termojinal.app"
    }
}

/// Initialize the notification system. Call once at startup.
pub fn init() {
    let bid = bundle_id();
    if let Err(e) = mac_notification_sys::set_application(bid) {
        log::warn!("notification init failed for {bid}: {e}");
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

    if let Err(e) = mac_notification_sys::send_notification(title, None, body, Some(&notif)) {
        log::warn!("notification failed: {e}");
    }
}

/// Request notification permission if not already granted.
///
/// Uses `UNUserNotificationCenter` to check the current authorization status.
/// Only runs when the process is inside a `.app` bundle (has a valid bundle
/// identifier). Bare CLI binaries (e.g. from Homebrew) are skipped because
/// `UNUserNotificationCenter` does not work correctly without proper code
/// signing and bundle identity.
#[cfg(target_os = "macos")]
pub fn request_notification_permission_if_needed() {
    use std::sync::mpsc;

    use block2::RcBlock;
    use objc2::runtime::{AnyClass, AnyObject, Bool};
    use objc2::{msg_send, msg_send_id};

    // Only proceed if running inside a .app bundle.
    // UNUserNotificationCenter requires a valid bundle identifier and code
    // signature; without one, authorizationStatus always returns NotDetermined
    // and the dialog reappears on every launch.
    let is_bundled = std::env::current_exe()
        .map(|p| p.to_string_lossy().contains(".app/Contents/MacOS/"))
        .unwrap_or(false);
    if !is_bundled {
        log::debug!("not running from .app bundle — skipping notification permission check");
        return;
    }

    let center_class = match AnyClass::get("UNUserNotificationCenter") {
        Some(cls) => cls,
        None => {
            log::warn!("UNUserNotificationCenter class not available");
            return;
        }
    };

    let center: objc2::rc::Id<AnyObject> =
        unsafe { msg_send_id![center_class, currentNotificationCenter] };

    // Check current notification authorization status.
    let (tx, rx) = mpsc::channel::<i64>();

    let check_block = RcBlock::new(move |settings: *mut AnyObject| {
        if settings.is_null() {
            let _ = tx.send(-1);
            return;
        }
        // UNAuthorizationStatus: 0 = NotDetermined, 1 = Denied, 2 = Authorized,
        // 3 = Provisional, 4 = Ephemeral
        let status: i64 = unsafe { msg_send![settings, authorizationStatus] };
        let _ = tx.send(status);
    });

    unsafe {
        let _: () = msg_send![&*center, getNotificationSettingsWithCompletionHandler: &*check_block];
    }

    // Wait for the result with a timeout.
    let status = match rx.recv_timeout(std::time::Duration::from_secs(5)) {
        Ok(s) => s,
        Err(_) => {
            log::warn!("timeout checking notification permission status");
            return;
        }
    };

    // UNAuthorizationStatus values:
    //   0 = NotDetermined, 1 = Denied, 2 = Authorized, 3 = Provisional, 4 = Ephemeral
    match status {
        2 | 3 | 4 => {
            log::info!("notification permission already granted (status={status})");
        }
        0 => {
            // User has never been asked — request permission.
            log::info!("notification permission not determined, requesting...");

            // UNAuthorizationOptionBadge | UNAuthorizationOptionSound | UNAuthorizationOptionAlert
            // = (1<<0) | (1<<1) | (1<<2) = 7
            let options: u64 = 7;

            let request_block =
                RcBlock::new(move |granted: Bool, _error: *mut AnyObject| {
                    if granted.as_bool() {
                        log::info!("notification permission granted by user");
                    } else {
                        log::info!("notification permission denied by user");
                    }
                });

            unsafe {
                let _: () = msg_send![&*center, requestAuthorizationWithOptions: options completionHandler: &*request_block];
            }
        }
        1 => {
            log::info!("notification permission denied (user previously denied)");
        }
        _ => {
            log::info!("notification permission status: unknown ({status})");
        }
    }
}
